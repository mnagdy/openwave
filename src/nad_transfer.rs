//! Independently authored OFFLINE native coefficient package builder.
//! Contains no network/file/device operations and never activates or commits a slot.
//! New filter identity uses OS entropy and current system time.
use crate::{nad_v2, native_dsp};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

const RATES: [u32; 3] = [32000, 44100, 48000];
const MAX_PAYLOAD: usize = 1024 * 1024;
const MAX_CALLS: usize = 32;
const PREFIX: &str = "/dio.endpoint.EndpointDevice/";

/// Prepare a stereo package from included measurements. Performs no device writes.
pub fn prepare_session_package(
    session: &crate::session::Session,
    device: &nad_v2::NativeDevice,
    slot: u32,
    name: &str,
) -> Result<TransferPackage> {
    session.validate()?;
    let mut designs = Vec::new();
    for speaker in 0..2 {
        let measured = session.average(speaker)?;
        designs.push(native_dsp::design(
            &measured,
            &session.target,
            session.low_hz,
            session.high_hz,
            session.max_boost_db,
        )?);
    }
    build_package(device, &designs, slot, name)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ResponseKind {
    UnaryVoid,
    ServerStreamingProgress,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferCall {
    pub method: String,
    pub request_type: String,
    /// None is an explicit unresolved request, never an empty protobuf message.
    pub payload: Option<Vec<u8>>,
    pub response: ResponseKind,
    pub note: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferPackage {
    pub version: u32,
    pub slot: u32,
    pub name: String,
    pub filter_family: String,
    pub creation_time_ms: i64,
    pub unique_id: String,
    pub attenuation_db: f64,
    pub hardware_validated: bool,
    pub transfer_semantics_verified: bool,
    pub ready_for_device_write: bool,
    pub calls: Vec<TransferCall>,
    pub blockers: Vec<String>,
    pub topology_assumptions: Vec<String>,
    pub total_payload_bytes: usize,
    pub includes_activation: bool,
    pub model_summaries: Vec<FilterModelSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterModelSummary {
    pub speaker: u32,
    pub sample_rate: u32,
    pub filter_approximation_error_db: f64,
    pub room_target_residual_db: f64,
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push(value as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn scalar(out: &mut Vec<u8>, field: u32, value: u32) {
    if value != 0 {
        varint(out, u64::from(field) << 3);
        varint(out, value.into());
    }
}
fn scalar64(out: &mut Vec<u8>, field: u32, value: u64) {
    if value != 0 {
        varint(out, u64::from(field) << 3);
        varint(out, value);
    }
}
fn message(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    varint(out, (u64::from(field) << 3) | 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}
fn fixed_float(out: &mut Vec<u8>, field: u32, value: f32) {
    if value != 0. {
        varint(out, (u64::from(field) << 3) | 5);
        out.extend_from_slice(&value.to_le_bytes());
    }
}
fn info(meta: &nad_v2::NativeDualRateInfo) -> Vec<u8> {
    let mut b = Vec::new();
    scalar(&mut b, 1, meta.variant);
    fixed_float(&mut b, 2, meta.sample_rate);
    scalar(&mut b, 3, meta.high_taps);
    scalar(&mut b, 4, meta.low_taps);
    scalar(&mut b, 5, meta.sub_sampling_factor);
    scalar(&mut b, 6, meta.cascades);
    scalar(&mut b, 7, meta.high_delay_min);
    scalar(&mut b, 8, meta.high_delay_max);
    scalar(&mut b, 9, u32::from(meta.individual_delays));
    b
}
/// FIR deliberately contains only packed float32 taps: no invented nested FIRInfo.
fn fir(taps: &[f32]) -> Vec<u8> {
    let mut packed = Vec::with_capacity(taps.len() * 4);
    for tap in taps {
        packed.extend_from_slice(&tap.to_le_bytes());
    }
    let mut b = Vec::new();
    if !packed.is_empty() {
        message(&mut b, 2, &packed);
    }
    b
}
fn drfir(cell: &native_dsp::NativeCell, meta: &nad_v2::NativeDualRateInfo) -> Vec<u8> {
    let mut b = Vec::new();
    message(&mut b, 1, &info(meta));
    // The SDK converter creates both FIR messages, including the zero-high cross case.
    message(&mut b, 2, &fir(&cell.fir_hi));
    message(&mut b, 3, &fir(&cell.fir_lo));
    scalar(&mut b, 4, cell.delay_hi);
    fixed_float(&mut b, 5, cell.max_gain_db);
    b
}
fn common(taps: &[f32], meta: &nad_v2::NativeDualRateInfo) -> Vec<u8> {
    let mut b = Vec::new();
    message(&mut b, 1, &info(meta));
    message(&mut b, 2, &fir(taps));
    // section_index=0 is the proto3 scalar default, not an inferred additional section.
    b
}
fn cell_index(input: u32, output: u32) -> Vec<u8> {
    let mut b = Vec::new();
    scalar(&mut b, 1, input);
    scalar(&mut b, 2, output);
    b
}
fn slot_info(
    slot: u32,
    name: &str,
    family: &str,
    creation_time_ms: u64,
    uuid_low: u64,
    uuid_high: u64,
) -> Vec<u8> {
    let mut b = Vec::new();
    scalar(&mut b, 1, slot);
    message(&mut b, 2, name.as_bytes());
    message(&mut b, 3, family.as_bytes());
    scalar(&mut b, 4, 2);
    scalar(&mut b, 5, 2);
    scalar(&mut b, 9, 1);
    scalar64(&mut b, 10, creation_time_ms);
    let mut id = Vec::new();
    scalar64(&mut id, 1, uuid_low);
    scalar64(&mut id, 2, uuid_high);
    message(&mut b, 11, &id);
    // Omit max_delay/min_gain/max_gain defaults and unknown optional metadata.
    // These omissions are candidate metadata, not proof of firmware acceptance.
    b
}
fn call(
    method: &str,
    request_type: &str,
    payload: Option<Vec<u8>>,
    response: ResponseKind,
    note: &str,
) -> Result<TransferCall> {
    ensure!(
        payload.as_ref().is_none_or(|b| b.len() <= MAX_PAYLOAD),
        "transfer payload exceeds 1 MiB"
    );
    Ok(TransferCall {
        method: format!("{PREFIX}{method}"),
        request_type: request_type.into(),
        payload,
        response,
        note: note.into(),
    })
}
fn metadata(device: &nad_v2::NativeDevice, slot: u32) -> Result<()> {
    ensure!(
        device.manufacturer == "NAD" && device.model == "M10" && device.speakers == 2,
        "package supports only the inspected two-speaker NAD M10 profile"
    );
    ensure!(
        !device.coverage_limited,
        "complete native topology metadata is required"
    );
    ensure!(
        slot == 1 || slot == 2,
        "only public candidate slots 1 or 2 may be previewed"
    );
    ensure!(
        device.reported_slot_count >= 3,
        "public candidate slot metadata is incomplete"
    );
    ensure!(
        device
            .slots
            .iter()
            .map(|s| s.index)
            .collect::<BTreeSet<_>>()
            .len()
            == device.slots.len(),
        "duplicate slot metadata"
    );
    let selected = device
        .slots
        .iter()
        .find(|s| s.index == slot)
        .context("candidate slot was not inspected")?;
    ensure!(
        selected.name.is_empty(),
        "candidate slot is named: existing filter must be preserved"
    );
    ensure!(
        (selected.inputs, selected.outputs, selected.sections) == (2, 2, 1),
        "candidate slot topology differs"
    );
    ensure!(device.sections.len() == 1, "unsupported section count");
    let section = &device.sections[0];
    ensure!(
        (
            section.index,
            section.inputs,
            section.outputs,
            section.disposition
        ) == (0, 2, 2, 1),
        "unsupported LIVE section"
    );
    ensure!(
        device.cells.len() == 4,
        "complete four-cell topology is required"
    );
    let mut cells = BTreeSet::new();
    for c in &device.cells {
        ensure!(
            c.input < 2 && c.output < 2 && c.section == 0 && c.filter_type == 3,
            "unsupported cell metadata"
        );
        ensure!(cells.insert((c.input, c.output)), "duplicate cell metadata");
        let expected: BTreeSet<_> = if c.input == c.output {
            [0, 2, 4]
        } else {
            [1, 3, 5]
        }
        .into_iter()
        .collect();
        ensure!(
            c.variants.iter().copied().collect::<BTreeSet<_>>() == expected
                && c.variants.len() == 3,
            "unsupported or duplicate cell variants"
        );
    }
    ensure!(
        device.dual_rate_variants.len() == 6,
        "six advertised variants are required"
    );
    let mut variants = BTreeSet::new();
    for m in &device.dual_rate_variants {
        ensure!(
            m.variant < 6 && variants.insert(m.variant),
            "invalid or duplicate native variant"
        );
        ensure!(
            m.sample_rate.is_finite() && m.sample_rate == RATES[m.variant as usize / 2] as f32,
            "native sample rate differs from inspected profile"
        );
        ensure!(
            m.high_taps == (if m.variant % 2 == 0 { 1024 } else { 0 })
                && m.low_taps == 1024
                && m.sub_sampling_factor == 4
                && m.cascades == 2,
            "unsupported native tap/resampling dimensions"
        );
        ensure!(
            m.high_delay_min <= m.high_delay_max
                && m.high_delay_max <= 8192
                && !m.individual_delays,
            "unsupported high-delay capabilities"
        );
        ensure!(
            m.variant % 2 == 0 || (m.high_delay_min == 0 && m.high_delay_max == 0),
            "cross cell advertises unexpected delay"
        );
    }
    Ok(())
}
fn validate_designs(
    device: &nad_v2::NativeDevice,
    designs: &[native_dsp::NativeDesign],
) -> Result<()> {
    ensure!(
        designs.len() == 2,
        "exactly two independently designed speakers are required"
    );
    for design in designs {
        ensure!(
            design.attenuation_db.is_finite() && (0. ..=120.).contains(&design.attenuation_db),
            "invalid shared attenuation"
        );
        ensure!(
            design.rates.len() == 3,
            "all three native rates are required"
        );
        let mut variants = BTreeSet::new();
        for rate in &design.rates {
            let diagonal = &rate.diagonal;
            let cross = &rate.zero_cross;
            ensure!(
                diagonal.variant >= 0
                    && diagonal.variant <= 4
                    && diagonal.variant % 2 == 0
                    && variants.insert(diagonal.variant),
                "wrong or duplicate diagonal variant"
            );
            ensure!(cross.variant == diagonal.variant + 1, "wrong cross variant");
            for cell in [diagonal, cross] {
                let m = device
                    .dual_rate_variants
                    .iter()
                    .find(|m| m.variant == cell.variant as u32)
                    .context("missing coefficient capabilities")?;
                ensure!(
                    cell.sample_rate as f32 == m.sample_rate
                        && cell.sub_sampling_factor == m.sub_sampling_factor
                        && cell.cascade_number == m.cascades,
                    "coefficient rate/resampling differs from device metadata"
                );
                ensure!(
                    cell.fir_hi.len() == m.high_taps as usize
                        && cell.fir_lo.len() == m.low_taps as usize,
                    "coefficient tap count differs from advertised dimensions"
                );
                ensure!(
                    (m.high_delay_min..=m.high_delay_max).contains(&cell.delay_hi),
                    "coefficient delay exceeds advertised range"
                );
                ensure!(
                    cell.max_gain_db.is_finite() && cell.max_gain_db == 0.,
                    "package requires conservative 0 dB maximum gain"
                );
                ensure!(
                    cell.fir_hi
                        .iter()
                        .chain(&cell.fir_lo)
                        .all(|v| v.is_finite()),
                    "nonfinite native coefficients"
                );
            }
            ensure!(
                cross.fir_hi.is_empty()
                    && cross.fir_lo.iter().all(|v| *v == 0.)
                    && cross.delay_hi == 0,
                "off-diagonal path must be explicitly silent"
            );
            ensure!(
                rate.common_lowpass.len() == 75
                    && rate.common_lowpass.iter().all(|v| v.is_finite()),
                "unsupported common prototype"
            );
            let dc = rate.common_lowpass.iter().map(|v| *v as f64).sum::<f64>();
            ensure!(
                (dc - 1.).abs() <= 0.00001,
                "common prototype must retain unit DC"
            );
            ensure!(
                rate.model.induced_peak_bound.is_finite()
                    && (0. ..=1.000001).contains(&rate.model.induced_peak_bound)
                    && rate.model.max_shape_error_db.is_finite()
                    && (0. ..=1.).contains(&rate.model.max_shape_error_db)
                    && rate.model.max_crossover_phase_error_deg.is_finite()
                    && (0. ..=5.).contains(&rate.model.max_crossover_phase_error_deg)
                    && rate.model.max_alias_amplitude_db.is_finite()
                    && rate.model.max_alias_amplitude_db <= -70.,
                "conditional DSP model validation gate failed"
            );
        }
    }
    for variant in [0, 2, 4] {
        let a = designs[0]
            .rates
            .iter()
            .find(|r| r.diagonal.variant == variant)
            .context("missing left rate")?;
        let b = designs[1]
            .rates
            .iter()
            .find(|r| r.diagonal.variant == variant)
            .context("missing right rate")?;
        ensure!(
            a.common_lowpass == b.common_lowpass,
            "speakers must share identical common preprocessing"
        );
        ensure!(
            a.diagonal.delay_hi == b.diagonal.delay_hi,
            "profile forbids independently chosen speaker high delays"
        );
    }
    Ok(())
}

/// Build a reviewable original coefficient plan. No network, file or device I/O.
/// Begin uses observed compatible-family metadata with a newly generated identity.
/// Firmware acceptance of this candidate is unverified; no package is write-ready.
pub fn build_package(
    device: &nad_v2::NativeDevice,
    designs: &[native_dsp::NativeDesign],
    slot: u32,
    name: &str,
) -> Result<TransferPackage> {
    metadata(device, slot)?;
    ensure!(
        !name.trim().is_empty() && name.len() <= 80 && !name.chars().any(char::is_control),
        "filter display name must be 1–80 bytes without control characters"
    );
    validate_designs(device, designs)?;
    let mut aligned = designs.to_vec();
    let attenuation_db = native_dsp::align_speaker_headroom(&mut aligned)?;
    validate_designs(device, &aligned)?;
    let families: BTreeSet<_> = device
        .slots
        .iter()
        .filter(|s| !s.filter_type_name.is_empty())
        .map(|s| s.filter_type_name.as_str())
        .collect();
    ensure!(
        families.len() == 1,
        "a single observed nonempty compatible filter-family string is required"
    );
    let family = *families
        .iter()
        .next()
        .context("missing compatible filter family")?;
    ensure!(
        family.len() <= 128 && !family.chars().any(char::is_control),
        "invalid observed filter-family metadata"
    );
    let display_name = format!("Open Room — {name}");
    let creation_time_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time precedes epoch")?
            .as_millis(),
    )?;
    ensure!(
        creation_time_ms <= i64::MAX as u64,
        "creation time exceeds int64 range"
    );
    let id = uuid::Uuid::new_v4();
    let bits = id.as_u128();
    let begin = slot_info(
        slot,
        &display_name,
        family,
        creation_time_ms,
        bits as u64,
        (bits >> 64) as u64,
    );
    let mut calls=vec![call("BeginFilterTransfer","SlotInfo",Some(begin),ResponseKind::UnaryVoid,
        "Offline candidate: independent name/UUID/time, observed compatible family, 2x2/one section. Optional unknown metadata omitted; acceptance unverified.")?];
    for variant in [0, 2, 4] {
        let r = aligned[0]
            .rates
            .iter()
            .find(|r| r.diagonal.variant == variant)
            .context("missing rate")?;
        let m = device
            .dual_rate_variants
            .iter()
            .find(|m| m.variant == variant as u32)
            .context("missing metadata")?;
        calls.push(call("SetCommonDRFIR","DRFIRCOMMON",Some(common(&r.common_lowpass,m)),ResponseKind::UnaryVoid,
            "Original 75-tap prototype; firmware complement derivation and common acceptance are unverified.")?);
    }
    // This bounded order is a proposed typed-transfer plan, not a replayed transaction.
    for variant in [0, 2, 4] {
        for (input, output) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let owner = output as usize;
            let r = aligned[owner]
                .rates
                .iter()
                .find(|r| r.diagonal.variant == variant)
                .context("missing speaker rate")?;
            let c = if input == output {
                &r.diagonal
            } else {
                &r.zero_cross
            };
            let m = device
                .dual_rate_variants
                .iter()
                .find(|m| m.variant == c.variant as u32)
                .context("missing metadata")?;
            calls.push(call(
                "SetTransferCell",
                "CellIndex",
                Some(cell_index(input, output)),
                ResponseKind::UnaryVoid,
                "Select section 0 and the explicit matrix cell.",
            )?);
            calls.push(call(
                "SetCellDRFIR",
                "DRFIR",
                Some(drfir(c, m)),
                ResponseKind::UnaryVoid,
                if input == output {
                    "Independent speaker correction; shared headroom remains in both branches."
                } else {
                    "Explicit zero off-diagonal low FIR and empty high FIR."
                },
            )?);
        }
    }
    for input in 0..2 {
        let mut gain_delay = Vec::new();
        scalar(&mut gain_delay, 1, input);
        calls.push(call("SetInputGainDelay","GainDelay",Some(gain_delay),ResponseKind::UnaryVoid,
            "Set the new filter's input gain and added input delay to zero; preserve calculated coefficient headroom.")?);
    }
    let mut commit = Vec::new();
    scalar(&mut commit, 1, slot);
    calls.push(call("EndFilterTransfer","Index",Some(commit),ResponseKind::ServerStreamingProgress,
        "Proposed commit: would require consuming Progress and successful terminal gRPC status; not executed.")?);
    ensure!(
        calls.len() <= MAX_CALLS,
        "transfer plan exceeds bounded call count"
    );
    let total_payload_bytes = calls
        .iter()
        .filter_map(|c| c.payload.as_ref())
        .map(Vec::len)
        .sum();
    ensure!(
        total_payload_bytes <= MAX_PAYLOAD,
        "aggregate coefficient plan exceeds 1 MiB"
    );
    Ok(TransferPackage {
        model_summaries: aligned.iter().enumerate().flat_map(|(speaker,design)|design.rates.iter().map(move |rate|FilterModelSummary {
            speaker:speaker as u32, sample_rate:rate.diagonal.sample_rate,
            filter_approximation_error_db:rate.model.max_shape_error_db,
            room_target_residual_db:rate.model.max_target_error_db,
        })).collect(),
        version:1,slot,name:display_name,filter_family:family.into(),creation_time_ms:creation_time_ms as i64,unique_id:id.to_string(),attenuation_db,
        hardware_validated:false,transfer_semantics_verified:false,ready_for_device_write:false,
        includes_activation:false,calls,total_payload_bytes,
        topology_assumptions:aligned.iter().map(|d|d.topology_assumption.clone()).collect(),
        blockers:vec![
            "Firmware acceptance of the independently constructed SlotInfo candidate, including omitted optional metadata and new UUID/time, is unverified.".into(),
            "Global common/cell ordering, pre-commit effects and rollback/cancellation remain hardware-unverified.".into(),
            "Firmware derivation of the high complement, resampling gain/phases and actual acoustic response are unverified.".into(),
            "No explicit slot activation request is included; activation caused by commit itself remains unverified. Speaker playback is held at the user's request, and this builder performs no device writes.".into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (nad_v2::NativeDevice, Vec<native_dsp::NativeDesign>) {
        let dual_rate_variants = (0..6)
            .map(|variant| nad_v2::NativeDualRateInfo {
                variant,
                sample_rate: RATES[variant as usize / 2] as f32,
                high_taps: if variant % 2 == 0 { 1024 } else { 0 },
                low_taps: 1024,
                sub_sampling_factor: 4,
                cascades: 2,
                high_delay_min: 0,
                high_delay_max: if variant % 2 == 0 {
                    [638, 759, 798][variant as usize / 2]
                } else {
                    0
                },
                individual_delays: false,
            })
            .collect();
        let device = nad_v2::NativeDevice {
            manufacturer: "NAD".into(),
            model: "M10".into(),
            name: "Test".into(),
            speakers: 2,
            microphones: 0,
            reported_slot_count: 5,
            slots: (0..5)
                .map(|index| nad_v2::NativeSlot {
                    index,
                    name: if index == 0 {
                        "Keep existing filter".into()
                    } else {
                        String::new()
                    },
                    filter_type_name: if index == 0 {
                        "Dirac Live".into()
                    } else {
                        String::new()
                    },
                    inputs: 2,
                    outputs: 2,
                    sections: 1,
                })
                .collect(),
            sections: vec![nad_v2::NativeSection {
                index: 0,
                inputs: 2,
                outputs: 2,
                disposition: 1,
            }],
            cells: [(0, 0), (0, 1), (1, 0), (1, 1)]
                .into_iter()
                .map(|(input, output)| nad_v2::NativeCell {
                    input,
                    output,
                    section: 0,
                    filter_type: 3,
                    variants: if input == output {
                        vec![0, 2, 4]
                    } else {
                        vec![1, 3, 5]
                    },
                })
                .collect(),
            dual_rate_variants,
            measurement: None,
            coverage_limited: false,
            notes: vec![],
        };
        let rates = RATES
            .into_iter()
            .enumerate()
            .map(|(i, sample_rate)| {
                let mut hi = vec![0.; 1024];
                hi[511] = 0.001;
                let mut lo = vec![0.; 1024];
                lo[64] = 0.001;
                let mut p = vec![0.; 75];
                p[37] = 1.;
                native_dsp::RateDesign {
                    common_lowpass: p,
                    diagonal: native_dsp::NativeCell {
                        variant: (i * 2) as i32,
                        sample_rate,
                        sub_sampling_factor: 4,
                        cascade_number: 2,
                        fir_hi: hi,
                        fir_lo: lo,
                        delay_hi: 513,
                        max_gain_db: 0.,
                    },
                    zero_cross: native_dsp::NativeCell {
                        variant: (i * 2 + 1) as i32,
                        sample_rate,
                        sub_sampling_factor: 4,
                        cascade_number: 2,
                        fir_hi: vec![],
                        fir_lo: vec![0.; 1024],
                        delay_hi: 0,
                        max_gain_db: 0.,
                    },
                    model: native_dsp::ModelCheck {
                        max_shape_error_db: 0.,
                        max_crossover_phase_error_deg: 0.,
                        worst_shape_hz: 100.,
                        requested_response: vec![],
                        achieved_response: vec![],
                        target_response: vec![],
                        predicted_room_response: vec![],
                        max_target_error_db: 0.,
                        worst_target_hz: 100.,
                        max_full_band_target_error_db: 0.,
                        max_alias_amplitude_db: -100.,
                        induced_peak_bound: 0.1,
                    },
                }
            })
            .collect();
        let design = native_dsp::NativeDesign {
            rates,
            attenuation_db: 0.,
            hardware_validated: false,
            topology_assumption: "Conditional model fixture, not firmware validation".into(),
            design_strategy: "fixture".into(),
            linear_phase_rejection: None,
        };
        (device, vec![design.clone(), design])
    }
    #[test]
    fn independent_packed_float_wire_vectors() {
        // IEEE754 1=3f800000 and -.5=bf000000, bytes sent little endian.
        assert_eq!(
            fir(&[1., -0.5]),
            vec![0x12, 8, 0, 0, 0x80, 0x3f, 0, 0, 0, 0xbf]
        );
        assert_eq!(cell_index(1, 0), vec![8, 1]);
        assert_eq!(cell_index(0, 0), Vec::<u8>::new());
        let (d, _) = fixture();
        assert_eq!(
            info(&d.dual_rate_variants[0]),
            vec![
                0x15, 0, 0, 0xfa, 0x46, 0x18, 0x80, 8, 0x20, 0x80, 8, 0x28, 4, 0x30, 2, 0x40, 0xfe,
                4
            ]
        );
        assert_eq!(
            slot_info(1, "X", "Y", 128, 1, 2),
            vec![
                8, 1, 0x12, 1, b'X', 0x1a, 1, b'Y', 0x20, 2, 0x28, 2, 0x48, 1, 0x50, 0x80, 1, 0x5a,
                4, 8, 1, 0x10, 2
            ]
        );
    }
    #[test]
    fn bounded_offline_package_never_activates_and_contains_begin_candidate() {
        let (d, s) = fixture();
        let p = build_package(&d, &s, 1, "Bass cut").unwrap();
        assert_eq!(p.calls.len(), 31);
        assert!(p.calls[0].payload.is_some());
        assert_eq!(p.filter_family, "Dirac Live");
        assert!(p.creation_time_ms > 1_000_000_000_000);
        let id = uuid::Uuid::parse_str(&p.unique_id).unwrap();
        assert_eq!(id.get_version_num(), 4);
        let q = build_package(&d, &s, 1, "Bass cut").unwrap();
        assert_ne!(p.unique_id, q.unique_id);
        assert!(!p.hardware_validated && !p.ready_for_device_write && !p.includes_activation);
        assert!(p
            .calls
            .iter()
            .all(|c| !c.method.ends_with("SetActiveSlot") && !c.method.ends_with("DeleteSlot")));
        assert!(p.total_payload_bytes < 1024 * 1024);
        assert!(matches!(
            p.calls.last().unwrap().response,
            ResponseKind::ServerStreamingProgress
        ));
        assert_eq!(p.calls.last().unwrap().payload, Some(vec![8, 1]));
    }
    #[test]
    fn rejects_named_and_internal_slots_and_incomplete_metadata() {
        let (mut d, s) = fixture();
        assert!(build_package(&d, &s, 0, "X").is_err());
        assert!(build_package(&d, &s, 3, "X").is_err());
        d.slots[1].name = "Occupied".into();
        assert!(build_package(&d, &s, 1, "X").is_err());
        d.slots[1].name.clear();
        d.coverage_limited = true;
        assert!(build_package(&d, &s, 1, "X").is_err());
    }
    #[test]
    fn rejects_absent_conflicting_family_and_malformed_names() {
        let (mut d, s) = fixture();
        d.slots[0].filter_type_name.clear();
        assert!(build_package(&d, &s, 1, "X").is_err());
        d.slots[0].filter_type_name = "Dirac Live".into();
        d.slots[2].filter_type_name = "Other".into();
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (d, s) = fixture();
        for name in ["", "\nX"] {
            assert!(build_package(&d, &s, 1, name).is_err());
        }
    }
    #[test]
    fn rejects_nans_dimensions_wrong_variants_and_actual_delay_limit() {
        let (d, mut s) = fixture();
        s[0].rates[0].diagonal.fir_hi[0] = f32::NAN;
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (d, mut s) = fixture();
        s[0].rates[0].diagonal.variant = 2;
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (d, mut s) = fixture();
        s[0].rates[0].diagonal.fir_lo.pop();
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (mut d, s) = fixture();
        d.dual_rate_variants[0].high_delay_max = 512;
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (d, mut s) = fixture();
        s[0].rates[0].model.max_crossover_phase_error_deg = 6.;
        assert!(build_package(&d, &s, 1, "X").is_err());
    }
    #[test]
    fn rejects_nonzero_cross_and_mismatched_common() {
        let (d, mut s) = fixture();
        s[0].rates[0].zero_cross.fir_lo[1] = 0.1;
        assert!(build_package(&d, &s, 1, "X").is_err());
        let (d, mut s) = fixture();
        s[1].rates[0].common_lowpass[37] = 0.9;
        assert!(build_package(&d, &s, 1, "X").is_err());
    }
    #[test]
    fn shared_headroom_adds_cuts_without_mutating_inputs() {
        let (d, mut s) = fixture();
        s[1].attenuation_db = 3.;
        let original = s[0].rates[0].diagonal.fir_hi[511];
        let p = build_package(&d, &s, 1, "X").unwrap();
        assert_eq!(p.attenuation_db, 3.);
        assert_eq!(s[0].rates[0].diagonal.fir_hi[511], original);
        let left = &p.calls[5].payload.as_ref().unwrap();
        let packed = (original as f64 * 10_f64.powf(-3. / 20.)) as f32;
        assert!(left.windows(4).any(|w| w == packed.to_le_bytes()));
    }
}
