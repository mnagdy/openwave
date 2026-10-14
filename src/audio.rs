//! Local CoreAudio input/output and portable mono WAV recordings.
//! Device/channel selection is explicit; WAV imports select the first channel.
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::Serialize;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const MAX_SECONDS: f64 = 180.0;
const MAX_RATE: u32 = 192_000;
const LEAD_IN: f64 = 0.5;
const TAIL: f64 = 2.0;

#[derive(Debug, Clone, Serialize)]
pub struct AudioInput {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone)]
pub struct Recording {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub peak_db: f64,
    pub clipped: bool,
}

type Failure = Arc<Mutex<Option<String>>>;
type Samples = Arc<Mutex<Vec<f32>>>;

fn failure() -> Failure {
    Arc::new(Mutex::new(None))
}

fn set_failure(error: &Failure, message: String) {
    if let Ok(mut slot) = error.lock() {
        if slot.is_none() {
            *slot = Some(message);
        }
    }
}

fn check_failure(error: &Failure) -> Result<()> {
    if let Some(message) = error
        .lock()
        .map_err(|_| anyhow!("Audio error state poisoned"))?
        .as_ref()
    {
        bail!("Audio stream failed: {message}");
    }
    Ok(())
}

// Resolve names consistently across listing and subsequent selection.
#[cfg(not(target_os = "macos"))]
fn devices(input: bool) -> Result<Vec<(String, Device)>> {
    let host = cpal::default_host();
    let iter = if input {
        host.input_devices()?
    } else {
        host.output_devices()?
    };
    let mut counts = std::collections::HashMap::new();
    let mut used = std::collections::HashSet::new();
    let mut result = Vec::new();
    for (index, device) in iter.enumerate() {
        let base = device
            .name()
            .unwrap_or_else(|_| format!("Unnamed audio device {}", index + 1));
        let count = counts.entry(base.clone()).or_insert(0usize);
        *count += 1;
        let mut name = if *count == 1 {
            base.clone()
        } else {
            format!("{base} [{}]", *count)
        };
        while !used.insert(name.clone()) {
            *count += 1;
            name = format!("{base} [{}]", *count);
        }
        result.push((name, device));
    }
    Ok(result)
}

#[cfg(not(target_os = "macos"))]
fn select(name: &str, input: bool) -> Result<Device> {
    devices(input)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
        .ok_or_else(|| anyhow!("Audio device '{name}' is unavailable; refresh the device list"))
}

#[cfg(not(target_os = "macos"))]
fn configs(device: &Device, input: bool) -> Result<Vec<cpal::SupportedStreamConfigRange>> {
    if input {
        Ok(device.supported_input_configs()?.collect())
    } else {
        Ok(device.supported_output_configs()?.collect())
    }
}

#[cfg(not(target_os = "macos"))]
fn supported_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::I64
            | SampleFormat::U8
            | SampleFormat::U16
            | SampleFormat::U32
            | SampleFormat::U64
            | SampleFormat::F32
            | SampleFormat::F64
    )
}

#[cfg(not(target_os = "macos"))]
fn choose(
    device: &Device,
    input: bool,
    channel: usize,
    rate: Option<u32>,
) -> Result<cpal::SupportedStreamConfig> {
    if channel >= u16::MAX as usize {
        bail!("Channel index is out of range");
    }
    if let Some(rate) = rate {
        if rate == 0 || rate > MAX_RATE {
            bail!("Sample rate must be between 1 and {MAX_RATE} Hz");
        }
    }
    let default = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    };
    if let Ok(config) = default {
        if config.channels() as usize > channel
            && supported_format(config.sample_format())
            && config.sample_rate().0 <= MAX_RATE
            && rate.is_none_or(|r| r == config.sample_rate().0)
        {
            return Ok(config);
        }
    }
    let mut candidates = configs(device, input)?;
    // Prefer float, then few channels to avoid touching unselected speaker channels.
    candidates.sort_by_key(|c| (c.sample_format() != SampleFormat::F32, c.channels()));
    for range in candidates {
        if range.channels() as usize <= channel || !supported_format(range.sample_format()) {
            continue;
        }
        let min = range.min_sample_rate().0;
        let max = range.max_sample_rate().0.min(MAX_RATE);
        if min > max {
            continue;
        }
        let selected = rate.unwrap_or_else(|| 48_000u32.clamp(min, max));
        if selected >= min && selected <= max {
            return Ok(range.with_sample_rate(cpal::SampleRate(selected)));
        }
    }
    bail!(
        "Device does not support channel {}{}",
        channel + 1,
        rate.map_or(String::new(), |r| format!(" at {r} Hz"))
    )
}

