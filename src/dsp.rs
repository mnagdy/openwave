//! Independently implemented measurement and magnitude-only room correction.
//! This is not an implementation of Dirac's proprietary optimization or filter format.

use anyhow::{bail, ensure, Context, Result};
use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub hz: f64,
    pub db: f64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SweepConfig {
    pub sample_rate: u32,
    pub seconds: f64,
    pub start_hz: f64,
    pub end_hz: f64,
    pub level_db: f64,
}

fn valid_rate(rate: u32) -> Result<()> {
    ensure!(
        (8_000..=192_000).contains(&rate),
        "sample rate must be 8000–192000 Hz"
    );
    Ok(())
}

fn validate_points(points: &[Point]) -> Result<()> {
    ensure!(!points.is_empty(), "curve must contain at least one point");
    let mut previous = 0.0;
    for p in points {
        ensure!(
            p.hz.is_finite() && p.hz > previous,
            "curve frequencies must be positive, finite and strictly increasing"
        );
        ensure!(
            p.db.is_finite() && p.db.abs() <= 200.0,
            "curve gains must be finite and within ±200 dB"
        );
        previous = p.hz;
    }
    Ok(())
}

/// Interpolate gain linearly in log frequency; hold the endpoint gains outside the curve.
pub fn interpolate(points: &[Point], hz: f64) -> Result<f64> {
    validate_points(points)?;
    ensure!(
        hz.is_finite() && hz > 0.0,
        "frequency must be positive and finite"
    );
    Ok(interpolate_validated(points, hz))
}

fn interpolate_validated(points: &[Point], hz: f64) -> f64 {
    if hz <= points[0].hz {
        return points[0].db;
    }
    let last = &points[points.len() - 1];
    if hz >= last.hz {
        return last.db;
    }
    let i = points.partition_point(|p| p.hz < hz);
    let (a, b) = (&points[i - 1], &points[i]);
    let fraction = (hz / a.hz).ln() / (b.hz / a.hz).ln();
    a.db + fraction * (b.db - a.db)
}

/// Subtract a microphone's frequency-response error from a measured response.
/// A positive calibration gain means the microphone reads high at that frequency.
/// Calibration endpoints must cover the measurement; unknown tails are not extrapolated.
pub fn calibrate(measured: &[Point], calibration: &[Point]) -> Result<Vec<Point>> {
    validate_points(measured)?;
    validate_points(calibration)?;
    ensure!(
        calibration[0].hz <= measured[0].hz
            && calibration[calibration.len() - 1].hz >= measured[measured.len() - 1].hz,
        "microphone calibration must cover the entire measured frequency range"
    );
    let corrected: Vec<_> = measured
        .iter()
        .map(|p| Point {
            hz: p.hz,
            db: p.db - interpolate_validated(calibration, p.hz),
        })
        .collect();
    validate_points(&corrected)?;
    Ok(corrected)
}

/// Import common microphone calibration text: frequency Hz, response error dB,
/// and optionally phase. Phase and recognized sensitivity headers are not applied.
/// CSV, proprietary containers and unrecognized text headers are rejected.
pub fn import_calibration(text: &str) -> Result<Vec<Point>> {
    ensure!(
        text.len() <= 1_048_576,
        "microphone calibration exceeds 1 MiB"
    );
    let mut points = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty()
            || ["#", ";", "*", "//"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        {
            continue;
        }
        let header = line.trim_matches('"').trim().to_ascii_lowercase();
        // Common UMIK/measurement-microphone headers carry sensitivity or serial
        // metadata, not frequency compensation. Ignoring these is explicit here.
        if header.starts_with("sens factor =")
            || header.starts_with("sensitivity =")
            || header.starts_with("serial number:")
            || header.starts_with("serno:")
        {
            continue;
        }
        if (header.starts_with("frequency") || header.starts_with("freq"))
            && header.contains("hz")
            && header.contains("db")
        {
            continue;
        }
        let line = line.split(['#', ';']).next().unwrap_or("").trim();
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 2 || fields.len() == 3,
            "calibration line {} must contain frequency Hz, gain dB and optional phase; unknown formats are unsupported", index + 1);
        let hz = fields[0].parse::<f64>().with_context(|| {
            format!(
                "invalid calibration frequency or unknown header on line {}",
                index + 1
            )
        })?;
        let db = fields[1]
            .parse::<f64>()
            .with_context(|| format!("invalid calibration gain on line {}", index + 1))?;
        if fields.len() == 3 {
            let phase = fields[2]
                .parse::<f64>()
                .with_context(|| format!("invalid calibration phase on line {}", index + 1))?;
            ensure!(
                phase.is_finite(),
                "non-finite calibration phase on line {}",
                index + 1
            );
        }
        points.push(Point { hz, db });
    }
    validate_points(&points)?;
    Ok(points)
}

