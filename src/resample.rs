//! Offline band-limited resampling of a finite measurement reference.
//! Frequency bins above the output Nyquist frequency are discarded. Padding to
//! an exact rational-rate period preserves physical frequency and sample times;
//! the returned duration is rounded to the nearest output sample. Finite signals
//! should have quiet ends (as measurement sweeps do) to limit Fourier edge ringing.
//! This is sample-rate conversion, not compensation for hardware clock drift.
use anyhow::{bail, ensure, Result};
use rustfft::{num_complex::Complex, FftPlanner};

const MAX_FFT_POINTS: usize = 4_194_304;
const MAX_SECONDS: u64 = 180;

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Resample a finite reference at exact physical sample times. Supported rates
/// are 8–192 kHz; each working FFT is limited to 4,194,304 points and the reference
/// to 180 seconds. Quiet signal ends avoid the periodic Fourier boundary causing
/// ringing. Padding repeats the last sample; it does not change the return length.
pub fn resample_reference(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    ensure!(
        (8_000..=192_000).contains(&from_rate) && (8_000..=192_000).contains(&to_rate),
        "Reference sample rates must be between 8000 and 192000 Hz"
    );
    ensure!(
        !samples.is_empty() && samples.len() <= MAX_FFT_POINTS,
        "Reference must contain 1–{MAX_FFT_POINTS} samples"
    );
    ensure!(
        samples.len() as u64 <= MAX_SECONDS * from_rate as u64,
        "Reference exceeds the 180-second duration limit"
    );
    ensure!(
        samples.iter().all(|s| s.is_finite()),
        "Reference contains non-finite samples"
    );
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let common = gcd(from_rate, to_rate);
    let denominator = (from_rate / common) as usize;
    let numerator = (to_rate / common) as usize;
    let blocks = samples.len().div_ceil(denominator);
    let input_len = blocks
        .checked_mul(denominator)
        .ok_or_else(|| anyhow::anyhow!("Reference length overflow"))?;
    let output_len = blocks
        .checked_mul(numerator)
        .ok_or_else(|| anyhow::anyhow!("Resampled reference length overflow"))?;
    ensure!(
        input_len <= MAX_FFT_POINTS && output_len <= MAX_FFT_POINTS,
        "Resampled reference exceeds the {MAX_FFT_POINTS}-point working FFT limit"
    );
    let returned_len = ((samples.len() as u64 * to_rate as u64 + from_rate as u64 / 2)
        / from_rate as u64) as usize;
    ensure!(
        returned_len > 0,
        "Reference is shorter than one output sample"
    );
    let last = *samples.last().unwrap() as f64;
    let mut source = vec![Complex::new(last, 0.0); input_len];
    for (slot, sample) in source.iter_mut().zip(samples) {
        slot.re = *sample as f64;
    }
    let mut planner = FftPlanner::<f64>::new();
    planner.plan_fft_forward(input_len).process(&mut source);
    let mut destination = spectrum_at_length(&source, output_len);
    drop(source);
    planner
        .plan_fft_inverse(output_len)
        .process(&mut destination);
    let scale = 1.0 / input_len as f64;
    let mut result = Vec::with_capacity(returned_len);
    for value in destination.into_iter().take(returned_len) {
        let sample = (value.re * scale) as f32;
        if !sample.is_finite() {
            bail!("Resampling produced a non-finite sample");
        }
        result.push(sample);
    }
    Ok(result)
}