#[cfg(not(target_os = "macos"))]
fn enumerate(input: bool) -> Result<Vec<AudioInput>> {
    let mut list = Vec::new();
    for (name, device) in devices(input)? {
        // A device can disappear during enumeration. Skip devices with no usable configuration.
        if let Ok(config) = choose(&device, input, 0, None) {
            list.push(AudioInput {
                name,
                channels: config.channels(),
                sample_rate: config.sample_rate().0,
            });
        }
    }
    Ok(list)
}

pub struct AudioDevices {
    pub inputs: Vec<AudioInput>,
    pub outputs: Vec<AudioInput>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
    pub notes: Vec<String>,
}
pub fn device_snapshot() -> Result<AudioDevices> {
    #[cfg(target_os = "macos")]
    {
        let snapshot = crate::mac_audio::snapshot()?;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut notes = snapshot.issues;
        for d in snapshot.devices {
            notes.extend(d.issues.iter().map(|s| format!("{}: {s}", d.name)));
            if d.sample_rate == 0 || d.sample_rate > MAX_RATE {
                continue;
            }
            if d.input_channels > 0 {
                inputs.push(AudioInput {
                    name: d.name.clone(),
                    channels: d.input_channels,
                    sample_rate: d.sample_rate,
                });
            }
            if d.output_channels > 0 {
                outputs.push(AudioInput {
                    name: d.name,
                    channels: d.output_channels,
                    sample_rate: d.sample_rate,
                });
            }
        }
        Ok(AudioDevices {
            inputs,
            outputs,
            default_input: snapshot.default_input_name,
            default_output: snapshot.default_output_name,
            notes,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(AudioDevices {
            inputs: enumerate(true)?,
            outputs: enumerate(false)?,
            default_input: default_input_name(),
            default_output: default_output_name(),
            notes: vec![],
        })
    }
}
pub fn inputs() -> Result<Vec<AudioInput>> {
    Ok(device_snapshot()?.inputs)
}
pub fn outputs() -> Result<Vec<AudioInput>> {
    Ok(device_snapshot()?.outputs)
}

fn select_config(
    name: &str,
    input: bool,
    channel: usize,
    rate: Option<u32>,
) -> Result<(Device, cpal::SupportedStreamConfig)> {
    #[cfg(target_os = "macos")]
    {
        let snapshot = crate::mac_audio::snapshot()?;
        let (metadata, config) =
            crate::mac_audio::select_config(&snapshot, name, input, channel, rate)?;
        let device = cpal::default_host()
            .devices()?
            .nth(metadata.enumeration_index)
            .ok_or_else(|| anyhow!("Audio device disappeared; refresh the device list"))?;
        // Confirm that hot-plugging did not change the selected enumeration entry.
        let current = crate::mac_audio::snapshot()?;
        if current
            .devices
            .iter()
            .find(|d| d.enumeration_index == metadata.enumeration_index)
            .map(|d| d.id)
            != Some(metadata.id)
        {
            bail!("Audio devices changed during selection; refresh and retry");
        }
        Ok((device, config))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let device = select(name, input)?;
        let config = choose(&device, input, channel, rate)?;
        Ok((device, config))
    }
}

fn input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    channel: usize,
    samples: Samples,
    max_samples: usize,
    error: Failure,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let callback_error = error.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            // Convert an entire callback before locking; no disk access in the audio callback.
            let batch: Vec<f32> = data
                .chunks_exact(channels)
                .map(|frame| frame[channel].to_sample::<f32>())
                .collect();
            if batch.iter().any(|v| !v.is_finite()) {
                set_failure(&callback_error, "Input supplied non-finite samples".into());
                return;
            }
            if let Ok(mut buffer) = samples.lock() {
                let remaining = max_samples.saturating_sub(buffer.len());
                buffer.extend_from_slice(&batch[..remaining.min(batch.len())]);
            } else {
                set_failure(&callback_error, "Input buffer poisoned".into());
            }
        },
        move |e| set_failure(&error, e.to_string()),
        None,
    )?)
}