/// Logarithmic sine sweep with 20 ms cosine fades and an explicit peak-level limit.
pub fn sweep(config: &SweepConfig) -> Result<Vec<f32>> {
    valid_rate(config.sample_rate)?;
    ensure!(
        config.seconds.is_finite() && (0.25..=60.0).contains(&config.seconds),
        "sweep duration must be 0.25–60 seconds"
    );
    ensure!(
        config.start_hz.is_finite()
            && config.end_hz.is_finite()
            && config.start_hz >= 5.0
            && config.end_hz > config.start_hz
            && config.end_hz < config.sample_rate as f64 / 2.0,
        "sweep frequencies must increase from ≥5 Hz to below Nyquist"
    );
    ensure!(
        config.level_db.is_finite() && (-90.0..=-3.0).contains(&config.level_db),
        "sweep peak level must be -90 to -3 dBFS"
    );
    let count = (config.seconds * config.sample_rate as f64).round() as usize;
    let duration = count as f64 / config.sample_rate as f64;
    let logarithm = (config.end_hz / config.start_hz).ln();
    let k = duration / logarithm;
    let amplitude = 10.0_f64.powf(config.level_db / 20.0);
    let fade = ((0.020 * config.sample_rate as f64).round() as usize)
        .min(count / 8)
        .max(1);
    Ok((0..count)
        .map(|i| {
            let t = i as f64 / config.sample_rate as f64;
            let phase = 2.0 * PI * config.start_hz * k * (t / k).exp_m1();
            let edge = i.min(count - 1 - i);
            let envelope = if edge < fade {
                0.5 - 0.5 * (PI * edge as f64 / fade as f64).cos()
            } else {
                1.0
            };
            (amplitude * envelope * phase.sin()) as f32
        })
        .collect())
}

/// Estimate the full-record transfer magnitude in approximately 1/12-octave bands.
/// Preserve the complete recording, including leading delay and the reverberant tail.
/// Delay changes phase, not magnitude, so no potentially incorrect peak alignment is used.
pub fn analyze(reference: &[f32], recording: &[f32], sample_rate: u32) -> Result<Vec<Point>> {
    valid_rate(sample_rate)?;
    ensure!(
        reference.len() >= 256 && recording.len() >= 256,
        "signals need at least 256 samples"
    );
    ensure!(
        reference.iter().chain(recording).all(|v| v.is_finite()),
        "signals contain non-finite samples"
    );
    let n = reference
        .len()
        .max(recording.len())
        .checked_next_power_of_two()
        .context("signal too long")?;
    ensure!(n <= 16_777_216, "recording exceeds analysis memory limit");
    let mut x = vec![Complex::<f64>::new(0.0, 0.0); n];
    let mut y = x.clone();
    // Remove electrical DC without changing the captured leading delay or tail.
    for (data, source) in [(&mut x, reference), (&mut y, recording)] {
        let mean = source.iter().map(|&v| v as f64).sum::<f64>() / source.len() as f64;
        for (dest, &value) in data.iter_mut().zip(source) {
            dest.re = value as f64 - mean;
        }
    }
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    fft.process(&mut x);
    fft.process(&mut y);
    let max_power = x[1..n / 2].iter().map(|z| z.norm_sqr()).fold(0.0, f64::max);
    ensure!(max_power > 1e-12, "reference signal is silent");
    ensure!(
        y.iter().map(|z| z.norm_sqr()).sum::<f64>() > 1e-12,
        "recording is silent"
    );
    let regularizer = max_power * 1e-8;
    let bin_hz = sample_rate as f64 / n as f64;
    let lower = 20.0_f64.max(bin_hz * 4.0);
    let upper = 20_000.0_f64.min(sample_rate as f64 * 0.475);
    ensure!(lower < upper, "recording too short for response analysis");
    let count = 193;
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        let hz = lower * (upper / lower).powf(i as f64 / (count - 1) as f64);
        let radius = 2.0_f64.powf(1.0 / 24.0);
        let first = ((hz / radius / bin_hz).floor() as usize).max(1);
        let last = ((hz * radius / bin_hz).ceil() as usize).min(n / 2 - 1);
        let mut weighted_power = 0.0;
        let mut weight = 0.0;
        for j in first..=last {
            let power = x[j].norm_sqr();
            // Omit essentially unexcited bins: dividing them would turn noise into gain.
            if power < max_power * 1e-8 {
                continue;
            }
            let h = y[j] * x[j].conj() / (power + regularizer);
            weighted_power += h.norm_sqr() * power;
            weight += power;
        }
        if weight > regularizer {
            points.push(Point {
                hz,
                db: 10.0 * (weighted_power / weight).max(1e-20).log10(),
            });
        }
    }
    ensure!(
        points.len() >= 8,
        "reference has insufficient broadband excitation"
    );
    Ok(points)
}

