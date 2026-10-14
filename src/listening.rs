//! Original listening-position guide and target presets. These are practical
//! starting points, not Dirac algorithms or predictions of the sound achieved.
use crate::dsp::Point;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    Main,
    FrontLeftHigh,
    FrontCenterHigh,
    FrontRightHigh,
    BackLeftHigh,
    BackCenterHigh,
    BackRightHigh,
    FrontLeftLow,
    FrontCenterLow,
    FrontRightLow,
    BackLeftLow,
    BackCenterLow,
    BackRightLow,
}
impl Placement {
    pub const ALL: [Self; 13] = [
        Self::Main,
        Self::FrontLeftHigh,
        Self::FrontCenterHigh,
        Self::FrontRightHigh,
        Self::BackLeftHigh,
        Self::BackCenterHigh,
        Self::BackRightHigh,
        Self::FrontLeftLow,
        Self::FrontCenterLow,
        Self::FrontRightLow,
        Self::BackLeftLow,
        Self::BackCenterLow,
        Self::BackRightLow,
    ];
    pub fn number(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap() + 1
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "Main listening position",
            Self::FrontLeftHigh => "Front · left · high",
            Self::FrontCenterHigh => "Front · centre · high",
            Self::FrontRightHigh => "Front · right · high",
            Self::BackLeftHigh => "Back · left · high",
            Self::BackCenterHigh => "Back · centre · high",
            Self::BackRightHigh => "Back · right · high",
            Self::FrontLeftLow => "Front · left · low",
            Self::FrontCenterLow => "Front · centre · low",
            Self::FrontRightLow => "Front · right · low",
            Self::BackLeftLow => "Back · left · low",
            Self::BackCenterLow => "Back · centre · low",
            Self::BackRightLow => "Back · right · low",
        }
    }
    /// Left/right, front/back, and height, relative to the listener facing speakers.
    pub fn axes(self) -> [f32; 3] {
        if self == Self::Main {
            return [0., 0., 0.];
        }
        let index = self.number() - 2;
        let x = (index % 3) as f32 - 1.;
        let y = if index % 6 < 3 { -1. } else { 1. };
        let z = if index < 6 { 1. } else { -1. };
        [x, y, z]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ListeningArea {
    pub width_cm: f32,
    pub depth_cm: f32,
    pub height_cm: f32,
}
impl Default for ListeningArea {
    fn default() -> Self {
        Self {
            width_cm: 180.,
            depth_cm: 40.,
            height_cm: 30.,
        }
    }
}
impl ListeningArea {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.width_cm.is_finite() && (100. ..=400.).contains(&self.width_cm),
            "Listening width must be 100–400 cm"
        );
        anyhow::ensure!(
            self.depth_cm.is_finite() && (30. ..=100.).contains(&self.depth_cm),
            "Front/back offset must be 30–100 cm"
        );
        anyhow::ensure!(
            self.height_cm.is_finite() && (15. ..=60.).contains(&self.height_cm),
            "Height offset must be 15–60 cm"
        );
        Ok(())
    }
    pub fn offsets(&self, p: Placement) -> [f32; 3] {
        let [x, y, z] = p.axes();
        [
            x * self.width_cm / 2.,
            y * self.depth_cm,
            z * self.height_cm,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurvePreset {
    Balanced,
    Warm,
    BassLift,
    ReducedBass,
    Flat,
}
impl CurvePreset {
    pub const ALL: [Self; 5] = [
        Self::Balanced,
        Self::Warm,
        Self::BassLift,
        Self::ReducedBass,
        Self::Flat,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Warm => "Warm & gentle",
            Self::BassLift => "More bass",
            Self::ReducedBass => "Less bass",
            Self::Flat => "Flat reference",
        }
    }
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Balanced => "A little bass warmth with a gentle treble slope. A useful starting point for everyday listening.",
            Self::Warm => "Fuller low notes and softer high notes. Try this if recordings feel thin or bright.",
            Self::BassLift => "A stronger low-frequency target for extra weight in music and films.",
            Self::ReducedBass => "A lower bass target if the sound feels heavy. This does not remove deep room cancellations.",
            Self::Flat => "An even target for comparison. Flat in-room sound may feel brighter than a sloping target.",
        }
    }
    pub fn levels(self) -> [f64; 7] {
        match self {
            Self::Balanced => [3., 3., 0., 0., -1., -2., -4.],
            Self::Warm => [4., 4., 1., 0., -2., -4., -6.],
            Self::BassLift => [6., 6., 1., 0., -1., -2., -4.],
            Self::ReducedBass => [-3., -3., -1., 0., -1., -2., -4.],
            Self::Flat => [0.; 7],
        }
    }
    pub fn points(self) -> Vec<Point> {
        [20., 80., 200., 1000., 4000., 10000., 20000.]
            .into_iter()
            .zip(self.levels())
            .map(|(hz, db)| Point { hz, db })
            .collect()
    }
    pub fn matches(self, points: &[Point]) -> bool {
        let expected = self.points();
        points.len() == expected.len()
            && points
                .iter()
                .zip(expected)
                .all(|(a, b)| (a.hz - b.hz).abs() < 1e-6 && (a.db - b.db).abs() < 1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sofa_plan_spans_width_depth_and_height_with_unique_points() {
        let area = ListeningArea::default();
        area.validate().unwrap();
        let offsets: Vec<_> = Placement::ALL
            .into_iter()
            .map(|p| area.offsets(p))
            .collect();
        assert_eq!(offsets[0], [0.; 3]);
        for (i, a) in offsets.iter().enumerate() {
            for b in &offsets[i + 1..] {
                let distance = a
                    .iter()
                    .zip(b)
                    .map(|(x, y)| (x - y).powi(2))
                    .sum::<f32>()
                    .sqrt();
                assert!(distance >= 30., "Points must not cluster: {a:?}, {b:?}");
            }
        }
        assert_eq!(area.offsets(Placement::FrontLeftHigh), [-90., -40., 30.]);
        assert_eq!(area.offsets(Placement::BackRightLow), [90., 40., -30.]);
    }
    #[test]
    fn presets_round_trip_through_the_real_target_export() {
        for preset in CurvePreset::ALL {
            let points = preset.points();
            let text = crate::dsp::export_target(&points, 30., 500.).unwrap();
            let (imported, low, high) = crate::dsp::import_target(&text).unwrap();
            assert!(preset.matches(&imported));
            assert_eq!((low, high), (30., 500.));
            let session = crate::session::Session {
                target: points,
                ..Default::default()
            };
            session.validate().unwrap();
            let filter = crate::dsp::design_filter(
                &session_demo_average(),
                &session.target,
                48000,
                4097,
                30.,
                500.,
                0.,
            )
            .unwrap();
            assert!(filter.iter().all(|v| v.is_finite()));
        }
    }
    fn session_demo_average() -> Vec<Point> {
        crate::session::Session::demo().average(0).unwrap()
    }
}