fn build_input(
    device: &Device,
    config: &cpal::SupportedStreamConfig,
    channel: usize,
    samples: Samples,
    max_samples: usize,
    error: Failure,
) -> Result<Stream> {
    macro_rules! build {
        ($t:ty) => {
            input_stream::<$t>(
                device,
                &config.config(),
                channel,
                samples,
                max_samples,
                error,
            )
        };
    }
    match config.sample_format() {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => bail!("Unsupported input sample format: {other:?}"),
    }
}

fn output_stream<T>(
    device: &Device,
    config: &StreamConfig,
    channel: usize,
    signal: Arc<Vec<f32>>,
    finished: Arc<AtomicBool>,
    error: Failure,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut offset = 0usize;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            for frame in data.chunks_exact_mut(channels) {
                frame.fill(T::from_sample(0.0f32));
                if offset < signal.len() {
                    frame[channel] = T::from_sample(signal[offset]);
                }
                offset = offset.saturating_add(1);
            }
            if offset >= signal.len() {
                finished.store(true, Ordering::Release);
            }
        },
        move |e| set_failure(&error, e.to_string()),
        None,
    )?)
}

fn build_output(
    device: &Device,
    config: &cpal::SupportedStreamConfig,
    channel: usize,
    signal: Arc<Vec<f32>>,
    finished: Arc<AtomicBool>,
    error: Failure,
) -> Result<Stream> {
    macro_rules! build {
        ($t:ty) => {
            output_stream::<$t>(device, &config.config(), channel, signal, finished, error)
        };
    }
    match config.sample_format() {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        other => bail!("Unsupported output sample format: {other:?}"),
    }
}

fn finish(samples: Vec<f32>, sample_rate: u32) -> Result<Recording> {
    if samples.is_empty() {
        bail!("Recording contains no samples");
    }
    if sample_rate == 0 || samples.iter().any(|v| !v.is_finite()) {
        bail!("Invalid recording samples or sample rate");
    }
    let peak = samples.iter().fold(0.0f64, |p, s| p.max((*s as f64).abs()));
    Ok(Recording {
        samples,
        sample_rate,
        peak_db: if peak == 0.0 {
            f64::NEG_INFINITY
        } else {
            20.0 * peak.log10()
        },
        clipped: peak >= 0.999,
    })
}