/// Windowed frequency-sampling linear-phase FIR. At most `max_boost_db` boost
/// and 24 dB cut before normalization. Frequency curtains taper over 1/6 octave.
/// The final filter is scaled to a maximum sampled frequency response of 0 dB;
/// this attenuation applies equally outside the correction curtains.
pub fn design_filter(
    measured: &[Point],
    target: &[Point],
    sample_rate: u32,
    taps: usize,
    low_hz: f64,
    high_hz: f64,
    max_boost_db: f64,
) -> Result<Vec<f32>> {
    valid_rate(sample_rate)?;
    validate_points(measured)?;
    validate_points(target)?;
    ensure!(
        (3..=32769).contains(&taps),
        "FIR length must be 3–32769 taps"
    );
    ensure!(
        low_hz.is_finite()
            && high_hz.is_finite()
            && low_hz > 0.0
            && high_hz > low_hz
            && high_hz < sample_rate as f64 / 2.0,
        "curtains must be positive, increasing, and below Nyquist"
    );
    ensure!(
        low_hz >= measured[0].hz && high_hz <= measured[measured.len() - 1].hz,
        "correction curtains must stay within the measured frequency range"
    );
    ensure!(
        max_boost_db.is_finite() && (0.0..=12.0).contains(&max_boost_db),
        "maximum boost must be 0–12 dB"
    );
    let n = (taps * 16).next_power_of_two();
    let delay = (taps - 1) as f64 / 2.0;
    let mut spectrum = vec![Complex::new(0.0, 0.0); n];
    for i in 0..=n / 2 {
        let hz = i as f64 * sample_rate as f64 / n as f64;
        let correction = if hz > low_hz && hz < high_hz {
            let edge_octaves = (hz / low_hz).log2().min((high_hz / hz).log2());
            let t = (edge_octaves * 6.0).clamp(0.0, 1.0);
            let curtain = 0.5 - 0.5 * (PI * t).cos();
            (interpolate_validated(target, hz) - interpolate_validated(measured, hz))
                .clamp(-24.0, max_boost_db)
                * curtain
        } else {
            0.0
        };
        let magnitude = 10.0_f64.powf(correction / 20.0);
        let phase = -2.0 * PI * i as f64 * delay / n as f64;
        spectrum[i] = Complex::from_polar(magnitude, phase);
        if i > 0 && i < n / 2 {
            spectrum[n - i] = spectrum[i].conj();
        }
    }
    // An even tap count has a half-sample delay and a forced zero at Nyquist.
    spectrum[n / 2].im = 0.0;
    let mut planner = FftPlanner::new();
    planner.plan_fft_inverse(n).process(&mut spectrum);
    let mut coefficients: Vec<f64> = (0..taps)
        .map(|i| {
            let phase = 2.0 * PI * i as f64 / (taps - 1) as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            spectrum[i].re * window / n as f64
        })
        .collect();
    // Measure the windowed result; normalizing the requested spectrum alone can miss ripple.
    let mut check = vec![Complex::new(0.0, 0.0); n];
    for (z, value) in check.iter_mut().zip(&coefficients) {
        z.re = *value;
    }
    planner.plan_fft_forward(n).process(&mut check);
    let peak = check.iter().map(|z| z.norm()).fold(1.0, f64::max);
    for value in &mut coefficients {
        *value /= peak;
    }
    Ok(coefficients.into_iter().map(|v| v as f32).collect())
}

/// Interoperable target-curve text, independently authored from the public example grammar.
/// This is target data for an existing Dirac application, never a device filter payload.
pub fn export_target(points: &[Point], low: f64, high: f64) -> Result<String> {
    validate_points(points)?;
    ensure!(
        low.is_finite() && high.is_finite() && low > 0.0 && high > low,
        "invalid target curtains"
    );
    let mut text = String::from("# Independently authored Open Room target curve\nBREAKPOINTS\n");
    for point in points {
        text.push_str(&format!("{:.8} {:.8}\n", point.hz, point.db));
    }
    text.push_str(&format!("HPSLOPEON 0\nLPSLOPEON 0\nHPCUTOFF 15\nLPCUTOFF 17647\nHPORDER 4\nLPORDER 4\nLOWLIMITHZ {low:.8}\nHIGHLIMITHZ {high:.8}\n"));
    Ok(text)
}

