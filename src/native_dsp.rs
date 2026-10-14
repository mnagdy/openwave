//! Original conditional M10 dual-rate DSP design. This module performs no I/O.
//! It models a complementary high path plus two M=4 down/up sampling stages.
//! Firmware deriving the same complement from SetCommonDRFIR is UNVERIFIED.
//! No proprietary coefficients or optimization algorithms are included.
use crate::dsp::{self, Point};
use anyhow::{ensure, Result};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

const M: usize = 4;
const PHASES: usize = 16;
const STAGE_TAPS: usize = 75;
const COMMON_DELAY: usize = 370;
const HIGH_ACTIVE_TAPS: usize = 1023;
const LOW_ACTIVE_TAPS: usize = 129;
const HIGH_DELAY: usize = 513;
/// Unity/reference delay. Minimum-phase correction adds frequency-dependent phase,
/// so this is not a claim of constant group delay for every corrected frequency.
pub const TOTAL_DELAY_SAMPLES: usize = 1394;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeCell {
    pub variant: i32,
    pub sample_rate: u32,
    pub sub_sampling_factor: u32,
    pub cascade_number: u32,
    pub fir_hi: Vec<f32>,
    pub fir_lo: Vec<f32>,
    pub delay_hi: u32,
    /// Conservative upper bound on the actual modeled gain, in dB (not linear).
    pub max_gain_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateDesign {
    pub common_lowpass: Vec<f32>,
    pub diagonal: NativeCell,
    pub zero_cross: NativeCell,
    pub model: ModelCheck,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCheck {
    pub max_shape_error_db: f64,
    /// Phase difference where both modeled complementary paths contribute materially.
    #[serde(default)]
    pub max_crossover_phase_error_deg: f64,
    #[serde(default)]
    pub worst_shape_hz: f64,
    /// Requested correction shape before the shared headroom attenuation.
    #[serde(default)]
    pub requested_response: Vec<Point>,
    /// Complete modeled correction with uniform headroom attenuation removed.
    /// Compare directly with requested_response; this is simulation, not acoustic data.
    #[serde(default)]
    pub achieved_response: Vec<Point>,
    /// Full user target and predicted room shape, before uniform attenuation.
    /// Only frequencies supported by the measured room response are included.
    #[serde(default)]
    pub target_response: Vec<Point>,
    #[serde(default)]
    pub predicted_room_response: Vec<Point>,
    /// Residual against the full room target within the correction curtains.
    /// May remain large because intentional boost/cut limits cannot fill deep nulls.
    #[serde(default)]
    pub max_target_error_db: f64,
    #[serde(default)]
    pub worst_target_hz: f64,
    #[serde(default)]
    pub max_full_band_target_error_db: f64,
    pub max_alias_amplitude_db: f64,
    /// Induced peak bound: |output| <= this * max |input| in the modeled system.
    pub induced_peak_bound: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeDesign {
    pub rates: Vec<RateDesign>,
    /// Shared attenuation, applied identically to both branches and all three rates.
    pub attenuation_db: f64,
    pub hardware_validated: bool,
    pub topology_assumption: String,
    #[serde(default)]
    pub design_strategy: String,
    #[serde(default)]
    pub linear_phase_rejection: Option<String>,
}

/// Match overall level across independently designed speakers by adding cuts.
/// Never increase the gain of a design whose peak bound already sets its headroom.
pub fn align_speaker_headroom(designs: &mut [NativeDesign]) -> Result<f64> {
    ensure!(!designs.is_empty(), "no native speaker designs");
    ensure!(
        designs
            .iter()
            .all(|d| d.attenuation_db.is_finite() && d.attenuation_db >= 0.),
        "invalid speaker attenuation"
    );
    let common = designs
        .iter()
        .map(|d| d.attenuation_db)
        .fold(0_f64, f64::max);
    for design in designs {
        let additional = common - design.attenuation_db;
        if additional > 0. {
            let scale = 10_f64.powf(-additional / 20.);
            for rate in &mut design.rates {
                for tap in rate
                    .diagonal
                    .fir_hi
                    .iter_mut()
                    .chain(&mut rate.diagonal.fir_lo)
                {
                    *tap = (*tap as f64 * scale) as f32;
                }
                rate.model.induced_peak_bound =
                    induced_peak_bound(&all_phase_impulses(&rate.diagonal, &rate.common_lowpass)?);
                ensure!(
                    rate.model.induced_peak_bound <= 1.000001,
                    "speaker headroom alignment failed"
                );
                rate.model.max_alias_amplitude_db -= additional;
            }
        }
        design.attenuation_db = common;
    }
    Ok(common)
}

/// Original bass-only approximation with a measured-shape validation gate.
/// The high curtain is limited to 500 Hz so all three low-rate paths can represent it.
/// Produces all 0/2/4 diagonal variants and explicit silent 1/3/5 cross variants.
pub fn design(
    measured: &[Point],
    target: &[Point],
    low_hz: f64,
    high_hz: f64,
    max_boost_db: f64,
) -> Result<NativeDesign> {
    dsp::interpolate(measured, 1000.)?;
    dsp::interpolate(target, 1000.)?;
    ensure!(
        low_hz.is_finite()
            && high_hz.is_finite()
            && low_hz >= 20.
            && high_hz > low_hz
            && high_hz <= 500.,
        "native modeled correction must stay within 20–500 Hz"
    );
    ensure!(
        low_hz >= measured[0].hz && high_hz <= measured[measured.len() - 1].hz,
        "native correction curtains exceed measured support"
    );
    ensure!(
        max_boost_db.is_finite() && (0. ..=6.).contains(&max_boost_db),
        "native maximum boost must be 0–6 dB"
    );
    match design_impl(measured, target, low_hz, high_hz, max_boost_db, false) {
        Ok(design) => Ok(design),
        Err(linear_error) => {
            let mut design = design_impl(measured, target, low_hz, high_hz, max_boost_db, true)?;
            design.linear_phase_rejection = Some(linear_error.to_string());
            Ok(design)
        }
    }
}

fn design_impl(
    measured: &[Point],
    target: &[Point],
    low_hz: f64,
    high_hz: f64,
    max_boost_db: f64,
    minimum_phase: bool,
) -> Result<NativeDesign> {
    let common = original_lowpass();
    let mut rates = Vec::new();
    let mut maximum_peak_bound = 1_f64;
    for (i, rate) in [32000, 44100, 48000].into_iter().enumerate() {
        let (hi, lo) = if minimum_phase {
            matched_minimum_phase(
                measured,
                target,
                rate as f64,
                (low_hz, high_hz),
                max_boost_db,
            )
        } else {
            (
                original_fir(
                    measured,
                    target,
                    rate as f64,
                    HIGH_ACTIVE_TAPS,
                    (low_hz, high_hz),
                    max_boost_db,
                ),
                original_fir(
                    measured,
                    target,
                    rate as f64 / PHASES as f64,
                    LOW_ACTIVE_TAPS,
                    (low_hz, high_hz),
                    max_boost_db,
                ),
            )
        };
        let mut high = hi.iter().map(|&h| h as f32).collect::<Vec<_>>();
        high.resize(1024, 0.);
        let mut low = lo.iter().map(|&h| h as f32).collect::<Vec<_>>();
        low.resize(1024, 0.);
        let cell = NativeCell {
            variant: (i * 2) as i32,
            sample_rate: rate,
            sub_sampling_factor: M as u32,
            cascade_number: 2,
            fir_hi: high,
            fir_lo: low,
            delay_hi: HIGH_DELAY as u32,
            max_gain_db: 0.,
        };
        let impulses = all_phase_impulses(&cell, &common)?;
        let bound = induced_peak_bound(&impulses);
        maximum_peak_bound = maximum_peak_bound.max(bound);
        let mut check = check_model(
            &impulses,
            measured,
            target,
            rate,
            (low_hz, high_hz),
            max_boost_db,
            0.,
        );
        check.max_crossover_phase_error_deg = crossover_phase_error(&cell, &common);
        ensure!(check.max_shape_error_db <= 1.,
            "native {} Hz modeled shape error {:.2} dB at {:.1} Hz exceeds 1 dB; this correction exceeds the available FIR resolution", rate, check.max_shape_error_db,check.worst_shape_hz);
        ensure!(
            check.max_alias_amplitude_db <= -70.,
            "native {} Hz model alias amplitude {:.1} dB exceeds -70 dB",
            rate,
            check.max_alias_amplitude_db
        );
        ensure!(
            check.max_crossover_phase_error_deg <= 5.,
            "native {} Hz crossover phase error {:.2}° exceeds 5°",
            rate,
            check.max_crossover_phase_error_deg
        );
        let cross = NativeCell {
            variant: cell.variant + 1,
            sample_rate: rate,
            sub_sampling_factor: 4,
            cascade_number: 2,
            fir_hi: vec![],
            fir_lo: vec![0.; 1024],
            delay_hi: 0,
            max_gain_db: 0.,
        };
        rates.push(RateDesign {
            common_lowpass: common.clone(),
            diagonal: cell,
            zero_cross: cross,
            model: ModelCheck {
                induced_peak_bound: bound,
                ..check
            },
        });
    }
    // A time-domain peak bound avoids independently normalizing the two branches,
    // which would break their complement and alter the crossover response.
    // Reserve a tiny conversion margin for the final float32 coefficients.
    maximum_peak_bound *= 1.000001;
    let scale = 1. / maximum_peak_bound;
    let attenuation_db = 20. * maximum_peak_bound.log10();
    for rate in &mut rates {
        for tap in rate
            .diagonal
            .fir_hi
            .iter_mut()
            .chain(&mut rate.diagonal.fir_lo)
        {
            *tap = (*tap as f64 * scale) as f32;
        }
        let impulses = all_phase_impulses(&rate.diagonal, &rate.common_lowpass)?;
        let final_bound = induced_peak_bound(&impulses);
        ensure!(
            final_bound <= 1.000001,
            "joint native gain normalization failed"
        );
        let mut check = check_model(
            &impulses,
            measured,
            target,
            rate.diagonal.sample_rate,
            (low_hz, high_hz),
            max_boost_db,
            attenuation_db,
        );
        check.max_crossover_phase_error_deg =
            crossover_phase_error(&rate.diagonal, &rate.common_lowpass);
        ensure!(
            check.max_shape_error_db <= 1. && check.max_crossover_phase_error_deg <= 5.,
            "normalized native response exceeds accuracy/phase gate"
        );
        rate.model = ModelCheck {
            induced_peak_bound: final_bound,
            ..check
        };
    }
    Ok(NativeDesign {rates,attenuation_db,hardware_validated:false,
        design_strategy: if minimum_phase {"matched_minimum_phase_1023_960"} else {"linear_phase_1023_129"}.into(),linear_phase_rejection:None,
        topology_assumption:"Firmware must construct the same complementary high path from the supplied original 75-tap common low-pass, use phase-zero M=4 decimation twice with gain 4 at each decimation, and apply delay_hi in full-rate samples. These firmware behaviors remain unverified.".into()})
}

/// Original real-cepstrum minimum-phase low FIR. The matched high spectrum uses
/// this SAME truncated low transfer at physical frequencies, avoiding the phase
/// mismatch of independently minimizing the two branches at different rates.
fn matched_minimum_phase(
    measured: &[Point],
    target: &[Point],
    rate: f64,
    curtains: (f64, f64),
    boost: f64,
) -> (Vec<f64>, Vec<f64>) {
    let n = 65536;
    let low_rate = rate / PHASES as f64;
    let mut planner = FftPlanner::new();
    let forward = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let mut spectrum = vec![Complex::new(0., 0.); n];
    for i in 0..=n / 2 {
        let hz = i as f64 * low_rate / n as f64;
        spectrum[i].re =
            desired(measured, target, hz, curtains.0, curtains.1, boost) * 10_f64.ln() / 20.;
        if i > 0 && i < n / 2 {
            spectrum[n - i] = spectrum[i];
        }
    }
    inverse.process(&mut spectrum);
    for (i, c) in spectrum.iter_mut().enumerate() {
        c.re = if i == 0 || i == n / 2 {
            c.re / n as f64
        } else if i < n / 2 {
            2. * c.re / n as f64
        } else {
            0.
        };
        c.im = 0.;
    }
    forward.process(&mut spectrum);
    for value in &mut spectrum {
        *value = Complex::from_polar(value.re.exp(), value.im);
    }
    inverse.process(&mut spectrum);
    let mut active_low = spectrum[..960]
        .iter()
        .map(|z| z.re / n as f64)
        .collect::<Vec<_>>();
    for (i, v) in active_low[896..].iter_mut().enumerate() {
        *v *= 0.5 + 0.5 * (PI * i as f64 / 63.).cos();
    }
    spectrum.fill(Complex::new(0., 0.));
    for (z, &tap) in spectrum.iter_mut().zip(&active_low) {
        z.re = tap;
    }
    forward.process(&mut spectrum);
    let mut high_spectrum = vec![Complex::new(0., 0.); n];
    for i in 0..=n / 2 {
        // Both FFTs have the same bin count: high-rate bin i maps exactly to
        // low-rate bin 16*i up to the low-rate Nyquist frequency.
        let low_response = if i <= n / (2 * PHASES) {
            spectrum[i * PHASES]
        } else {
            Complex::new(1., 0.)
        };
        high_spectrum[i] =
            low_response * Complex::from_polar(1., -2. * PI * i as f64 * 511. / n as f64);
        if i > 0 && i < n / 2 {
            high_spectrum[n - i] = high_spectrum[i].conj();
        }
    }
    high_spectrum[n / 2].im = 0.;
    inverse.process(&mut high_spectrum);
    let hi = (0..HIGH_ACTIVE_TAPS)
        .map(|i| high_spectrum[i].re / n as f64 * kaiser(i, HIGH_ACTIVE_TAPS, 6.))
        .collect();
    let mut lo = vec![0.; 64];
    lo.extend(active_low);
    (hi, lo)
}

fn at(points: &[Point], frequency: f64) -> f64 {
    let i = points.partition_point(|p| p.hz < frequency);
    if i == 0 {
        return points[0].db;
    }
    if i == points.len() {
        return points[i - 1].db;
    }
    let (a, b) = (&points[i - 1], &points[i]);
    a.db + (b.db - a.db) * (frequency / a.hz).ln() / (b.hz / a.hz).ln()
}
fn desired(measured: &[Point], target: &[Point], hz: f64, low: f64, high: f64, boost: f64) -> f64 {
    if hz <= low || hz >= high {
        return 0.;
    }
    let edge = (hz / low).log2().min((high / hz).log2());
    let curtain = 0.5 - 0.5 * (PI * (edge * 6.).clamp(0., 1.)).cos();
    (at(target, hz) - at(measured, hz)).clamp(-24., boost) * curtain
}
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.;
    let mut term = 1.;
    for k in 1..80 {
        term *= x * x / (4. * (k * k) as f64);
        sum += term;
        if term < sum * 1e-15 {
            break;
        }
    }
    sum
}
fn kaiser(position: usize, length: usize, beta: f64) -> f64 {
    let x = 2. * position as f64 / (length - 1) as f64 - 1.;
    bessel_i0(beta * (1. - x * x).max(0.).sqrt()) / bessel_i0(beta)
}
fn original_lowpass() -> Vec<f32> {
    // Original Kaiser-sinc: 0.08 cycles/stage sample, unit DC. Never reads a client table.
    let mut p = (0..STAGE_TAPS)
        .map(|i| {
            let x = i as f64 - 37.;
            let sinc = if x == 0. {
                0.16
            } else {
                (2. * PI * 0.08 * x).sin() / (PI * x)
            };
            sinc * kaiser(i, STAGE_TAPS, 9.)
        })
        .collect::<Vec<_>>();
    let sum = p.iter().sum::<f64>();
    for v in &mut p {
        *v /= sum;
    }
    p.into_iter().map(|v| v as f32).collect()
}
fn original_fir(
    measured: &[Point],
    target: &[Point],
    rate: f64,
    taps: usize,
    curtains: (f64, f64),
    boost: f64,
) -> Vec<f64> {
    let (low, high) = curtains;
    let n = 65536;
    let delay = (taps - 1) as f64 / 2.;
    let mut s = vec![Complex::new(0., 0.); n];
    for i in 0..=n / 2 {
        let hz = i as f64 * rate / n as f64;
        s[i] = Complex::from_polar(
            10_f64.powf(desired(measured, target, hz, low, high, boost) / 20.),
            -2. * PI * i as f64 * delay / n as f64,
        );
        if i > 0 && i < n / 2 {
            s[n - i] = s[i].conj();
        }
    }
    s[n / 2].im = 0.;
    FftPlanner::new().plan_fft_inverse(n).process(&mut s);
    (0..taps)
        .map(|i| s[i].re / n as f64 * kaiser(i, taps, 6.))
        .collect()
}
fn active(values: &[f32]) -> Vec<f64> {
    let end = values.iter().rposition(|v| *v != 0.).map_or(1, |i| i + 1);
    values.iter().take(end).map(|&v| v as f64).collect()
}
fn convolve(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut out = vec![0.; a.len() + b.len() - 1];
    for (i, &x) in a.iter().enumerate() {
        if x != 0. {
            for (j, &y) in b.iter().enumerate() {
                out[i + j] += x * y;
            }
        }
    }
    out
}
fn low_path(signal: &[f64], lo: &[f64], p: &[f64]) -> Vec<f64> {
    let mut x = signal.to_vec();
    for _ in 0..2 {
        x = convolve(&x, p)
            .into_iter()
            .step_by(M)
            .map(|v| v * M as f64)
            .collect();
    }
    x = convolve(&x, lo);
    for _ in 0..2 {
        let mut up = vec![0.; (x.len() - 1) * M + 1];
        for (i, &v) in x.iter().enumerate() {
            up[i * M] = v;
        }
        x = convolve(&up, p);
    }
    x
}
/// Simulate the complete conditional topology, including multirate alias terms.
pub fn simulate(cell: &NativeCell, common_lowpass: &[f32], signal: &[f64]) -> Result<Vec<f64>> {
    ensure!(
        !signal.is_empty() && signal.iter().all(|x| x.is_finite()),
        "invalid simulation signal"
    );
    ensure!((0..=5).contains(&cell.variant), "unknown native variant");
    let (expected_rate, max_delay) =
        [(32000, 638), (44100, 759), (48000, 798)][cell.variant as usize / 2];
    ensure!(
        cell.sample_rate == expected_rate
            && cell.delay_hi <= max_delay
            && cell.max_gain_db.is_finite(),
        "invalid native rate/delay/gain metadata"
    );
    ensure!(
        cell.variant % 2 == 0 || cell.delay_hi == 0,
        "cross cells do not support high delay"
    );
    ensure!(
        cell.sub_sampling_factor == 4 && cell.cascade_number == 2 && common_lowpass.len() == 75,
        "unsupported modeled topology"
    );
    ensure!(
        common_lowpass
            .iter()
            .chain(&cell.fir_hi)
            .chain(&cell.fir_lo)
            .all(|x| x.is_finite()),
        "invalid modeled coefficients"
    );
    ensure!(
        cell.fir_lo.len() == 1024
            && ((cell.variant % 2 == 0 && cell.fir_hi.len() == 1024)
                || (cell.variant % 2 == 1 && cell.fir_hi.is_empty())),
        "invalid modeled tap dimensions"
    );
    let p = common_lowpass.iter().map(|&x| x as f64).collect::<Vec<_>>();
    let low = low_path(signal, &active(&cell.fir_lo), &p);
    let mut complement = low_path(&[1.], &[1.], &p);
    complement.resize(2 * COMMON_DELAY + 1, 0.);
    for v in &mut complement {
        *v = -*v;
    }
    complement[COMMON_DELAY] += 1.;
    let hi = if cell.fir_hi.is_empty() {
        vec![0.]
    } else {
        convolve(&convolve(signal, &complement), &active(&cell.fir_hi))
    };
    let mut output = vec![0.; low.len().max(hi.len() + cell.delay_hi as usize)];
    for (i, v) in low.into_iter().enumerate() {
        output[i] += v;
    }
    for (i, v) in hi.into_iter().enumerate() {
        output[i + cell.delay_hi as usize] += v;
    }
    Ok(output)
}
fn all_phase_impulses(cell: &NativeCell, p: &[f32]) -> Result<Vec<Vec<f64>>> {
    (0..PHASES)
        .map(|r| {
            let mut signal = vec![0.; r + 1];
            signal[r] = 1.;
            simulate(cell, p, &signal)
        })
        .collect()
}
fn induced_peak_bound(impulses: &[Vec<f64>]) -> f64 {
    (0..PHASES)
        .map(|phase| {
            impulses
                .iter()
                .map(|h| {
                    h.iter()
                        .skip(phase)
                        .step_by(PHASES)
                        .map(|v| v.abs())
                        .sum::<f64>()
                })
                .sum()
        })
        .fold(0., f64::max)
}

fn crossover_phase_error(cell: &NativeCell, common: &[f32]) -> f64 {
    let p = common.iter().map(|&v| v as f64).collect::<Vec<_>>();
    let baseline = low_path(&[1.], &[1.], &p);
    let rate = cell.sample_rate as f64;
    let mut maximum = 0_f64;
    for i in 1..=96 {
        let hz = rate / PHASES as f64 * 0.49 * i as f64 / 96.;
        let omega = 2. * PI * hz / rate;
        let low_weight = baseline
            .iter()
            .enumerate()
            .map(|(n, &v)| Complex::from_polar(v, -omega * n as f64))
            .sum::<Complex<f64>>();
        let high_weight = Complex::from_polar(1., -omega * COMMON_DELAY as f64) - low_weight;
        if low_weight.norm() < 0.1 || high_weight.norm() < 0.1 {
            continue;
        }
        let hi = cell
            .fir_hi
            .iter()
            .enumerate()
            .map(|(n, &v)| {
                Complex::from_polar(v as f64, -omega * (n as f64 + cell.delay_hi as f64))
            })
            .sum::<Complex<f64>>();
        let lo = cell
            .fir_lo
            .iter()
            .enumerate()
            .map(|(n, &v)| Complex::from_polar(v as f64, -omega * PHASES as f64 * n as f64))
            .sum::<Complex<f64>>();
        maximum = maximum.max((hi * lo.conj()).arg().to_degrees().abs());
    }
    maximum
}
fn check_model(
    impulses: &[Vec<f64>],
    measured: &[Point],
    target: &[Point],
    rate: u32,
    curtains: (f64, f64),
    boost: f64,
    attenuation: f64,
) -> ModelCheck {
    let (low, high) = curtains;
    let mut shape = 0_f64;
    let mut worst_shape_hz = 0.;
    let mut alias = 1e-20_f64;
    let mut requested_response = Vec::new();
    let mut achieved_response = Vec::new();
    let mut target_response = Vec::new();
    let mut predicted_room_response = Vec::new();
    let mut max_target_error_db = 0_f64;
    let mut max_full_band_target_error_db = 0_f64;
    let mut worst_target_hz = 0.;
    let mut frequencies = (0..=120)
        .map(|i| 20. * ((rate as f64 * 0.475) / 20.).powf(i as f64 / 120.))
        .collect::<Vec<_>>();
    frequencies.extend(
        measured
            .iter()
            .chain(target)
            .filter(|p| p.hz >= 20. && p.hz <= high)
            .map(|p| p.hz),
    );
    frequencies.sort_by(f64::total_cmp);
    frequencies.dedup();
    for hz in frequencies {
        let phase_values = impulses
            .iter()
            .enumerate()
            .map(|(r, h)| {
                h.iter()
                    .enumerate()
                    .map(|(n, &v)| {
                        Complex::from_polar(v, -2. * PI * hz * (n as f64 - r as f64) / rate as f64)
                    })
                    .sum::<Complex<f64>>()
            })
            .collect::<Vec<_>>();
        let main = phase_values.iter().sum::<Complex<f64>>() / PHASES as f64;
        let image = (phase_values
            .iter()
            .map(|v| (*v - main).norm_sqr())
            .sum::<f64>()
            / PHASES as f64)
            .sqrt();
        let requested = desired(measured, target, hz, low, high, boost);
        let achieved = 20. * main.norm().max(1e-20).log10() + attenuation;
        let error = (achieved - requested).abs();
        if error > shape {
            shape = error;
            worst_shape_hz = hz;
        }
        requested_response.push(Point { hz, db: requested });
        achieved_response.push(Point { hz, db: achieved });
        if hz >= measured[0].hz && hz <= measured[measured.len() - 1].hz {
            let target_db = at(target, hz);
            let predicted_db = at(measured, hz) + achieved;
            let residual = (predicted_db - target_db).abs();
            max_full_band_target_error_db = max_full_band_target_error_db.max(residual);
            if hz >= low && hz <= high && residual > max_target_error_db {
                max_target_error_db = residual;
                worst_target_hz = hz;
            }
            target_response.push(Point { hz, db: target_db });
            predicted_room_response.push(Point {
                hz,
                db: predicted_db,
            });
        }
        alias = alias.max(image);
    }
    ModelCheck {
        max_shape_error_db: shape,
        max_crossover_phase_error_deg: 0.,
        worst_shape_hz,
        requested_response,
        achieved_response,
        target_response,
        predicted_room_response,
        max_target_error_db,
        worst_target_hz,
        max_full_band_target_error_db,
        max_alias_amplitude_db: 20. * alias.log10(),
        induced_peak_bound: induced_peak_bound(impulses),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat(db: f64) -> Vec<Point> {
        vec![Point { hz: 20., db }, Point { hz: 20000., db }]
    }
    fn bump(center: f64, width: f64, gain: f64) -> Vec<Point> {
        (0..=240)
            .map(|i| {
                let hz = 20. * 1000_f64.powf(i as f64 / 240.);
                Point {
                    hz,
                    db: gain * (-((hz / center).log2() / width).powi(2)).exp(),
                }
            })
            .collect()
    }
    #[test]
    fn unity_reconstructs_every_phase_all_rates_and_cross_is_silent() {
        let d = design(&flat(0.), &flat(0.), 30., 500., 0.).unwrap();
        assert!(!d.hardware_validated);
        assert!(d.attenuation_db < 0.001);
        let expected = 10_f64.powf(-d.attenuation_db / 20.);
        for (i, r) in d.rates.iter().enumerate() {
            assert_eq!(r.diagonal.variant, (i * 2) as i32);
            assert_eq!(r.zero_cross.variant, (i * 2 + 1) as i32);
            assert_eq!(r.diagonal.fir_hi.len(), 1024);
            assert_eq!(r.diagonal.fir_lo.len(), 1024);
            assert_eq!(r.diagonal.delay_hi, 513);
            for phase in 0..16 {
                let mut x = vec![0.; phase + 1];
                x[phase] = 1.;
                let y = simulate(&r.diagonal, &r.common_lowpass, &x).unwrap();
                assert!(
                    (y[TOTAL_DELAY_SAMPLES + phase] - expected).abs() < 2e-6,
                    "rate {}, phase {}, actual {}, expected {}",
                    r.diagonal.sample_rate,
                    phase,
                    y[TOTAL_DELAY_SAMPLES + phase],
                    expected
                );
                assert!(y
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != TOTAL_DELAY_SAMPLES + phase)
                    .all(|(_, v)| v.abs() < 2e-6));
            }
            assert!(simulate(&r.zero_cross, &r.common_lowpass, &[1., -0.2, 0.7])
                .unwrap()
                .iter()
                .all(|v| *v == 0.));
        }
    }
    #[test]
    fn broad_bass_cut_matches_complete_response_with_shared_headroom() {
        let d = design(&bump(200., 0.6, 6.), &flat(0.), 30., 500., 0.).unwrap();
        assert!(d.attenuation_db > 0.);
        for r in d.rates {
            assert!(r.model.max_shape_error_db < 1.);
            assert!(r.model.max_alias_amplitude_db < -70.);
            assert!(r.model.induced_peak_bound <= 1.000001);
            assert_eq!(r.diagonal.max_gain_db, 0.);
        }
    }
    #[test]
    fn broad_boost_is_bounded_by_joint_time_domain_headroom() {
        let d = design(&bump(200., 0.6, -6.), &flat(0.), 30., 500., 6.).unwrap();
        assert!(d.attenuation_db >= 6.);
        for r in d.rates {
            assert!(r.model.max_shape_error_db < 1.);
            assert!(r.model.induced_peak_bound <= 1.000001);
            assert_eq!(r.diagonal.max_gain_db, 0.);
        }
    }
    #[test]
    fn longer_low_branch_preserves_narrow_request_and_reports_achieved_response() {
        let measured = bump(60., 0.08, 12.);
        assert!(design_impl(&measured, &flat(0.), 30., 500., 0., false).is_err());
        let d = design(&measured, &flat(0.), 30., 500., 0.).unwrap();
        assert_eq!(d.design_strategy, "matched_minimum_phase_1023_960");
        assert!(d.linear_phase_rejection.is_some());
        assert!(!d.hardware_validated);
        for r in d.rates {
            assert_eq!(r.diagonal.fir_lo.len(), 1024);
            assert_eq!(&r.diagonal.fir_lo[..64], &[0.; 64]);
            assert!(r.model.max_shape_error_db <= 1.);
            assert!(r.model.max_crossover_phase_error_deg <= 5.);
            let error = r
                .model
                .requested_response
                .iter()
                .zip(&r.model.achieved_response)
                .map(|(a, b)| {
                    assert_eq!(a.hz, b.hz);
                    (a.db - b.db).abs()
                })
                .fold(0_f64, f64::max);
            assert!((error - r.model.max_shape_error_db).abs() < 1e-10);
        }
    }
    #[test]
    fn unresolved_sub_hertz_feature_and_invalid_curtains_are_rejected() {
        let measured = vec![
            Point { hz: 20., db: 0. },
            Point { hz: 59.9, db: 0. },
            Point { hz: 60., db: 24. },
            Point { hz: 60.1, db: 0. },
            Point { hz: 20000., db: 0. },
        ];
        assert!(design(&measured, &flat(0.), 30., 500., 0.).is_err());
        assert!(design(&flat(0.), &flat(0.), 30., 1000., 0.).is_err());
    }
    #[test]
    fn high_only_does_not_pass_bass() {
        let mut d = design(&flat(0.), &flat(0.), 30., 500., 0.).unwrap();
        let r = &mut d.rates[2];
        r.diagonal.fir_lo.fill(0.);
        let y = simulate(&r.diagonal, &r.common_lowpass, &[1.]).unwrap();
        assert!(y.iter().sum::<f64>().abs() < 1e-4);
    }

    #[test]
    fn crossover_phase_check_detects_misaligned_branches() {
        let d = design(&flat(0.), &flat(0.), 30., 500., 0.).unwrap();
        let r = &d.rates[2];
        assert!(crossover_phase_error(&r.diagonal, &r.common_lowpass) < 0.001);
        let mut cell = r.diagonal.clone();
        cell.delay_hi += 16;
        assert!(crossover_phase_error(&cell, &r.common_lowpass) > 5.);
    }

    #[test]
    fn reports_full_target_residual_separately_from_filter_accuracy() {
        let d = design(&flat(-10.), &flat(0.), 30., 500., 3.).unwrap();
        for r in d.rates {
            assert!(r.model.max_shape_error_db <= 1.);
            assert!(r.model.max_target_error_db > 6.);
            let predicted = r
                .model
                .predicted_room_response
                .iter()
                .find(|p| p.hz > 80. && p.hz < 200.)
                .unwrap();
            assert!((predicted.db + 7.).abs() < 1.);
            assert!(r.model.target_response.iter().all(|p| p.db == 0.));
        }
    }

    #[test]
    fn speaker_levels_are_aligned_by_extra_attenuation() {
        let a = design(&flat(0.), &flat(0.), 30., 500., 0.).unwrap();
        let mut b = a.clone();
        b.attenuation_db += 20_f64 * 2_f64.log10();
        for rate in &mut b.rates {
            for tap in rate
                .diagonal
                .fir_hi
                .iter_mut()
                .chain(&mut rate.diagonal.fir_lo)
            {
                *tap *= 0.5;
            }
        }
        let mut designs = [a, b];
        align_speaker_headroom(&mut designs).unwrap();
        assert_eq!(designs[0].attenuation_db, designs[1].attenuation_db);
        for (left, right) in designs[0].rates.iter().zip(&designs[1].rates) {
            for (a, b) in left.diagonal.fir_hi.iter().zip(&right.diagonal.fir_hi) {
                assert!((a - b).abs() < 1e-7);
            }
            for (a, b) in left.diagonal.fir_lo.iter().zip(&right.diagonal.fir_lo) {
                assert!((a - b).abs() < 1e-7);
            }
            assert!(left.model.induced_peak_bound < 0.500001);
        }
    }
}
