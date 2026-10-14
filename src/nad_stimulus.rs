//! Independent finite native sweep workflow. No operation runs on construction.
//! Hardware playback/cancellation remain unverified; callers must opt into playback.
use crate::nad_v2::{self, NativeDevice, NativeLogSweep, NativeOperationMode, RawCodec};
use anyhow::{anyhow, bail, ensure, Context, Result};
use bytes::Bytes;
use serde::Serialize;
use std::{
    f64::consts::PI,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tonic::{
    transport::{Channel, Endpoint},
    Request,
};

#[derive(Clone, Debug, Serialize)]
pub struct MeasurementPlan {
    pub speaker: u32,
    pub gain_db: f64,
    pub sweep: NativeLogSweep,
    pub sequence_seconds: f64,
    pub reference_note: String,
}

/// Build a plan from read-only metadata. This function sends nothing.
pub fn plan(device: &NativeDevice, speaker: u32) -> Result<MeasurementPlan> {
    ensure!(
        device.manufacturer == "NAD" && device.model == "M10" && device.speakers == 2,
        "Native measurement currently supports the two-speaker NAD M10 profile"
    );
    ensure!(speaker < device.speakers, "Speaker index is out of range");
    let m = device
        .measurement
        .as_ref()
        .context("Read amplifier measurement capabilities first")?;
    ensure!(
        !m.operation_mode.measure_enabled && !m.operation_mode.streaming_enabled,
        "Amplifier is already in a measurement or streaming session"
    );
    ensure!(
        m.stimuli
            .iter()
            .any(|s| s.kind == 3 && s.variant == 0 && !s.supports_override),
        "Amplifier does not advertise the expected built-in full-range sweep"
    );
    let sweep = m
        .log_sweeps
        .iter()
        .find(|s| s.variant == 0)
        .context("Full-range sweep metadata missing")?
        .clone();
    ensure!(
        sweep.sample_rate == 48000.
            && sweep.start_hz == 10.
            && sweep.end_hz == 24000.
            && sweep.samples == 240000
            && sweep.silence_samples == 24000
            && sweep.periods == 1
            && sweep.fade_out_tail_hz == 300.,
        "Native sweep differs from the recovered M10 waveform profile"
    );
    ensure!(
        m.gain_min_db <= -15.
            && m.gain_max_db >= -15.
            && m.gain_step_db > 0.
            && ((-15. - m.gain_min_db) / m.gain_step_db).fract().abs() < 1e-9,
        "Amplifier does not support the bounded -15 dB stimulus setting"
    );
    Ok(MeasurementPlan { speaker, gain_db: -15., sequence_seconds: 5.5, sweep,
        reference_note: "Independent reconstruction of the recovered sweep formula; device waveform and acoustic synchronization still require validation.".into() })
}

/// Original mathematical reconstruction; no proprietary waveform is embedded.
/// Silence follows the sweep. Absolute SPL and bit-exact firmware equivalence are not asserted.
pub fn reference(plan: &MeasurementPlan) -> Result<Vec<f32>> {
    let s = &plan.sweep;
    ensure!(
        s.samples == 240000
            && s.sample_rate == 48000.
            && s.start_hz == 10.
            && s.end_hz == 24000.
            && s.silence_samples == 24000
            && s.periods == 1
            && s.fade_out_tail_hz == 300.,
        "Unsupported native reference dimensions"
    );
    ensure!(
        plan.gain_db.is_finite() && plan.gain_db <= -15. && plan.gain_db >= -120.,
        "Invalid reference gain"
    );
    let n = s.samples as usize;
    let ratio = (s.end_hz as f64 / s.start_hz as f64).ln();
    let coefficient = 2. * PI * s.start_hz as f64 * n as f64 / (s.sample_rate as f64 * ratio);
    let growth = (ratio / (n - 1) as f64).exp();
    let fade_start = (n as f64 * ((s.end_hz as f64 - s.fade_out_tail_hz) / s.start_hz as f64).ln()
        / ratio)
        .round() as usize;
    let fade_count = n - fade_start;
    let scale = 10_f64.powf(plan.gain_db / 20.);
    let mut phase_factor = 1.;
    let mut output = Vec::with_capacity(n + s.silence_samples as usize);
    for i in 0..n {
        let envelope = if i >= fade_start {
            0.5 * (1.
                + (PI * (2 * (i - fade_start) + 1) as f64 / (2 * fade_count + 1) as f64).cos())
        } else {
            1.
        };
        output.push((scale * envelope * (coefficient * (phase_factor - 1.)).sin()) as f32);
        phase_factor *= growth;
    }
    output.resize(n + s.silence_samples as usize, 0.);
    Ok(output)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    ReadMode,
    ReadGain,
    SetMode,
    SetGain,
    Prepare,
}
impl Method {
    fn path(self) -> &'static str {
        match self {
            Self::ReadMode => "/dio.endpoint.EndpointDevice/GetOperationMode",
            Self::ReadGain => "/dio.endpoint.EndpointDevice/GetStimuliGain",
            Self::SetMode => "/dio.endpoint.EndpointDevice/SetOperationMode",
            Self::SetGain => "/dio.endpoint.EndpointDevice/SetStimuliGain",
            Self::Prepare => "/dio.endpoint.EndpointDevice/PrepareStimuli",
        }
    }
}
trait Control {
    async fn call(
        &mut self,
        method: Method,
        payload: Vec<u8>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>>;
    async fn play(&mut self, cancel: &AtomicBool) -> Result<()>;
}
struct Rpc {
    grpc: tonic::client::Grpc<Channel>,
}
impl Control for Rpc {
    async fn call(
        &mut self,
        method: Method,
        payload: Vec<u8>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(4), async {
            self.grpc.ready().await?;
            if let Some(cancel) = cancel {
                check_cancel(cancel)?;
            }
            let mut req = Request::new(Bytes::from(payload));
            req.set_timeout(Duration::from_secs(3));
            let r = self
                .grpc
                .unary(
                    req,
                    tonic::codegen::http::uri::PathAndQuery::from_static(method.path()),
                    RawCodec,
                )
                .await?;
            nad_v2::require_success_status(
                r.metadata()
                    .get("grpc-status")
                    .map(|s| s.to_str())
                    .transpose()?,
            )?;
            Ok(r.into_inner().to_vec())
        })
        .await
        .map_err(|_| anyhow!("Native control request timed out: {}", method.path()))?
    }
    async fn play(&mut self, cancel: &AtomicBool) -> Result<()> {
        let r = cancellable(
            async {
                self.grpc.ready().await?;
                check_cancel(cancel)?;
                let mut req = Request::new(Bytes::new());
                req.set_timeout(Duration::from_secs(8));
                Ok(self
                    .grpc
                    .server_streaming(
                        req,
                        tonic::codegen::http::uri::PathAndQuery::from_static(
                            "/dio.endpoint.EndpointDevice/StartStimuli",
                        ),
                        RawCodec,
                    )
                    .await?)
            },
            cancel,
            Duration::from_secs(8),
        )
        .await?;
        let initial_status = r
            .metadata()
            .get("grpc-status")
            .map(|s| s.to_str().map(str::to_owned))
            .transpose()?;
        let mut stream = r.into_inner();
        let mut messages = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Measurement cancelled; remote silence must be checked");
            }
            tokio::select! {
                value = stream.message() => match value? {
                    None => {
                        let trailers=stream.trailers().await?;
                        let status=trailers.as_ref().and_then(|m|m.get("grpc-status")).map(|s|s.to_str()).transpose()?;
                        nad_v2::require_success_status(status.or(if messages==0 {initial_status.as_deref()}else{None}))?;
                        return Ok(());
                    },
                    Some(value) => {
                        messages += 1;
                        ensure!(value.len() <= 4096 && messages <= 256, "Native progress stream exceeded bounds");
                    }
                },
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Measurement cancelled; remote silence must be checked"
    );
    Ok(())
}
/// Keep one in-flight future alive across checks. Cancellation drops the request;
/// the caller must still perform bounded restoration after any uncertain write.
async fn cancellable<F: Future<Output = Result<T>>, T>(
    future: F,
    cancel: &AtomicBool,
    limit: Duration,
) -> Result<T> {
    let deadline = Instant::now() + limit;
    let mut future = std::pin::pin!(future);
    loop {
        check_cancel(cancel)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "Native measurement request exceeded its deadline"
        );
        match tokio::time::timeout(remaining.min(Duration::from_millis(50)), future.as_mut()).await
        {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}
fn mode_payload(mode: &NativeOperationMode) -> Vec<u8> {
    [
        mode.gain_enabled,
        mode.delay_enabled,
        mode.filtering_enabled,
        mode.measure_enabled,
        mode.streaming_enabled,
    ]
    .into_iter()
    .enumerate()
    .flat_map(|(i, v)| [((i + 1) * 8) as u8, u8::from(v)])
    .collect()
}
fn gain_payload(speaker: u32, gain: f64) -> Vec<u8> {
    let mut data = vec![8, speaker as u8, 17];
    data.extend_from_slice(&gain.to_le_bytes());
    data
}
fn read_gain(data: &[u8], speaker: u32) -> Result<f64> {
    // StimuliGain has only Index(int32) and gain(double). The generic bounded
    // decoder is shared with the metadata client, including omitted zero fields.
    nad_v2::parse_stimulus_gain(data, speaker)
}
async fn run<C: Control>(c: &mut C, p: &MeasurementPlan, cancel: &AtomicBool) -> Result<()> {
    let mode_before = nad_v2::parse_operation_mode(&c.call(Method::ReadMode, vec![], None).await?)?;
    ensure!(
        !mode_before.measure_enabled && !mode_before.streaming_enabled,
        "Amplifier is already busy measuring"
    );
    let gain_before = read_gain(
        &c.call(Method::ReadGain, vec![8, p.speaker as u8], None)
            .await?,
        p.speaker,
    )?;
    ensure!(
        gain_before.is_finite() && (-200. ..=24.).contains(&gain_before),
        "Invalid saved speaker gain"
    );
    if cancel.load(Ordering::Relaxed) {
        bail!("Measurement cancelled before amplifier changes");
    }
    let active = NativeOperationMode {
        gain_enabled: false,
        delay_enabled: false,
        filtering_enabled: false,
        measure_enabled: true,
        streaming_enabled: false,
    };
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        cancellable(
            c.call(Method::SetMode, mode_payload(&active), Some(cancel)),
            cancel,
            Duration::from_secs(4),
        )
        .await?;
        cancellable(
            c.call(
                Method::SetGain,
                gain_payload(p.speaker, p.gain_db),
                Some(cancel),
            ),
            cancel,
            Duration::from_secs(4),
        )
        .await?;
        // Built-in LOG_SWEEP type3, variant0, one packed speaker, loop=false.
        cancellable(
            c.call(
                Method::Prepare,
                vec![10, 2, 8, 3, 34, 1, p.speaker as u8],
                Some(cancel),
            ),
            cancel,
            Duration::from_secs(4),
        )
        .await?;
        if cancel.load(Ordering::Relaxed) {
            bail!("Measurement cancelled before playback");
        }
        c.play(cancel).await
    })
    .await
    .map_err(|_| anyhow!("Native measurement exceeded its 20-second control deadline"))
    .and_then(|r| r);
    // Attempt both restorations even when the first fails, including uncertain
    // outcomes after a timed-out setter. No filter slots are involved.
    let gain_restore = c
        .call(Method::SetGain, gain_payload(p.speaker, gain_before), None)
        .await;
    let mode_restore = c
        .call(Method::SetMode, mode_payload(&mode_before), None)
        .await;
    if gain_restore.is_err() || mode_restore.is_err() {
        bail!(
            "{} Amplifier restoration failed: gain={:?}, mode={:?}",
            result.err().map_or(String::new(), |e| format!("{e:#};")),
            gain_restore.err(),
            mode_restore.err()
        );
    }
    let after = nad_v2::parse_operation_mode(&c.call(Method::ReadMode, vec![], None).await?)?;
    let after_gain = read_gain(
        &c.call(Method::ReadGain, vec![8, p.speaker as u8], None)
            .await?,
        p.speaker,
    )?;
    ensure!(
        mode_payload(&after) == mode_payload(&mode_before)
            && (after_gain - gain_before).abs() < 1e-9,
        "Amplifier state restoration could not be verified"
    );
    result
}