/// Parse breakpoint text and curtains, rejecting unknown directives rather than
/// silently discarding high/low-pass settings that this original designer cannot implement.
pub fn import_target(text: &str) -> Result<(Vec<Point>, f64, f64)> {
    ensure!(text.len() <= 1_048_576, "target file exceeds 1 MiB");
    let mut points = Vec::new();
    let mut started = false;
    let mut directives = false;
    let mut low = None;
    let mut high = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line == "BREAKPOINTS" {
            ensure!(!started, "duplicate BREAKPOINTS header");
            started = true;
            continue;
        }
        ensure!(started, "missing BREAKPOINTS header");
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 2, "invalid target line {}", index + 1);
        let value = fields[1]
            .parse::<f64>()
            .with_context(|| format!("invalid target value on line {}", index + 1))?;
        ensure!(value.is_finite(), "non-finite target value");
        if let Ok(hz) = fields[0].parse::<f64>() {
            ensure!(!directives, "breakpoint found after target directives");
            points.push(Point { hz, db: value });
        } else {
            directives = true;
            match fields[0] {
                "LOWLIMITHZ" => {
                    ensure!(low.is_none(), "duplicate low curtain");
                    low = Some(value);
                }
                "HIGHLIMITHZ" => {
                    ensure!(high.is_none(), "duplicate high curtain");
                    high = Some(value);
                }
                "HPSLOPEON" | "LPSLOPEON" => ensure!(
                    value == 0.0,
                    "enabled slope directives are unsupported; disable them before importing"
                ),
                "HPCUTOFF" | "LPCUTOFF" | "HPORDER" | "LPORDER" => {
                    ensure!(value > 0.0, "invalid slope parameter")
                }
                key => bail!("unknown target directive {key}"),
            }
        }
    }
    validate_points(&points)?;
    let low = low.context("target missing LOWLIMITHZ")?;
    let high = high.context("target missing HIGHLIMITHZ")?;
    ensure!(low > 0.0 && high > low, "invalid target curtains");
    Ok((points, low, high))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flat(db: f64) -> Vec<Point> {
        vec![Point { hz: 20.0, db }, Point { hz: 20_000.0, db }]
    }
    fn response(coefficients: &[f32], frequency: f64, sample_rate: u32) -> f64 {
        coefficients
            .iter()
            .enumerate()
            .map(|(i, &h)| {
                Complex::from_polar(
                    h as f64,
                    -2.0 * PI * frequency * i as f64 / sample_rate as f64,
                )
            })
            .sum::<Complex<f64>>()
            .norm()
    }

    #[test]
    fn sweep_has_safe_peak_fades_and_rejects_invalid_values() {
        let mut config = SweepConfig {
            sample_rate: 48_000,
            seconds: 1.0,
            start_hz: 20.0,
            end_hz: 20_000.0,
            level_db: -12.0,
        };
        let signal = sweep(&config).unwrap();
        assert_eq!(signal.len(), 48_000);
        assert_eq!(signal[0], 0.0);
        assert_eq!(signal[signal.len() - 1], 0.0);
        assert!(signal
            .iter()
            .all(|s| s.abs() <= 10.0_f32.powf(-12.0 / 20.0) + 1e-7));
        config.end_hz = 24_000.0;
        assert!(sweep(&config).is_err());
        config.end_hz = 20_000.0;
        config.level_db = f64::NAN;
        assert!(sweep(&config).is_err());
    }

    #[test]
    fn recovers_delayed_gain_and_two_tap_room_response() {
        let rate = 48_000;
        let signal = sweep(&SweepConfig {
            sample_rate: rate,
            seconds: 2.0,
            start_hz: 10.0,
            end_hz: 22_000.0,
            level_db: -12.0,
        })
        .unwrap();
        let delay = 1731;
        let mut recording = vec![0.0; signal.len() + delay + 2000];
        for (i, &x) in signal.iter().enumerate() {
            recording[i + delay] += x * 0.5;
            recording[i + delay + 13] += x * 0.125;
        }
        let measured = analyze(&signal, &recording, rate).unwrap();
        for hz in [50.0, 150.0, 500.0, 1000.0] {
            let expected = Complex::new(0.5, 0.0)
                + Complex::from_polar(0.125, -2.0 * PI * hz * 13.0 / rate as f64);
            let difference = interpolate(&measured, hz).unwrap() - 20.0 * expected.norm().log10();
            assert!(difference.abs() < 0.12, "{hz}: {difference} dB error");
        }
    }

    #[test]
    fn unity_is_delayed_impulse_and_even_filter_is_symmetric() {
        let h = design_filter(&flat(0.0), &flat(0.0), 48_000, 4097, 30.0, 15_000.0, 6.0).unwrap();
        assert!((h[2048] - 1.0).abs() < 1e-6);
        assert!(h
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 2048)
            .all(|(_, x)| x.abs() < 1e-6));
        let even =
            design_filter(&flat(0.0), &flat(0.0), 48_000, 1024, 30.0, 15_000.0, 6.0).unwrap();
        for (a, b) in even.iter().zip(even.iter().rev()) {
            assert!((a - b).abs() < 1e-6);
        }
        assert!((response(&even, 1000.0, 48_000) - 1.0).abs() < 0.001);
    }

    #[test]
    fn cuts_response_and_leaves_outside_curtains_flat() {
        let h = design_filter(&flat(6.0), &flat(0.0), 48_000, 8193, 100.0, 6000.0, 6.0).unwrap();
        for hz in [500.0, 2000.0, 4000.0] {
            assert!((20.0 * response(&h, hz, 48_000).log10() + 6.0).abs() < 0.05);
        }
        for hz in [30.0, 10_000.0, 18_000.0] {
            assert!((20.0 * response(&h, hz, 48_000).log10()).abs() < 0.05);
        }
    }

    #[test]
    fn boost_is_bounded_and_reserves_gain_headroom() {
        let h = design_filter(&flat(-20.0), &flat(0.0), 48_000, 8193, 100.0, 6000.0, 6.0).unwrap();
        let inside = 20.0 * response(&h, 1000.0, 48_000).log10();
        let outside = 20.0 * response(&h, 12_000.0, 48_000).log10();
        assert!(inside <= 0.001 && inside > -0.1);
        assert!((inside - outside - 6.0).abs() < 0.05);
        for i in 0..1000 {
            assert!(response(&h, i as f64 * 24.0, 48_000) <= 1.0001);
        }
    }

    #[test]
    fn target_roundtrip_and_invalid_curves() {
        let text = export_target(&flat(-3.0), 30.0, 15_000.0).unwrap();
        let (points, low, high) = import_target(&text).unwrap();
        assert_eq!((low, high), (30.0, 15_000.0));
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].db, -3.0);
        assert!(import_target(&text.replace("HPSLOPEON 0", "HPSLOPEON 1")).is_err());
        assert!(interpolate(
            &[Point { hz: 100.0, db: 0.0 }, Point { hz: 50.0, db: 0.0 }],
            70.0
        )
        .is_err());
        assert!(analyze(&vec![0.0; 256], &vec![0.0; 256], 48_000).is_err());
    }

    #[test]
    fn calibration_import_and_log_interpolation_preserve_known_response() {
        let text = "\u{feff}\"Sens Factor =-1.2dB, SERNO: 123456\"\n# Calibration response\nFrequency(Hz) Magnitude(dB) Phase(degrees)\n20\t2\t0\n200 -2 90\n2000 2 -90\n20000 0 0 ; tail\n";
        let calibration = import_calibration(text).unwrap();
        let frequency = (20.0_f64 * 200.0).sqrt();
        let measured = vec![
            Point { hz: 20.0, db: 5.0 },
            Point {
                hz: frequency,
                db: 3.0,
            },
            Point { hz: 200.0, db: 1.0 },
        ];
        let corrected = calibrate(&measured, &calibration).unwrap();
        for point in corrected {
            assert!((point.db - 3.0).abs() < 1e-10);
        }
        assert!(calibrate(&flat(0.0), &calibration[1..]).is_err());
    }

    #[test]
    fn calibration_rejects_unknown_formats_and_invalid_data() {
        for text in [
            "20,1\n200,0",
            "unexpected header\n20 0\n200 0",
            "20 0 nan\n200 0 0",
            "200 0\n20 0",
            "20 NaN\n200 0",
            "# no data",
        ] {
            assert!(import_calibration(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn filter_rejects_correction_outside_measured_support() {
        let measured = vec![
            Point { hz: 100.0, db: 3.0 },
            Point {
                hz: 5000.0,
                db: 0.0,
            },
        ];
        assert!(design_filter(&measured, &flat(0.0), 48_000, 4097, 30.0, 500.0, 0.0).is_err());
        assert!(design_filter(&measured, &flat(0.0), 48_000, 4097, 100.0, 10_000.0, 0.0).is_err());
        assert!(design_filter(&measured, &flat(0.0), 48_000, 4097, 100.0, 5000.0, 0.0).is_ok());
    }
}
