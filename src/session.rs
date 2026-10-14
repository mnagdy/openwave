use crate::{
    dsp::{self, Point},
    project_import,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub channel: usize,
    pub position: usize,
    pub points: Vec<Point>,
    pub source: String,
    pub included: bool,
    #[serde(default)]
    pub calibration_applied: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub name: String,
    pub sample_rate: u32,
    pub target: Vec<Point>,
    pub low_hz: f64,
    pub high_hz: f64,
    pub max_boost_db: f64,
    pub measurements: Vec<Measurement>,
    pub calibration: Vec<Point>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            name: "My listening room".into(),
            sample_rate: 48000,
            target: vec![
                Point { hz: 20., db: 3. },
                Point { hz: 80., db: 3. },
                Point { hz: 200., db: 0. },
                Point { hz: 1000., db: 0. },
                Point {
                    hz: 10000.,
                    db: -2.,
                },
                Point {
                    hz: 20000.,
                    db: -4.,
                },
            ],
            low_hz: 30.,
            high_hz: 500.,
            max_boost_db: 0.,
            measurements: vec![],
            calibration: vec![],
        }
    }
}
impl Session {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.version == 1, "Unsupported OpenRoom session version");
        anyhow::ensure!(
            (8000..=192000).contains(&self.sample_rate),
            "Invalid sample rate"
        );
        anyhow::ensure!(
            self.low_hz.is_finite()
                && self.high_hz.is_finite()
                && self.low_hz > 0.
                && self.high_hz > self.low_hz
                && self.high_hz < self.sample_rate as f64 / 2.,
            "Invalid correction range"
        );
        anyhow::ensure!(
            self.max_boost_db.is_finite() && (0. ..=12.).contains(&self.max_boost_db),
            "Invalid boost limit"
        );
        anyhow::ensure!(
            self.target.len() <= 16384
                && self.calibration.len() <= 16384
                && self.measurements.len() <= 2048,
            "Session is too large"
        );
        dsp::interpolate(&self.target, 1000.)?;
        if !self.calibration.is_empty() {
            dsp::interpolate(&self.calibration, 1000.)?;
        }
        for m in &self.measurements {
            anyhow::ensure!(
                m.channel < 64 && m.position < 2048 && m.points.len() <= 16384,
                "Invalid measurement dimensions"
            );
            dsp::interpolate(&m.points, 1000.)?;
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        anyhow::ensure!(
            std::fs::metadata(path)?.len() <= 128 * 1024 * 1024,
            "Session is too large"
        );
        let s: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        s.validate()?;
        Ok(s)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
    pub fn from_dirac(path: &Path) -> Result<Self> {
        let summary = project_import::inspect(path)?;
        let responses = project_import::import_responses(path)?;
        let mut s = Self {
            name: summary.name,
            sample_rate: summary.sample_rate.unwrap_or(48000),
            ..Self::default()
        };
        s.measurements = responses
            .into_iter()
            .map(|r| Measurement {
                channel: r.channel,
                position: r.position,
                points: r.points,
                source: "Imported Dirac measured-response graph".into(),
                included: true,
                calibration_applied: true,
            })
            .collect();
        s.validate()?;
        Ok(s)
    }
    /// Each response is normalized to its log-frequency mean over 500–2000 Hz.
    /// Average power after normalization so arbitrary microphone gain is not SPL.
    pub fn average(&self, channel: usize) -> Result<Vec<Point>> {
        anyhow::ensure!(
            (8000..=192000).contains(&self.sample_rate),
            "Invalid sample rate"
        );
        let chosen: Vec<_> = self
            .measurements
            .iter()
            .filter(|m| m.included && m.channel == channel)
            .collect();
        anyhow::ensure!(
            !chosen.is_empty(),
            "No included measurements for this speaker"
        );
        let mut normalized = Vec::new();
        for m in chosen {
            let points = if self.calibration.is_empty() || m.calibration_applied {
                m.points.clone()
            } else {
                dsp::calibrate(&m.points, &self.calibration)?
            };
            dsp::interpolate(&points, 1000.)?;
            anyhow::ensure!(
                points[0].hz <= 500. && points[points.len() - 1].hz >= 2000.,
                "Each included response must cover 500–2000 Hz for level normalization"
            );
            let base = (0..=24)
                .map(|i| at(&points, 500. * 4f64.powf(i as f64 / 24.)))
                .collect::<Vec<_>>();
            let offset = base.iter().sum::<f64>() / base.len() as f64;
            normalized.push(
                points
                    .into_iter()
                    .map(|p| Point {
                        hz: p.hz,
                        db: p.db - offset,
                    })
                    .collect::<Vec<_>>(),
            );
        }
        // Do not invent measured response by holding endpoints outside any capture.
        let low = normalized.iter().map(|p| p[0].hz).fold(20_f64, f64::max);
        let high = normalized
            .iter()
            .map(|p| p[p.len() - 1].hz)
            .fold(20000_f64.min(self.sample_rate as f64 * 0.45), f64::min);
        anyhow::ensure!(
            high > low,
            "Included responses have no common measured frequency range"
        );
        let mut out = Vec::new();
        for i in 0..=240 {
            let hz = low * (high / low).powf(i as f64 / 240.);
            let mut power = 0.;
            for p in &normalized {
                power += 10f64.powf(at(p, hz) / 10.);
            }
            out.push(Point {
                hz,
                db: 10. * (power / normalized.len() as f64).log10(),
            });
        }
        Ok(out)
    }
    pub fn demo() -> Self {
        let mut s = Self {
            name: "Demonstration — synthetic room".into(),
            ..Self::default()
        };
        for ch in 0..2 {
            for pos in 0..3 {
                let mut points = Vec::new();
                for i in 0..=240 {
                    let hz = 20. * 1000f64.powf(i as f64 / 240.);
                    let x = hz.log2();
                    let db = 7. * (-((x - 6.0 - ch as f64 * 0.15) / 0.24).powi(2)).exp()
                        - 5. * (-((x - 7.9) / 0.35).powi(2)).exp()
                        + 1.3 * (x * 3. + pos as f64).sin()
                        - 0.4 * (x - 10.).max(0.);
                    points.push(Point { hz, db });
                }
                s.measurements.push(Measurement {
                    channel: ch,
                    position: pos,
                    points,
                    source: "Synthetic demonstration".into(),
                    included: true,
                    calibration_applied: true,
                });
            }
        }
        s
    }
}
fn at(p: &[Point], hz: f64) -> f64 {
    let i = p.partition_point(|p| p.hz < hz);
    if i == 0 {
        return p[0].db;
    }
    if i == p.len() {
        return p[p.len() - 1].db;
    }
    let a = &p[i - 1];
    let b = &p[i];
    a.db + (b.db - a.db) * (hz / a.hz).ln() / (b.hz / a.hz).ln()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_input_gain_before_power_average() {
        let mut s = Session::default();
        for db in [-20., 10.] {
            s.measurements.push(Measurement {
                channel: 0,
                position: s.measurements.len(),
                points: vec![Point { hz: 20., db }, Point { hz: 20000., db }],
                source: "test".into(),
                included: true,
                calibration_applied: false,
            });
        }
        let p = s.average(0).unwrap();
        assert!(p.iter().all(|p| p.db.abs() < 1e-8));
    }
    #[test]
    fn rejected_sessions_and_exclusions() {
        let mut s = Session::demo();
        s.version = 8;
        assert!(s.validate().is_err());
        s.version = 1;
        for m in &mut s.measurements {
            m.included = false;
        }
        assert!(s.average(0).is_err());
    }
    #[test]
    fn averages_only_measured_overlap_and_rejects_missing_normalization_band() {
        let mut s = Session::default();
        for (low, high) in [(20., 15000.), (100., 20000.)] {
            s.measurements.push(Measurement {
                channel: 0,
                position: s.measurements.len(),
                points: vec![Point { hz: low, db: 0. }, Point { hz: high, db: 0. }],
                source: "test".into(),
                included: true,
                calibration_applied: false,
            });
        }
        let p = s.average(0).unwrap();
        assert!((p[0].hz - 100.).abs() < 1e-8);
        assert!((p[p.len() - 1].hz - 15000.).abs() < 1e-8);
        s.measurements[0].points[0].hz = 1000.;
        assert!(s.average(0).is_err());
    }
    #[test]
    fn calibrates_raw_capture_once_and_preserves_imported_graph() {
        let mut s = Session {
            calibration: vec![
                Point { hz: 20., db: 6. },
                Point { hz: 500., db: 0. },
                Point { hz: 2000., db: 0. },
                Point { hz: 20000., db: 0. },
            ],
            ..Session::default()
        };
        s.measurements.push(Measurement {
            channel: 0,
            position: 0,
            points: s.calibration.clone(),
            source: "Raw microphone response".into(),
            included: true,
            calibration_applied: false,
        });
        s.measurements.push(Measurement {
            channel: 0,
            position: 1,
            points: vec![Point { hz: 20., db: 0. }, Point { hz: 20000., db: 0. }],
            source: "Existing processed graph".into(),
            included: true,
            calibration_applied: true,
        });
        let p = s.average(0).unwrap();
        assert!(p.iter().all(|p| p.db.abs() < 1e-8));
    }
}