/// Explicit playback action, intended to run after the microphone stream starts.
/// This is experimental until a bounded speaker test verifies playback and cleanup.
pub fn play_log_sweep(
    address: &str,
    port: u16,
    speaker: u32,
    cancel: Arc<AtomicBool>,
) -> Result<MeasurementPlan> {
    let device = nad_v2::inspect(address, port)?;
    let p = plan(&device, speaker)?;
    let ip = crate::device::address(address)?.ip();
    let url = format!("http://{}", std::net::SocketAddr::new(ip, port));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let channel = Endpoint::from_shared(url)?
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .connect()
            .await?;
        let mut c = Rpc {
            grpc: tonic::client::Grpc::new(channel)
                .max_encoding_message_size(1024 * 1024)
                .max_decoding_message_size(1024 * 1024),
        };
        tokio::time::timeout(Duration::from_secs(48), run(&mut c, &p, &cancel))
            .await
            .map_err(|_| anyhow!("Native measurement and restoration exceeded 48 seconds"))?
    })?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn original_mode() -> NativeOperationMode {
        NativeOperationMode {
            gain_enabled: true,
            delay_enabled: true,
            filtering_enabled: true,
            measure_enabled: false,
            streaming_enabled: false,
        }
    }
    fn example_plan() -> MeasurementPlan {
        MeasurementPlan {
            speaker: 1,
            gain_db: -15.,
            sequence_seconds: 5.5,
            sweep: NativeLogSweep {
                variant: 0,
                sample_rate: 48000.,
                start_hz: 10.,
                end_hz: 24000.,
                samples: 240000,
                silence_samples: 24000,
                periods: 1,
                fade_out_tail_hz: 300.,
            },
            reference_note: String::new(),
        }
    }
    struct Fake {
        mode: NativeOperationMode,
        gains: [f64; 2],
        events: Vec<String>,
        fail_at: Option<usize>,
        fail_play: bool,
        cancel_after: Option<(usize, Arc<AtomicBool>)>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                mode: original_mode(),
                gains: [-4., 0.],
                events: vec![],
                fail_at: None,
                fail_play: false,
                cancel_after: None,
            }
        }
    }
    impl Control for Fake {
        async fn call(
            &mut self,
            method: Method,
            payload: Vec<u8>,
            cancel: Option<&AtomicBool>,
        ) -> Result<Vec<u8>> {
            if let Some(cancel) = cancel {
                check_cancel(cancel)?;
            }
            self.events.push(format!("{method:?}"));
            let response = match method {
                Method::ReadMode => mode_payload(&self.mode),
                Method::ReadGain => {
                    gain_payload(payload[1] as u32, self.gains[payload[1] as usize])
                }
                Method::SetMode => {
                    self.mode = nad_v2::parse_operation_mode(&payload)?;
                    vec![]
                }
                Method::SetGain => {
                    let index = payload[1] as usize;
                    self.gains[index] = read_gain(&payload, index as u32)?;
                    vec![]
                }
                Method::Prepare => {
                    assert_eq!(payload, [10, 2, 8, 3, 34, 1, 1]);
                    vec![]
                }
            };
            // Simulate an ambiguous failure after a setter already took effect.
            if let Some((index, cancel)) = &self.cancel_after {
                if *index == self.events.len() - 1 {
                    cancel.store(true, Ordering::Release);
                }
            }
            if self.fail_at == Some(self.events.len() - 1) {
                bail!("Injected RPC failure");
            }
            Ok(response)
        }
        async fn play(&mut self, _: &AtomicBool) -> Result<()> {
            self.events.push("Play".into());
            assert!(self.mode.measure_enabled && !self.mode.filtering_enabled);
            assert_eq!(self.gains, [-4., -15.]);
            if self.fail_play {
                bail!("Injected stream failure");
            }
            Ok(())
        }
    }
    fn exercise(fake: &mut Fake, cancel: bool) -> Result<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(run(fake, &example_plan(), &AtomicBool::new(cancel)))
    }
    #[test]
    fn finite_sequence_preserves_other_speaker_and_restores_all_mode_flags() {
        let mut f = Fake::new();
        exercise(&mut f, false).unwrap();
        assert_eq!(
            f.events,
            [
                "ReadMode", "ReadGain", "SetMode", "SetGain", "Prepare", "Play", "SetGain",
                "SetMode", "ReadMode", "ReadGain"
            ]
        );
        assert_eq!(f.gains, [-4., 0.]);
        assert_eq!(mode_payload(&f.mode), mode_payload(&original_mode()));
    }
    #[test]
    fn stream_failure_and_uncertain_setter_failure_restore_state() {
        let mut f = Fake::new();
        f.fail_play = true;
        assert!(exercise(&mut f, false).is_err());
        assert_eq!(f.gains, [-4., 0.]);
        assert_eq!(mode_payload(&f.mode), mode_payload(&original_mode()));
        for index in 2..=4 {
            let mut f = Fake::new();
            f.fail_at = Some(index);
            assert!(exercise(&mut f, false).is_err());
            assert_eq!(f.gains, [-4., 0.]);
            assert_eq!(mode_payload(&f.mode), mode_payload(&original_mode()));
            assert!(!f.events.iter().any(|e| e == "Play"));
        }
    }
    #[test]
    fn cancellation_before_changes_and_cleanup_failure_are_explicit() {
        let mut f = Fake::new();
        assert!(exercise(&mut f, true).is_err());
        assert_eq!(f.events, ["ReadMode", "ReadGain"]);
        let mut f = Fake::new();
        f.fail_at = Some(6);
        let error = exercise(&mut f, false).unwrap_err().to_string();
        assert!(error.contains("restoration failed"));
        assert_eq!(f.events.last().unwrap(), "SetMode");
    }
    #[test]
    fn cancellation_between_setup_steps_skips_remaining_setup_and_restores() {
        for index in 2..=4 {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut f = Fake::new();
            f.cancel_after = Some((index, cancel.clone()));
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(run(&mut f, &example_plan(), &cancel));
            assert!(result.is_err());
            assert!(!f.events.iter().any(|e| e == "Play"));
            if index == 2 {
                assert!(!f.events.iter().any(|e| e == "Prepare"));
                assert_eq!(f.events.iter().filter(|e| *e == "SetGain").count(), 1);
            }
            if index == 3 {
                assert!(!f.events.iter().any(|e| e == "Prepare"));
            }
            assert_eq!(f.gains, [-4., 0.]);
            assert_eq!(mode_payload(&f.mode), mode_payload(&original_mode()));
            assert_eq!(
                &f.events[f.events.len() - 4..],
                ["SetGain", "SetMode", "ReadMode", "ReadGain"]
            );
        }
    }
    #[test]
    fn cancellation_during_ready_skips_dispatch_and_pending_handshake_is_dropped() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut dispatched = false;
        let result = rt.block_on(cancellable(
            async {
                // A readiness future can become ready after the user cancelled.
                cancel.store(true, Ordering::Release);
                check_cancel(&cancel)?;
                dispatched = true;
                Ok(())
            },
            &cancel,
            Duration::from_secs(1),
        ));
        assert!(result.is_err());
        assert!(!dispatched);
        cancel.store(false, Ordering::Release);
        let result = rt.block_on(cancellable(
            async {
                cancel.store(true, Ordering::Release);
                std::future::pending::<Result<()>>().await
            },
            &cancel,
            Duration::from_secs(1),
        ));
        assert!(result.is_err());
    }
    #[test]
    fn original_sweep_has_exact_dimensions_peak_bound_and_trailing_silence() {
        let p = example_plan();
        let x = reference(&p).unwrap();
        assert_eq!(x.len(), 264000);
        assert_eq!(x[0], 0.);
        assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 0.177829));
        assert!(x[240000..].iter().all(|v| *v == 0.));
        let ratio = (24000_f64 / 10.).ln();
        let k = (240000. * (23700_f64 / 10.).ln() / ratio).round() as usize;
        assert_eq!(240000 - k, 388);
        assert!(x[239999].abs() < 1e-5);
        let mut bad = p;
        bad.sweep.samples = u32::MAX;
        assert!(reference(&bad).is_err());
    }
}