/// Transfer a real signal's conjugate spectrum. An even-length source's single
/// Nyquist bin is split on upsampling; on downsampling the two source bins that
/// become the destination Nyquist bin are summed. Odd lengths have no such bin.
fn spectrum_at_length(source: &[Complex<f64>], output_len: usize) -> Vec<Complex<f64>> {
    let input_len = source.len();
    let mut out = vec![Complex::new(0.0, 0.0); output_len];
    out[0] = source[0];
    let shared = input_len.min(output_len);
    for bin in 1..=(shared - 1) / 2 {
        out[bin] = source[bin];
        out[output_len - bin] = source[input_len - bin];
    }
    if shared.is_multiple_of(2) {
        let bin = shared / 2;
        if output_len > input_len {
            let half = Complex::new(source[bin].re * 0.5, 0.0);
            out[bin] = half;
            out[output_len - bin] = half;
        } else if output_len < input_len {
            out[bin] = Complex::new((source[bin] + source[input_len - bin]).re, 0.0);
        } else {
            out[bin] = source[bin];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{PI, TAU};
    fn tone(len: usize, rate: u32, hz: f64, amplitude: f64, phase: f64) -> Vec<f32> {
        (0..len)
            .map(|i| (amplitude * (TAU * hz * i as f64 / rate as f64 + phase).cos()) as f32)
            .collect()
    }
    fn max_error(actual: &[f32], expected: &[f32]) -> f64 {
        assert_eq!(actual.len(), expected.len());
        actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (*a as f64 - *b as f64).abs())
            .fold(0.0, f64::max)
    }
    #[test]
    fn preserves_physical_frequency_amplitude_and_phase() {
        let input = tone(48_000, 48_000, 997.0, 0.125, 0.37);
        for rate in [8_000, 16_000, 44_100, 96_000, 192_000] {
            let output = resample_reference(&input, 48_000, rate).unwrap();
            let expected = tone(rate as usize, rate, 997.0, 0.125, 0.37);
            assert!(max_error(&output, &expected) < 0.000_001, "rate {rate}");
        }
    }
    #[test]
    fn rejects_above_nyquist_energy_in_downsampling() {
        let input = tone(48_000, 48_000, 12_000.0, 0.25, 0.27);
        let output = resample_reference(&input, 48_000, 16_000).unwrap();
        let rms =
            (output.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / output.len() as f64).sqrt();
        assert!(rms < 0.000_001, "alias RMS {rms}");
    }
    #[test]
    fn dc_and_rounded_duration_survive_padding() {
        for len in [1, 7, 8, 101, 160, 161, 999] {
            let input = vec![0.375; len];
            for rate in [16_000, 44_100, 96_000] {
                let output = resample_reference(&input, 48_000, rate);
                let expected_len = (len * rate as usize + 24_000) / 48_000;
                if expected_len == 0 {
                    assert!(output.is_err());
                    continue;
                }
                let output = output.unwrap();
                assert_eq!(output.len(), expected_len);
                assert!(max_error(&output, &vec![0.375; expected_len]) < 0.000_001);
            }
        }
    }
    #[test]
    fn even_source_nyquist_splits_correctly_on_upsampling() {
        let input = tone(8, 8_000, 4_000.0, 0.5, 0.0);
        let output = resample_reference(&input, 8_000, 12_000).unwrap();
        assert!(max_error(&output, &tone(12, 12_000, 4_000.0, 0.5, 0.0)) < 0.000_001);
    }
    #[test]
    fn odd_source_bins_merge_at_even_destination_nyquist() {
        let input = tone(9, 12_000, 4_000.0, 0.5, 0.0);
        let output = resample_reference(&input, 12_000, 8_000).unwrap();
        assert!(max_error(&output, &tone(6, 8_000, 4_000.0, 0.5, 0.0)) < 0.000_001);
    }
    #[test]
    fn even_to_odd_discards_source_nyquist_without_aliasing() {
        let input: Vec<_> = (0..8)
            .map(|i| (0.5 * (PI * i as f64).cos()) as f32)
            .collect();
        let output = resample_reference(&input, 16_000, 14_000).unwrap();
        assert_eq!(output.len(), 7);
        assert!(output.iter().all(|s| s.abs() < 0.000_001));
        let input = tone(7, 14_000, 6_000.0, 0.5, 0.0);
        let output = resample_reference(&input, 14_000, 16_000).unwrap();
        assert!(max_error(&output, &tone(8, 16_000, 6_000.0, 0.5, 0.0)) < 0.000_001);
    }
    #[test]
    fn validates_input_and_working_size_before_transform() {
        assert!(resample_reference(&[], 48_000, 44_100).is_err());
        assert!(resample_reference(&[0.0], 0, 48_000).is_err());
        assert!(resample_reference(&[0.0], 48_000, 192_001).is_err());
        assert!(resample_reference(&[f32::NAN], 48_000, 44_100).is_err());
        assert!(resample_reference(&[f32::INFINITY], 48_000, 48_000).is_err());
        assert_eq!(
            resample_reference(&[0.0, -0.25, 0.5], 48_000, 48_000).unwrap(),
            [0.0, -0.25, 0.5]
        );
        assert!(resample_reference(&vec![0.0; 180 * 8_000 + 1], 8_000, 16_000).is_err());
        assert!(resample_reference(&vec![0.0; 200_000], 8_000, 192_000).is_err());
    }
}