pub fn capture(input_name: &str, channel: usize, seconds: f64) -> Result<Recording> {
    if !seconds.is_finite() || seconds <= 0.0 || seconds > MAX_SECONDS {
        bail!("Recording duration must be greater than 0 and at most {MAX_SECONDS} seconds");
    }
    #[cfg(target_os = "macos")]
    crate::mac_permission::microphone()?;
    let (device, config) = select_config(input_name, true, channel, None)?;
    let rate = config.sample_rate().0;
    let count = (seconds * rate as f64).ceil() as usize;
    let samples = Arc::new(Mutex::new(Vec::with_capacity(count)));
    let error = failure();
    let stream = build_input(
        &device,
        &config,
        channel,
        samples.clone(),
        count,
        error.clone(),
    )?;
    stream
        .play()
        .context("Starting microphone; check macOS microphone permission")?;
    let deadline = Instant::now() + Duration::from_secs_f64(seconds + 5.0);
    loop {
        check_failure(&error)?;
        if samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?
            .len()
            >= count
        {
            break;
        }
        if Instant::now() > deadline {
            bail!("Microphone timed out or disconnected before the recording finished");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    check_failure(&error)?;
    let captured = std::mem::take(
        &mut *samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?,
    );
    finish(captured, rate)
}

/// Keep the selected microphone running around an externally controlled signal.
/// The action starts only after input arrives, and must impose its own <=90 s
/// deadline. This function performs no playback or network operation itself.
/// Returned samples include microphone startup and the requested capture tail.
pub fn capture_with_action<F>(
    input_name: &str,
    channel: usize,
    exact_rate: u32,
    tail_seconds: f64,
    action: F,
) -> Result<Recording>
where
    F: FnOnce() -> Result<()>,
{
    if exact_rate == 0 || exact_rate > MAX_RATE {
        bail!("Invalid measurement sample rate");
    }
    if channel >= u16::MAX as usize {
        bail!("Channel index is out of range");
    }
    if !tail_seconds.is_finite() || tail_seconds < 0.0 || tail_seconds > MAX_SECONDS {
        bail!("Capture tail must be between 0 and {MAX_SECONDS} seconds");
    }
    #[cfg(target_os = "macos")]
    crate::mac_permission::microphone()?;
    let (device, config) = select_config(input_name, true, channel, Some(exact_rate))?;
    let maximum = (MAX_SECONDS * exact_rate as f64) as usize;
    // Grow as callbacks arrive rather than preallocating three minutes up front.
    let samples = Arc::new(Mutex::new(Vec::with_capacity(2 * exact_rate as usize)));
    let error = failure();
    let stream = build_input(
        &device,
        &config,
        channel,
        samples.clone(),
        maximum,
        error.clone(),
    )?;
    stream
        .play()
        .context("Starting microphone; check macOS microphone permission")?;
    let started = Instant::now();
    let ready_deadline = started + Duration::from_secs(2);
    let sample_count = || -> Result<usize> {
        Ok(samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?
            .len())
    };
    loop {
        check_failure(&error)?;
        if Instant::now() >= ready_deadline {
            bail!("Microphone supplied no first buffer within two seconds; signal was not started");
        }
        if sample_count()? > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // The caller controls the synchronous action's timeout. Dropping this stream
    // on any error ends capture; invoking a network action here does not create
    // a CoreAudio output or generate an additional signal.
    check_failure(&error)?;
    check_action_capture_limit(sample_count()?, maximum, started.elapsed())?;
    let action_started = Instant::now();
    let action_result = action();
    check_failure(&error)?;
    action_result.context("Running the measurement signal while recording")?;
    if action_started.elapsed() > Duration::from_secs(90) {
        bail!("Measurement action exceeded its 90-second deadline");
    }
    let initial_tail_count = sample_count()?;
    check_action_capture_limit(initial_tail_count, maximum, started.elapsed())?;
    let tail_samples = (tail_seconds * exact_rate as f64).ceil() as usize;
    let required = initial_tail_count
        .checked_add(tail_samples)
        .filter(|count| *count < maximum)
        .ok_or_else(|| anyhow!("Capture tail would exceed the 180-second recording limit"))?;
    let tail_started = Instant::now();
    let mut last_count = initial_tail_count;
    let mut last_input_at = tail_started;
    loop {
        check_failure(&error)?;
        let now = Instant::now();
        let count = sample_count()?;
        check_action_capture_limit(count, maximum, now.duration_since(started))?;
        if count > last_count {
            last_count = count;
            last_input_at = now;
        } else if now.duration_since(last_input_at) >= Duration::from_secs(2) {
            bail!("Microphone stopped supplying data during the measurement tail");
        }
        if count >= required && now.duration_since(tail_started).as_secs_f64() >= tail_seconds {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    check_failure(&error)?;
    let captured = std::mem::take(
        &mut *samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?,
    );
    // Account for a final callback racing with the last loop iteration.
    check_action_capture_limit(captured.len(), maximum, started.elapsed())?;
    finish(captured, exact_rate)
}

fn check_action_capture_limit(count: usize, maximum: usize, elapsed: Duration) -> Result<()> {
    if count >= maximum {
        bail!("Microphone recording buffer reached its 180-second limit");
    }
    if elapsed.as_secs_f64() >= MAX_SECONDS {
        bail!("Microphone recording exceeded its 180-second deadline");
    }
    Ok(())
}

pub fn record(input_name: &str, channel: usize, seconds: f64, path: &Path) -> Result<Recording> {
    let recording = capture(input_name, channel, seconds)?;
    write_wav(path, &recording.samples, recording.sample_rate)?;
    Ok(recording)
}

pub fn default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| d.name().ok())
}
pub fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
}

/// Play one channel and capture one input channel at an exact shared sample rate.
/// Signal has 0.5 s silence before and 2 s after; returned samples retain capture latency.
pub fn measure(
    input_name: &str,
    input_channel: usize,
    output_name: &str,
    output_channel: usize,
    sweep: &[f32],
    sample_rate: u32,
) -> Result<Recording> {
    if sample_rate == 0 || sample_rate > MAX_RATE {
        bail!("Invalid measurement sample rate");
    }
    let seconds = sweep.len() as f64 / sample_rate as f64;
    if sweep.is_empty() || seconds + LEAD_IN + TAIL + 5.0 > MAX_SECONDS {
        bail!("Sweep is empty or exceeds the bounded measurement duration");
    }
    if sweep.iter().any(|s| !s.is_finite() || s.abs() > 0.100_001) {
        bail!("Sweep must contain finite samples with peak at most -20 dBFS (0.1)");
    }
    #[cfg(target_os = "macos")]
    crate::mac_permission::microphone()?;
    let (input, input_config) = select_config(input_name, true, input_channel, Some(sample_rate))?;
    let (output, output_config) =
        select_config(output_name, false, output_channel, Some(sample_rate))?;
    let lead = (LEAD_IN * sample_rate as f64).round() as usize;
    let tail = (TAIL * sample_rate as f64).round() as usize;
    let mut signal = vec![0.0; lead];
    signal.extend_from_slice(sweep);
    signal.resize(signal.len() + tail, 0.0);
    let duration = signal.len() as f64 / sample_rate as f64;
    let maximum = ((duration + 5.0) * sample_rate as f64).ceil() as usize;
    let samples = Arc::new(Mutex::new(Vec::with_capacity(maximum)));
    let error = failure();
    let finished = Arc::new(AtomicBool::new(false));
    let input_stream = build_input(
        &input,
        &input_config,
        input_channel,
        samples.clone(),
        maximum,
        error.clone(),
    )?;
    let output_stream = build_output(
        &output,
        &output_config,
        output_channel,
        Arc::new(signal),
        finished.clone(),
        error.clone(),
    )?;
    input_stream
        .play()
        .context("Starting microphone; check macOS microphone permission")?;
    output_stream
        .play()
        .context("Starting selected audio output")?;
    let deadline = Instant::now() + Duration::from_secs_f64(duration + 5.0);
    let mut drained_at = None;
    let mut last_count = 0usize;
    let mut last_input_at = Instant::now();
    loop {
        check_failure(&error)?;
        let count = samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?
            .len();
        if count > last_count {
            last_count = count;
            last_input_at = Instant::now();
        } else if last_input_at.elapsed() > Duration::from_secs(2) {
            bail!("Microphone stopped supplying data during measurement");
        }
        if finished.load(Ordering::Acquire) {
            let when = drained_at.get_or_insert_with(Instant::now);
            // Output callbacks precede DAC playback; retain an extra short capture tail.
            if when.elapsed() >= Duration::from_millis(250) {
                break;
            }
        }
        if Instant::now() > deadline {
            bail!("Measurement timed out; audio device may have disconnected");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(output_stream);
    drop(input_stream);
    check_failure(&error)?;
    let captured = std::mem::take(
        &mut *samples
            .lock()
            .map_err(|_| anyhow!("Input buffer poisoned"))?,
    );
    if captured.len() < sweep.len() {
        bail!("Microphone stopped or supplied insufficient data during measurement");
    }
    finish(captured, sample_rate)
}

/// Read float32 or 8/16/24/32-bit PCM WAV, selecting channel 1 from multichannel files.
pub fn read_wav(path: &Path) -> Result<Recording> {
    let mut reader = hound::WavReader::open(path).context("Opening WAV recording")?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        bail!("Invalid WAV channel count or sample rate");
    }
    let count = reader.duration() as usize;
    if spec.sample_rate > MAX_RATE || count as f64 / spec.sample_rate as f64 > MAX_SECONDS {
        bail!("WAV exceeds {MAX_SECONDS} seconds or {MAX_RATE} Hz");
    }
    let channels = spec.channels as usize;
    let mut mono = Vec::with_capacity(count);
    let mut total = 0usize;
    match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                bail!("Only 32-bit float WAV is supported");
            }
            for (index, sample) in reader.samples::<f32>().enumerate() {
                let value = sample.context("Reading float WAV sample")?;
                if !value.is_finite() {
                    bail!("WAV contains non-finite samples");
                }
                if index % channels == 0 {
                    mono.push(value);
                }
                total += 1;
            }
        }
        hound::SampleFormat::Int => {
            if !matches!(spec.bits_per_sample, 8 | 16 | 24 | 32) {
                bail!("Unsupported PCM WAV bit depth");
            }
            let scale = 2.0f64.powi(spec.bits_per_sample as i32 - 1);
            for (index, sample) in reader.samples::<i32>().enumerate() {
                let value = sample.context("Reading PCM WAV sample")?;
                if index % channels == 0 {
                    mono.push((value as f64 / scale) as f32);
                }
                total += 1;
            }
        }
    }
    if !total.is_multiple_of(channels) {
        bail!("WAV ends in an incomplete channel frame");
    }
    finish(mono, spec.sample_rate)
}

