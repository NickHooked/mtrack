// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
// SPDX-License-Identifier: GPL-3.0-only
//! Saved live-performance deck. Media remains in the validated video timeline.
use super::VideoConfig;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub const MAX_OUTPUTS: usize = 8;
pub const MAX_SLICES: usize = 16;
pub const MAX_OUTPUT_PIXELS: u64 = 16_777_216;
fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
fn bpm() -> f64 {
    120.0
}
fn columns() -> usize {
    8
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct VjConfig {
    pub bpm: f64,
    /// Zero = immediate, otherwise number of quarter-note beats.
    pub quantize: f64,
    pub columns: usize,
    pub grid: Vec<Vec<Option<String>>>,
    pub layers: Vec<LayerSettings>,
    pub clips: BTreeMap<String, ClipSettings>,
    pub outputs: Vec<OutputCanvas>,
    pub midi: Vec<MidiMapping>,
}
impl Default for VjConfig {
    fn default() -> Self {
        Self {
            bpm: bpm(),
            quantize: 0.0,
            columns: columns(),
            grid: vec![],
            layers: vec![],
            clips: BTreeMap::new(),
            outputs: vec![],
            midi: vec![],
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerSettings {
    pub opacity: f64,
    pub mute: bool,
    pub solo: bool,
    pub blend: BlendMode,
    pub crossfader: CrossfaderSide,
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub invert: f64,
}
impl Default for LayerSettings {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            mute: false,
            solo: false,
            blend: BlendMode::Normal,
            crossfader: CrossfaderSide::None,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            invert: 0.0,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
    Multiply,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CrossfaderSide {
    #[default]
    None,
    A,
    B,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    #[default]
    Restart,
    Toggle,
    Piano,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipSettings {
    pub trigger: TriggerMode,
    pub looping: bool,
    pub auto_next: bool,
}
impl Default for ClipSettings {
    fn default() -> Self {
        Self {
            trigger: TriggerMode::Restart,
            looping: true,
            auto_next: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Default for InputRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}
fn corners() -> [Point; 4] {
    [
        Point { x: 0.0, y: 0.0 },
        Point { x: 1.0, y: 0.0 },
        Point { x: 1.0, y: 1.0 },
        Point { x: 0.0, y: 1.0 },
    ]
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputSource {
    #[default]
    Composition,
    Layer {
        index: usize,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSlice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source: OutputSource,
    #[serde(default)]
    pub input: InputRect,
    /// Clockwise TL, TR, BR, BL, normalized to the output canvas.
    #[serde(default = "corners")]
    pub corners: [Point; 4],
    /// Left, top, right, bottom feather widths in local slice coordinates.
    #[serde(default)]
    pub feather: [f64; 4],
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputCanvas {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default = "one")]
    pub gamma: f64,
    #[serde(default = "one")]
    pub brightness: f64,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default)]
    pub test_pattern: bool,
    #[serde(default)]
    pub slices: Vec<OutputSlice>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlTarget {
    Launch { layer: usize, column: usize },
    Column { column: usize },
    StopLayer { layer: usize },
    StopAll,
    LayerOpacity { layer: usize },
    LayerMute { layer: usize },
    LayerSolo { layer: usize },
    MasterOpacity,
    Crossfader,
    Blackout,
    Freeze,
    TapTempo,
}
impl ControlTarget {
    pub fn is_continuous(&self) -> bool {
        matches!(
            self,
            Self::LayerOpacity { .. } | Self::MasterOpacity | Self::Crossfader
        )
    }
    pub fn validate(&self, layers: usize, columns: usize) -> bool {
        match *self {
            Self::Launch { layer, column } => layer < layers && column < columns,
            Self::Column { column } => column < columns,
            Self::StopLayer { layer }
            | Self::LayerOpacity { layer }
            | Self::LayerMute { layer }
            | Self::LayerSolo { layer } => layer < layers,
            _ => true,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MidiKind {
    Note,
    Cc,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MidiMode {
    #[default]
    Absolute,
    Toggle,
    Momentary,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidiMapping {
    pub id: String,
    /// Empty matches any input; `host` matches the player's configured MIDI input.
    #[serde(default)]
    pub device: String,
    pub channel: u8,
    pub kind: MidiKind,
    pub number: u8,
    pub target: ControlTarget,
    #[serde(default)]
    pub mode: MidiMode,
    #[serde(default)]
    pub min: f64,
    #[serde(default = "one")]
    pub max: f64,
}
fn range(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && (lo..=hi).contains(&v)
}
fn label(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 128
}
/// Reject folded/degenerate quads before inversion. Off-canvas corners are valid.
pub fn valid_quad(p: &[Point; 4]) -> bool {
    if p.iter()
        .any(|p| !range(p.x, -2.0, 3.0) || !range(p.y, -2.0, 3.0))
    {
        return false;
    }
    let mut sign = 0.0f64;
    for i in 0..4 {
        let a = p[i];
        let b = p[(i + 1) % 4];
        let c = p[(i + 2) % 4];
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if cross.abs() < 1e-6 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}
impl VjConfig {
    pub fn clip_at<'a>(
        &'a self,
        config: &'a VideoConfig,
        layer: usize,
        column: usize,
    ) -> Option<&'a str> {
        if layer >= config.tracks().len() || column >= self.columns {
            return None;
        }
        if self.grid.is_empty() {
            config.tracks()[layer].clips().get(column).map(|c| c.id())
        } else {
            self.grid.get(layer)?.get(column)?.as_deref()
        }
    }
    pub fn validate(&self, video: &VideoConfig) -> Vec<String> {
        let mut errors = vec![];
        if video.canvas().frame_rate() > 60.0 || video.canvas().frame_rate() < 1.0 {
            errors.push("VJ compositions support 1..60 fps".into());
        }
        if u64::from(video.canvas().width())
            * u64::from(video.canvas().height())
            * video.tracks().len().max(1) as u64
            > 67_108_864
        {
            errors.push("VJ layers exceed the combined 64 megapixel working-frame limit".into());
        }
        if !range(self.bpm, 20.0, 300.0)
            || ![0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0].contains(&self.quantize)
        {
            errors.push(
                "VJ tempo must be 20..300 BPM and quantization a supported beat division".into(),
            );
        }
        if !(1..=64).contains(&self.columns)
            || self.layers.len() > video.tracks().len()
            || (!self.grid.is_empty() && self.grid.len() != video.tracks().len())
        {
            errors.push("VJ grid must match the tracks and have 1..64 columns".into());
        }
        for (i, row) in self.grid.iter().enumerate() {
            let mut seen = HashSet::new();
            if row.len() != self.columns
                || row.iter().flatten().any(|id| {
                    !seen.insert(id)
                        || !video
                            .tracks()
                            .get(i)
                            .is_some_and(|t| t.clips().iter().any(|c| c.id() == id))
                })
            {
                errors.push(format!("VJ row {i} contains a missing, repeated or foreign clip, or has the wrong width"));
            }
        }
        if self.layers.iter().any(|l| {
            !range(l.opacity, 0.0, 1.0)
                || !range(l.brightness, -1.0, 1.0)
                || !range(l.contrast, 0.0, 2.0)
                || !range(l.saturation, 0.0, 2.0)
                || !range(l.invert, 0.0, 1.0)
        }) {
            errors.push("VJ layer controls are out of range".into());
        }
        if self.clips.len() > 512
            || self.clips.keys().any(|id| {
                !video
                    .tracks()
                    .iter()
                    .flat_map(|t| t.clips())
                    .any(|c| c.id() == id)
            })
        {
            errors.push("VJ clip settings reference missing clips".into());
        }
        let mut ids = HashSet::new();
        let mut total_pixels = 0u64;
        if self.outputs.len() > MAX_OUTPUTS {
            errors.push("VJ supports at most 8 output canvases".into());
        }
        for out in &self.outputs {
            total_pixels += u64::from(out.width) * u64::from(out.height);
            if out.id == "composition"
                || !label(&out.id)
                || !label(&out.name)
                || !ids.insert(&out.id)
                || out.width == 0
                || out.height == 0
                || out.width > 4096
                || out.height > 4096
                || !range(out.opacity, 0.0, 1.0)
                || !range(out.brightness, 0.0, 2.0)
                || !range(out.gamma, 0.1, 4.0)
                || out.slices.len() > MAX_SLICES
            {
                errors.push(format!("invalid VJ output {:?}", out.name));
            }
            let mut slices = HashSet::new();
            for slice in &out.slices {
                let r = slice.input;
                if !label(&slice.id)
                    || !label(&slice.name)
                    || !slices.insert(&slice.id)
                    || !valid_quad(&slice.corners)
                    || !range(slice.opacity, 0.0, 1.0)
                    || slice.feather.iter().any(|v| !range(*v, 0.0, 1.0))
                    || !range(r.x, 0.0, 1.0)
                    || !range(r.y, 0.0, 1.0)
                    || !range(r.width, 0.0001, 1.0)
                    || !range(r.height, 0.0001, 1.0)
                    || r.x + r.width > 1.000001
                    || r.y + r.height > 1.000001
                    || matches!(slice.source,OutputSource::Layer{index} if index>=video.tracks().len())
                {
                    errors.push(format!(
                        "invalid slice {:?} in output {:?}",
                        slice.name, out.name
                    ));
                }
            }
        }
        if total_pixels > MAX_OUTPUT_PIXELS {
            errors.push("VJ outputs exceed the combined 16 megapixel limit".into());
        }
        let mut bindings = HashSet::new();
        if self.midi.len() > 512
            || self.midi.iter().any(|m| {
                !label(&m.id)
                    || !bindings.insert(&m.id)
                    || m.device.len() > 256
                    || m.channel > 15
                    || m.number > 127
                    || !range(m.min, 0.0, 1.0)
                    || !range(m.max, 0.0, 1.0)
                    || !m.target.validate(video.tracks().len(), self.columns)
            })
        {
            errors.push("invalid VJ MIDI mapping".into());
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn video() -> VideoConfig {
        serde_json::from_value(serde_json::json!({"tracks":[{"name":"Camera","clips":[{"id":"camera","start":0,"duration":10,"source":{"type":"test_pattern"}}]}]})).unwrap()
    }
    #[test]
    fn saved_deck_roundtrip_and_foreign_cells() {
        let video = video();
        let mut deck = VjConfig::default();
        assert_eq!(deck.clip_at(&video, 0, 0), Some("camera"));
        deck.columns = 2;
        deck.grid = vec![vec![Some("camera".into()), None]];
        assert!(deck.validate(&video).is_empty());
        let decoded: VjConfig =
            serde_json::from_str(&serde_json::to_string(&deck).unwrap()).unwrap();
        assert_eq!(decoded.clip_at(&video, 0, 0), Some("camera"));
        deck.grid[0][1] = Some("camera".into());
        assert!(!deck.validate(&video).is_empty());
        deck.grid[0][1] = Some("missing".into());
        assert!(!deck.validate(&video).is_empty());
    }
    #[test]
    fn folded_or_flat_mapping_is_rejected() {
        let mut points = corners();
        assert!(valid_quad(&points));
        points.swap(1, 2);
        assert!(!valid_quad(&points));
        points = [Point { x: 0.0, y: 0.0 }; 4];
        assert!(!valid_quad(&points));
        points = corners();
        points[0].x = f64::NAN;
        assert!(!valid_quad(&points));
    }
    #[test]
    fn invalid_midi_and_nonfinite_parameters_are_rejected() {
        let video = video();
        let mut deck = VjConfig::default();
        deck.bpm = f64::NAN;
        assert!(!deck.validate(&video).is_empty());
        deck.bpm = 120.0;
        deck.midi.push(MidiMapping {
            id: "a".into(),
            device: "".into(),
            channel: 16,
            kind: MidiKind::Cc,
            number: 1,
            target: ControlTarget::MasterOpacity,
            mode: MidiMode::Absolute,
            min: 0.0,
            max: 1.0,
        });
        assert!(!deck.validate(&video).is_empty());
        deck.midi[0].channel = 0;
        deck.midi[0].target = ControlTarget::LayerOpacity { layer: 7 };
        assert!(!deck.validate(&video).is_empty());
    }
}
