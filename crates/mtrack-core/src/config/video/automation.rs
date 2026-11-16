//! Clip-local automation, independent of renderer and controller input.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Automation = BTreeMap<String, AutomationLane>;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationLane {
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub points: Vec<AutomationPoint>,
}
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationPoint {
    pub time: f64,
    pub value: f64,
    /// Curvature of the outgoing segment: -1..1, zero is linear.
    #[serde(default)]
    pub curve: f64,
    #[serde(default)]
    pub hold: bool,
}
impl AutomationLane {
    pub fn value_at(&self, time: f64) -> Option<f64> {
        if !self.enabled {
            return None;
        }
        let first = self.points.first()?;
        if time <= first.time {
            return Some(first.value);
        }
        for pair in self.points.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if time < b.time {
                if a.hold {
                    return Some(a.value);
                }
                let t = (time - a.time) / (b.time - a.time);
                let k = a.curve * 6.0;
                let t = if k.abs() < 1e-8 {
                    t
                } else {
                    (k * t).exp_m1() / k.exp_m1()
                };
                return Some(a.value + (b.value - a.value) * t);
            }
        }
        self.points.last().map(|p| p.value)
    }
}
/// Bounds for visual properties. Paths use JSON pointer syntax.
pub fn property_bounds(
    path: &str,
    masks: &[super::VideoMask],
    duration: f64,
) -> Option<(f64, f64)> {
    match path {
        "/x" | "/y" => Some((-4096.0, 4096.0)),
        "/width" | "/height" => Some((1.0, 4096.0)),
        "/opacity" => Some((0.0, 1.0)),
        "/fade_in" | "/fade_out" => Some((0.0, duration)),
        _ => {
            let parts: Vec<_> = path.split('/').collect();
            if parts.len() < 4 || parts[1] != "masks" {
                return None;
            }
            let mask = masks.get(parts[2].parse::<usize>().ok()?)?;
            if parts.len() == 4 {
                match parts[3] {
                    "x" | "y" | "opacity" | "invert" => Some((0.0, 1.0)),
                    "width" | "height" => Some((0.000001, 1.0)),
                    "feather" => Some((0.0, 4096.0)),
                    _ => None,
                }
            } else if parts.len() == 6 && parts[3] == "points" && matches!(parts[5], "x" | "y") {
                mask.points.get(parts[4].parse::<usize>().ok()?)?;
                Some((0.0, 1.0))
            } else {
                None
            }
        }
    }
}
pub fn validate(
    lanes: &Automation,
    masks: &[super::VideoMask],
    duration: f64,
) -> Result<(), String> {
    if lanes.len() > 256 || lanes.values().map(|l| l.points.len()).sum::<usize>() > 8192 {
        return Err("automation exceeds 256 lanes or 8192 points".into());
    }
    for (path, lane) in lanes {
        let (min, max) = property_bounds(path, masks, duration)
            .ok_or_else(|| format!("unknown automation property {path}"))?;
        let mut previous = -1.0;
        for p in &lane.points {
            if !p.time.is_finite()
                || p.time < 0.0
                || p.time > duration
                || p.time <= previous
                || !p.value.is_finite()
                || !(min..=max).contains(&p.value)
                || !p.curve.is_finite()
                || !(-1.0..=1.0).contains(&p.curve)
            {
                return Err(format!("invalid automation point for {path}"));
            }
            previous = p.time;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lane() -> AutomationLane {
        serde_json::from_str(r#"{"points":[{"time":0,"value":0},{"time":10,"value":1}]}"#).unwrap()
    }
    #[test]
    fn video_automation_curves_hold_and_random_access() {
        let mut l = lane();
        assert_eq!(l.value_at(5.0), Some(0.5));
        assert_eq!(l.value_at(-1.0), Some(0.0));
        assert_eq!(l.value_at(20.0), Some(1.0));
        l.points[0].curve = 1.0;
        assert!(l.value_at(5.0).unwrap() < 0.1);
        l.points[0].curve = -1.0;
        assert!(l.value_at(5.0).unwrap() > 0.9);
        l.points[0].hold = true;
        assert_eq!(l.value_at(9.99), Some(0.0));
        assert_eq!(l.value_at(10.0), Some(1.0));
        l.enabled = false;
        assert_eq!(l.value_at(5.0), None);
    }
    #[test]
    fn video_automation_rejects_duplicates_unknown_paths_and_out_of_range() {
        let mut lanes = Automation::from([("/opacity".into(), lane())]);
        assert!(validate(&lanes, &[], 10.0).is_ok());
        assert!(validate(&lanes, &[], 9.0).is_err());
        lanes.get_mut("/opacity").unwrap().points[1].time = 0.0;
        assert!(validate(&lanes, &[], 10.0).is_err());
        assert!(validate(&Automation::from([("/file".into(), lane())]), &[], 10.0).is_err());
    }
}
