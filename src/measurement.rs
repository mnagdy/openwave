//! Capture both speakers at one microphone position, in sequence.
//! The caller supplies playback/capture; this module never opens audio or a network.
use crate::{listening::Placement, session::Measurement};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct PairCapture {
    pub measurements: Vec<Measurement>,
    pub failure: Option<String>,
}

/// Keep completed responses if the next speaker fails. A partial pair must not
/// advance the guide, and cancellation must prevent starting the next speaker.
pub fn capture_pair(
    placement: Placement,
    position: usize,
    cancel: &AtomicBool,
    mut capture: impl FnMut(usize) -> Result<Measurement>,
    mut progress: impl FnMut(usize),
) -> PairCapture {
    let mut result = PairCapture {
        measurements: Vec::with_capacity(2),
        failure: None,
    };
    for channel in 0..2 {
        if cancel.load(Ordering::Relaxed) {
            result.failure = Some(
                "Measurement stopped. Keep the microphone here and retry this position.".into(),
            );
            break;
        }
        progress(channel);
        match capture(channel) {
            Ok(mut measurement) => {
                measurement.channel = channel;
                measurement.position = position;
                measurement.placement = Some(placement);
                result.measurements.push(measurement);
            }
            Err(error) => {
                let speaker = if channel == 0 { "Left" } else { "Right" };
                result.failure = Some(format!("{speaker} speaker measurement failed: {error:#}. Keep the microphone here and retry this position."));
                break;
            }
        }
    }
    result
}

pub fn position_complete(measurements: &[Measurement], placement: Placement) -> bool {
    (0..2).all(|channel| {
        measurements
            .iter()
            .any(|m| m.channel == channel && m.placement == Some(placement))
    })
}

/// Both responses use the same new take index, even when old channel counts differ.
pub fn next_take(measurements: &[Measurement]) -> usize {
    measurements
        .iter()
        .filter(|m| m.channel < 2)
        .map(|m| m.position)
        .max()
        .map_or(0, |p| p + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(channel: usize, position: usize, placement: Option<Placement>) -> Measurement {
        Measurement {
            channel,
            position,
            placement,
            points: vec![],
            source: "simulated".into(),
            included: true,
            calibration_applied: false,
        }
    }
    #[test]
    fn one_action_captures_left_then_right_at_same_position() {
        let mut order = Vec::new();
        let mut progress = Vec::new();
        let pair = capture_pair(
            Placement::BackRightLow,
            7,
            &AtomicBool::new(false),
            |ch| {
                order.push(ch);
                Ok(response(99, 99, None))
            },
            |ch| progress.push(ch),
        );
        assert_eq!(order, [0, 1]);
        assert_eq!(progress, [0, 1]);
        assert!(pair.failure.is_none());
        assert!(position_complete(
            &pair.measurements,
            Placement::BackRightLow
        ));
        assert!(pair.measurements.iter().all(|m| m.position == 7));
        assert_eq!(pair.measurements[0].channel, 0);
        assert_eq!(pair.measurements[1].channel, 1);
    }
    #[test]
    fn failed_right_preserves_left_but_does_not_complete_position() {
        let pair = capture_pair(
            Placement::Main,
            0,
            &AtomicBool::new(false),
            |ch| {
                if ch == 1 {
                    anyhow::bail!("microphone disconnected");
                }
                Ok(response(ch, 0, None))
            },
            |_| {},
        );
        assert_eq!(pair.measurements.len(), 1);
        assert!(pair.failure.unwrap().contains("Right speaker"));
        assert!(!position_complete(&pair.measurements, Placement::Main));
    }
    #[test]
    fn failed_left_never_starts_right() {
        let mut calls = Vec::new();
        let pair = capture_pair(
            Placement::Main,
            0,
            &AtomicBool::new(false),
            |ch| {
                calls.push(ch);
                anyhow::bail!("clipped")
            },
            |_| {},
        );
        assert_eq!(calls, [0]);
        assert!(pair.measurements.is_empty());
        assert!(pair.failure.is_some());
    }
    #[test]
    fn cancellation_between_speakers_preserves_left_and_skips_right() {
        let cancel = AtomicBool::new(false);
        let pair = capture_pair(
            Placement::Main,
            0,
            &cancel,
            |ch| {
                cancel.store(true, Ordering::Relaxed);
                Ok(response(ch, 0, None))
            },
            |_| {},
        );
        assert_eq!(pair.measurements.len(), 1);
        assert!(pair.failure.is_some());
    }
    #[test]
    fn cancelled_pair_never_starts_playback() {
        let pair = capture_pair(
            Placement::Main,
            0,
            &AtomicBool::new(true),
            |_| panic!("must not play"),
            |_| panic!("must not report starting"),
        );
        assert!(pair.measurements.is_empty());
        assert!(pair.failure.is_some());
    }
    #[test]
    fn unassigned_imports_do_not_complete_positions_and_take_indexes_stay_unique() {
        let old = vec![response(0, 9, None), response(1, 4, None)];
        assert!(!position_complete(&old, Placement::Main));
        assert_eq!(next_take(&old), 10);
    }
}