pub fn write_wav(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    if rate == 0 || rate > MAX_RATE || samples.is_empty() || samples.iter().any(|s| !s.is_finite())
    {
        bail!("Cannot write WAV with invalid sample rate, empty data, or non-finite samples");
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).context("Creating WAV recording")?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize().context("Finalizing WAV recording")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("open-room-audio-{}-{name}.wav", std::process::id()))
    }
    #[test]
    fn float_roundtrip_preserves_samples_and_clipping() {
        let path = temporary("float");
        let data = vec![-1.0, -0.25, 0.0, 0.125, 0.9];
        write_wav(&path, &data, 48_000).unwrap();
        let recording = read_wav(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(recording.samples, data);
        assert_eq!(recording.sample_rate, 48_000);
        assert!(recording.clipped);
        assert_eq!(recording.peak_db, 0.0);
    }
    #[test]
    fn stereo_pcm_import_selects_first_channel_and_scales() {
        let path = temporary("stereo");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in [-16_384i16, 30_000, 8_192, -30_000] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let recording = read_wav(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(recording.samples, vec![-0.5, 0.25]);
        assert!(!recording.clipped);
    }
    #[test]
    fn rejects_nonfinite_wav_samples() {
        let path = temporary("nan");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(f32::NAN).unwrap();
        writer.finalize().unwrap();
        assert!(read_wav(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn rejects_malformed_wav_header() {
        let path = temporary("malformed");
        std::fs::write(&path, b"RIFF\x01\x00\x00\x00WAVEbad").unwrap();
        assert!(read_wav(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn pcm_bit_depths_preserve_normalization() {
        for bits in [8, 24, 32] {
            let path = temporary(&format!("pcm-{bits}"));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: bits,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            let magnitude = 1i32 << (bits - 2);
            writer.write_sample(-magnitude).unwrap();
            writer.write_sample(magnitude).unwrap();
            writer.finalize().unwrap();
            let recording = read_wav(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(recording.samples, vec![-0.5, 0.5]);
        }
    }
    #[test]
    fn action_capture_rejects_bad_parameters_without_running_action() {
        let ran = std::cell::Cell::new(false);
        for (rate, tail, channel) in [
            (0, 2.0, 0),
            (MAX_RATE + 1, 2.0, 0),
            (48_000, f64::NAN, 0),
            (48_000, -1.0, 0),
            (48_000, MAX_SECONDS + 1.0, 0),
            (48_000, 2.0, u16::MAX as usize),
        ] {
            assert!(capture_with_action("missing", channel, rate, tail, || {
                ran.set(true);
                Ok(())
            })
            .is_err());
        }
        assert!(!ran.get());
    }
    #[test]
    fn action_capture_limits_reject_saturated_or_overlong_recordings() {
        assert!(check_action_capture_limit(9, 10, Duration::from_secs(179)).is_ok());
        assert!(check_action_capture_limit(10, 10, Duration::from_secs(1)).is_err());
        assert!(check_action_capture_limit(2, 10, Duration::from_secs(180)).is_err());
    }
    #[test]
    fn invalid_recording_is_rejected_before_device_access() {
        assert!(record("missing", 0, f64::NAN, Path::new("unused.wav")).is_err());
        assert!(measure("missing", 0, "missing", 0, &[0.5], 48_000).is_err());
        assert!(finish(vec![], 48_000).is_err());
    }
}
