// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.

use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};

/// Timeline comparisons tolerate one nanosecond so ordinary decimal seconds
/// (for example, `0.1 + 0.2`) do not create a spurious overlap.
const TIMELINE_EPSILON_SECONDS: f64 = 1.0e-9;

/// Lowest supported canvas frame rate. This keeps the milliframes-per-second
/// GStreamer fraction numerator non-zero and limits one frame period to 1,000s.
pub const MIN_FRAME_RATE: f64 = 0.001;
/// Highest supported canvas frame rate. This covers common high-frame-rate
/// video while keeping preview scheduling and resource use bounded.
pub const MAX_FRAME_RATE: f64 = 240.0;
/// Largest supported canvas dimension. This bounds compositor allocations.
pub const MAX_CANVAS_DIMENSION: u32 = 4096;
/// Largest supported canvas area (DCI 4K: 4096 by 2160 pixels).
pub const MAX_CANVAS_PIXELS: u64 = 8_847_360;
/// Maximum number of composited timeline tracks.
pub const MAX_VIDEO_TRACKS: usize = 32;
/// Maximum number of clips across all tracks.
pub const MAX_VIDEO_CLIPS: usize = 512;

/// A song's linear video timeline.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct VideoConfig {
    /// Output canvas shared by every video track.
    #[serde(default)]
    canvas: VideoCanvas,
    /// Video layers in back-to-front order.
    #[serde(default)]
    tracks: Vec<VideoTrack>,
}

impl VideoConfig {
    /// Gets the output canvas configuration.
    pub fn canvas(&self) -> &VideoCanvas {
        &self.canvas
    }

    /// Gets the ordered video tracks.
    pub fn tracks(&self) -> &[VideoTrack] {
        &self.tracks
    }

    /// Replaces clip file names with already validated canonical paths.
    ///
    /// This is crate-private because absolute paths are not part of the song
    /// file format; the preview service uses it to bind validation and use to
    /// the same filesystem targets.
    #[cfg(feature = "video")]
    pub(crate) fn replace_clip_files(&mut self, files: Vec<String>) -> Result<(), String> {
        let expected = self
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>();
        if files.len() != expected {
            return Err(format!(
                "expected {expected} clip files, got {}",
                files.len()
            ));
        }
        for (clip, file) in self
            .tracks
            .iter_mut()
            .flat_map(|track| track.clips.iter_mut())
            .zip(files)
        {
            clip.file = file;
        }
        Ok(())
    }

    /// Validates the canvas and timeline structure.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let mut ids = HashSet::new();

        if self.canvas.width == 0 {
            errors.push("canvas: width must be positive".to_string());
        }
        if self.canvas.height == 0 {
            errors.push("canvas: height must be positive".to_string());
        }
        if self.canvas.width > MAX_CANVAS_DIMENSION {
            errors.push(format!(
                "canvas: width must not exceed {MAX_CANVAS_DIMENSION}"
            ));
        }
        if self.canvas.height > MAX_CANVAS_DIMENSION {
            errors.push(format!(
                "canvas: height must not exceed {MAX_CANVAS_DIMENSION}"
            ));
        }
        let canvas_pixels = u64::from(self.canvas.width).checked_mul(u64::from(self.canvas.height));
        if canvas_pixels.is_none_or(|pixels| pixels > MAX_CANVAS_PIXELS) {
            errors.push(format!(
                "canvas: pixel count must not exceed {MAX_CANVAS_PIXELS}"
            ));
        }
        if !self.canvas.frame_rate.is_finite() {
            errors.push("canvas: frame_rate must be finite and positive".to_string());
        } else if !(MIN_FRAME_RATE..=MAX_FRAME_RATE).contains(&self.canvas.frame_rate) {
            errors.push(format!(
                "canvas: frame_rate must be within the supported range {MIN_FRAME_RATE}..={MAX_FRAME_RATE} fps"
            ));
        }
        if !is_hex_color(&self.canvas.background) {
            errors.push(format!(
                "canvas: background must be a hex color like \"#000000\", got {:?}",
                self.canvas.background
            ));
        }

        if self.tracks.len() > MAX_VIDEO_TRACKS {
            errors.push(format!("video: tracks must not exceed {MAX_VIDEO_TRACKS}"));
        }
        let clip_count = self
            .tracks
            .iter()
            .try_fold(0_usize, |count, track| count.checked_add(track.clips.len()));
        if clip_count.is_none_or(|count| count > MAX_VIDEO_CLIPS) {
            errors.push(format!("video: clips must not exceed {MAX_VIDEO_CLIPS}"));
        }

        for (track_index, track) in self.tracks.iter().enumerate() {
            let track_label = if track.name.trim().is_empty() {
                format!("video track[{track_index}]")
            } else {
                format!("video track {:?}", track.name)
            };
            if track.name.trim().is_empty() {
                errors.push(format!("{track_label}: name must not be empty"));
            }
            if track.clips.is_empty() {
                errors.push(format!("{track_label}: clips must not be empty"));
            }

            for (clip_index, clip) in track.clips.iter().enumerate() {
                let clip_label = format!("{track_label} clip[{clip_index}]");
                if clip.id.trim().is_empty() {
                    errors.push(format!("{clip_label}: id must not be empty"));
                } else if !ids.insert(clip.id.as_str()) {
                    errors.push(format!("{clip_label}: duplicate clip id {:?}", clip.id));
                }
                if !clip.start.is_finite() || clip.start < 0.0 {
                    errors.push(format!(
                        "{clip_label}: start must be finite and non-negative"
                    ));
                }
                if !clip.duration.is_finite() || clip.duration <= 0.0 {
                    errors.push(format!(
                        "{clip_label}: duration must be finite and positive"
                    ));
                }
                if clip.start.is_finite()
                    && clip.start >= 0.0
                    && clip.duration.is_finite()
                    && clip.duration > 0.0
                    && !clip.end().is_finite()
                {
                    errors.push(format!("{clip_label}: clip end must be finite"));
                }
                if !clip.source_in.is_finite() || clip.source_in < 0.0 {
                    errors.push(format!(
                        "{clip_label}: source_in must be finite and non-negative"
                    ));
                }
                if clip.x > i32::MAX as u32 {
                    errors.push(format!("{clip_label}: x is too large"));
                }
                if clip.y > i32::MAX as u32 {
                    errors.push(format!("{clip_label}: y is too large"));
                }
                if clip.width == Some(0) {
                    errors.push(format!("{clip_label}: width must be positive"));
                } else if clip.width.is_some_and(|width| width > MAX_CANVAS_DIMENSION) {
                    errors.push(format!(
                        "{clip_label}: width must not exceed {MAX_CANVAS_DIMENSION}"
                    ));
                }
                if clip.height == Some(0) {
                    errors.push(format!("{clip_label}: height must be positive"));
                } else if clip
                    .height
                    .is_some_and(|height| height > MAX_CANVAS_DIMENSION)
                {
                    errors.push(format!(
                        "{clip_label}: height must not exceed {MAX_CANVAS_DIMENSION}"
                    ));
                }
                let layout_width = clip.width.unwrap_or(self.canvas.width);
                let layout_height = clip.height.unwrap_or(self.canvas.height);
                let layout_pixels = u64::from(layout_width).checked_mul(u64::from(layout_height));
                if layout_pixels.is_none_or(|pixels| pixels > MAX_CANVAS_PIXELS) {
                    errors.push(format!(
                        "{clip_label}: pixel count must not exceed {MAX_CANVAS_PIXELS}"
                    ));
                }
                if !clip.opacity.is_finite() || !(0.0..=1.0).contains(&clip.opacity) {
                    errors.push(format!(
                        "{clip_label}: opacity must be finite and within 0..=1"
                    ));
                }
                for (field, fade) in [("fade_in", clip.fade_in), ("fade_out", clip.fade_out)] {
                    if !fade.is_finite() || fade < 0.0 {
                        errors.push(format!(
                            "{clip_label}: {field} must be finite and non-negative"
                        ));
                    }
                }
                if clip.file.trim().is_empty() {
                    errors.push(format!("{clip_label}: file must not be empty"));
                } else if !is_safe_relative_path(&clip.file) {
                    errors.push(format!("{clip_label}: file must be a safe relative path"));
                }
            }

            let mut clips: Vec<&VideoClip> = track
                .clips
                .iter()
                .filter(|clip| {
                    clip.start.is_finite()
                        && clip.start >= 0.0
                        && clip.duration.is_finite()
                        && clip.duration > 0.0
                })
                .collect();
            clips.sort_by(|left, right| left.start.total_cmp(&right.start));
            for pair in clips.windows(2) {
                if pair[0].end() - pair[1].start > TIMELINE_EPSILON_SECONDS {
                    errors.push(format!(
                        "{track_label}: clip {:?} overlaps clip {:?}",
                        pair[0].id, pair[1].id
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Last clip end on the song timeline, in seconds.
    pub fn end(&self) -> f64 {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .map(VideoClip::end)
            .fold(0.0, f64::max)
    }
}

/// Video output dimensions and timing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VideoCanvas {
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default = "default_frame_rate")]
    /// Frames per second, in the inclusive supported range 0.001..=240.
    frame_rate: f64,
    #[serde(default = "default_background")]
    background: String,
}

impl Default for VideoCanvas {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            frame_rate: default_frame_rate(),
            background: default_background(),
        }
    }
}

impl VideoCanvas {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    pub fn background(&self) -> &str {
        &self.background
    }
}

/// One ordered layer in the video timeline.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VideoTrack {
    name: String,
    #[serde(default)]
    clips: Vec<VideoClip>,
}

impl VideoTrack {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn clips(&self) -> &[VideoClip] {
        &self.clips
    }
}

/// A file-backed clip placed on a video track.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VideoClip {
    id: String,
    start: f64,
    duration: f64,
    file: String,
    #[serde(default)]
    source_in: f64,
    #[serde(default)]
    /// Horizontal position in output-canvas pixels (not a normalized value).
    x: u32,
    #[serde(default)]
    /// Vertical position in output-canvas pixels (not a normalized value).
    y: u32,
    #[serde(default)]
    /// Optional output width in pixels; omitted uses the canvas width.
    width: Option<u32>,
    #[serde(default)]
    /// Optional output height in pixels; omitted uses the canvas height.
    height: Option<u32>,
    #[serde(
        default = "default_opacity",
        skip_serializing_if = "is_default_opacity"
    )]
    /// Base compositor opacity in the inclusive range 0..=1.
    opacity: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    /// Linear fade-in duration in seconds.
    fade_in: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    /// Linear fade-out duration in seconds.
    fade_out: f64,
}

impl VideoClip {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn start(&self) -> f64 {
        self.start
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    pub fn source_in(&self) -> f64 {
        self.source_in
    }

    pub fn x(&self) -> u32 {
        self.x
    }

    pub fn y(&self) -> u32 {
        self.y
    }

    pub fn width(&self) -> Option<u32> {
        self.width
    }

    pub fn height(&self) -> Option<u32> {
        self.height
    }

    pub fn opacity(&self) -> f64 {
        self.opacity
    }

    pub fn fade_in(&self) -> f64 {
        self.fade_in
    }

    pub fn fade_out(&self) -> f64 {
        self.fade_out
    }

    /// End position on the song timeline, in seconds.
    pub fn end(&self) -> f64 {
        self.start + self.duration
    }
}

const fn default_width() -> u32 {
    1920
}

const fn default_height() -> u32 {
    1080
}

fn default_frame_rate() -> f64 {
    50.0
}

fn default_background() -> String {
    "#000000".to_string()
}

fn default_opacity() -> f64 {
    1.0
}

fn is_default_opacity(value: &f64) -> bool {
    *value == 1.0
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn is_safe_relative_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let has_windows_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !normalized.starts_with('/')
        && !has_windows_prefix
        && Path::new(&normalized)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Config, File, FileFormat};

    #[test]
    fn deserializes_timeline_with_canvas_tracks_and_clips() {
        let yaml = r##"
canvas:
  width: 1280
  height: 720
  frame_rate: 25.0
  background: "#112233"
tracks:
  - name: back wall
    clips:
      - id: intro
        start: 1.5
        duration: 4.0
        file: video/intro.mp4
        source_in: 0.25
  - name: overlay
    clips:
      - id: title
        start: 2.0
        duration: 1.0
        file: video/title.mov
        source_in: 0.0
"##;
        let video: VideoConfig = Config::builder()
            .add_source(File::from_str(yaml, FileFormat::Yaml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();

        assert_eq!(video.canvas().width(), 1280);
        assert_eq!(video.canvas().height(), 720);
        assert_eq!(video.canvas().frame_rate(), 25.0);
        assert_eq!(video.canvas().background(), "#112233");
        assert_eq!(video.tracks()[0].name(), "back wall");
        assert_eq!(video.tracks()[1].name(), "overlay");
        let clip = &video.tracks()[0].clips()[0];
        assert_eq!(clip.id(), "intro");
        assert_eq!(clip.start(), 1.5);
        assert_eq!(clip.duration(), 4.0);
        assert_eq!(clip.file(), "video/intro.mp4");
        assert_eq!(clip.source_in(), 0.25);
        assert_eq!(clip.x(), 0);
        assert_eq!(clip.y(), 0);
        assert_eq!(clip.width(), None);
        assert_eq!(clip.height(), None);
    }

    #[test]
    fn deserializes_optional_clip_layout() {
        let video = from_yaml(
            r#"
tracks:
  - name: overlay
    clips:
      - { id: title, start: 0, duration: 1, file: title.mkv, x: 12, y: 34, width: 320, height: 180 }
"#,
        );
        let clip = &video.tracks()[0].clips()[0];

        assert_eq!(clip.x(), 12);
        assert_eq!(clip.y(), 34);
        assert_eq!(clip.width(), Some(320));
        assert_eq!(clip.height(), Some(180));
        assert!(video.validate().is_ok());
    }

    fn from_yaml(yaml: &str) -> VideoConfig {
        Config::builder()
            .add_source(File::from_str(yaml, FileFormat::Yaml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[test]
    fn validates_clip_fields_ids_paths_and_track_names() {
        let mut video = from_yaml(
            r#"
tracks:
  - name: " "
    clips:
      - { id: "", start: -1.0, duration: 0.0, file: "../escape.mp4", source_in: -2.0 }
  - name: second
    clips:
      - { id: duplicate, start: 1.0, duration: 1.0, file: "/absolute.mp4", source_in: 0.0 }
      - { id: duplicate, start: 3.0, duration: 1.0, file: "C:\\absolute.mp4", source_in: 0.0 }
"#,
        );
        video.tracks[1].clips[0].start = f64::INFINITY;
        video.tracks[1].clips[0].duration = f64::INFINITY;
        video.tracks[1].clips[0].source_in = f64::NAN;

        let errors = video.validate().unwrap_err();
        for expected in [
            "name must not be empty",
            "id must not be empty",
            "start must be finite and non-negative",
            "duration must be finite and positive",
            "source_in must be finite and non-negative",
            "file must be a safe relative path",
            "duplicate clip id \"duplicate\"",
        ] {
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "missing {expected:?} in {errors:?}"
            );
        }
    }

    #[test]
    fn rejects_a_video_track_with_no_clips() {
        let video = from_yaml(
            r#"
tracks:
  - name: screen
    clips: []
"#,
        );

        assert_eq!(
            video.validate().unwrap_err(),
            vec!["video track \"screen\": clips must not be empty"]
        );
    }

    #[test]
    fn allows_decimal_clips_that_are_adjacent_within_timeline_precision() {
        let video = from_yaml(
            r#"
tracks:
  - name: screen
    clips:
      - { id: first, start: 0.1, duration: 0.2, file: first.mp4 }
      - { id: second, start: 0.3, duration: 1.0, file: second.mp4 }
"#,
        );

        assert!(video.validate().is_ok());
    }

    #[test]
    fn rejects_overlaps_within_a_track_but_allows_cross_track_overlap() {
        let overlapping = from_yaml(
            r#"
tracks:
  - name: screen
    clips:
      - { id: later, start: 4.0, duration: 3.0, file: later.mp4 }
      - { id: earlier, start: 2.0, duration: 3.0, file: earlier.mp4 }
"#,
        );
        assert!(overlapping
            .validate()
            .unwrap_err()
            .iter()
            .any(|error| error.contains("overlaps")));

        let layered = from_yaml(
            r#"
tracks:
  - name: background
    clips:
      - { id: background, start: 2.0, duration: 3.0, file: background.mp4 }
  - name: foreground
    clips:
      - { id: foreground, start: 2.0, duration: 3.0, file: foreground.mp4 }
"#,
        );
        assert!(layered.validate().is_ok());
    }

    #[test]
    fn rejects_a_non_finite_clip_end() {
        let mut video = from_yaml(
            r#"
tracks:
  - name: screen
    clips:
      - { id: huge, start: 1.0, duration: 1.0, file: huge.mp4 }
"#,
        );
        video.tracks[0].clips[0].start = f64::MAX;
        video.tracks[0].clips[0].duration = f64::MAX;

        let errors = video.validate().unwrap_err();
        assert!(errors.iter().any(|error| error.contains("clip end")));
    }

    #[test]
    fn rejects_zero_clip_layout_dimensions() {
        let video = from_yaml(
            r#"
tracks:
  - name: overlay
    clips:
      - { id: title, start: 0, duration: 1, file: title.mkv, x: 4294967295, y: 4294967295, width: 0, height: 0 }
"#,
        );

        let errors = video.validate().unwrap_err();
        assert!(errors.iter().any(|error| error.contains("x is too large")));
        assert!(errors.iter().any(|error| error.contains("y is too large")));
        assert!(errors
            .iter()
            .any(|error| error.contains("width must be positive")));
        assert!(errors
            .iter()
            .any(|error| error.contains("height must be positive")));
    }

    #[test]
    fn enforces_clip_layout_dimension_and_area_limits() {
        let at_limit = from_yaml(
            r#"
tracks:
  - name: overlay
    clips:
      - { id: title, start: 0, duration: 1, file: title.mkv, width: 4096, height: 2160 }
"#,
        );
        assert!(at_limit.validate().is_ok());

        for (width, height, expected) in [
            (4097, 1, "width must not exceed 4096"),
            (1, 4097, "height must not exceed 4096"),
            (4096, 2161, "pixel count must not exceed 8847360"),
        ] {
            let video = from_yaml(&format!(
                r#"
tracks:
  - name: overlay
    clips:
      - {{ id: title, start: 0, duration: 1, file: title.mkv, width: {width}, height: {height} }}
"#,
            ));
            let errors = video.validate().unwrap_err();
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "missing {expected:?} for {width}x{height}: {errors:?}"
            );
        }
    }

    #[test]
    fn accepts_supported_frame_rate_edges() {
        for frame_rate in [0.001, 240.0] {
            let video = from_yaml(&format!(
                r##"canvas: {{ width: 2, height: 2, frame_rate: {frame_rate}, background: "#000000" }}"##
            ));

            assert!(
                video.validate().is_ok(),
                "supported frame rate {frame_rate} was rejected"
            );
        }
    }

    #[test]
    fn rejects_frame_rates_outside_supported_range() {
        for frame_rate in [0.000_999, 240.001] {
            let video = from_yaml(&format!(
                r##"canvas: {{ width: 2, height: 2, frame_rate: {frame_rate}, background: "#000000" }}"##
            ));

            let errors = video.validate().unwrap_err();
            assert!(
                errors.iter().any(|error| error.contains("0.001..=240")),
                "out-of-range frame rate {frame_rate} produced {errors:?}"
            );
        }
    }

    #[test]
    fn accepts_documented_video_resource_boundaries() {
        let tracks = (0..32)
            .map(|track| {
                let clips = (0..16)
                    .map(|clip| format!("{{ id: clip-{track}-{clip}, start: {clip}, duration: 1, file: shared.mkv }}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ name: track-{track}, clips: [{clips}] }}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let video = from_yaml(&format!(
            r##"canvas: {{ width: 4096, height: 2160, frame_rate: 50, background: "#000000" }}
tracks: [{tracks}]"##
        ));

        assert!(video.validate().is_ok());
    }

    #[test]
    fn rejects_canvas_track_and_clip_resource_overages() {
        for (yaml, expected) in [
            (
                r##"canvas: { width: 4097, height: 1, background: "#000000" }"##,
                "width must not exceed 4096",
            ),
            (
                r##"canvas: { width: 1, height: 4097, background: "#000000" }"##,
                "height must not exceed 4096",
            ),
            (
                r##"canvas: { width: 4096, height: 4096, background: "#000000" }"##,
                "pixel count must not exceed 8847360",
            ),
        ] {
            let errors = from_yaml(yaml).validate().unwrap_err();
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "{errors:?}"
            );
        }

        let tracks = (0..33)
            .map(|index| format!("{{ name: t{index}, clips: [{{ id: c{index}, start: 0, duration: 1, file: f }}] }}"))
            .collect::<Vec<_>>()
            .join(", ");
        let errors = from_yaml(&format!("tracks: [{tracks}]"))
            .validate()
            .unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("tracks must not exceed 32")));

        let clips = (0..513)
            .map(|index| format!("{{ id: c{index}, start: {index}, duration: 1, file: f }}"))
            .collect::<Vec<_>>()
            .join(", ");
        let errors = from_yaml(&format!("tracks: [{{ name: t, clips: [{clips}] }}]"))
            .validate()
            .unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("clips must not exceed 512")));
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_opacity_and_fades() {
        let mut video =
            from_yaml("tracks: [{ name: t, clips: [{ id: c, start: 0, duration: 1, file: f }] }]");
        let clip = &mut video.tracks[0].clips[0];
        clip.opacity = f64::NAN;
        clip.fade_in = -1.0;
        clip.fade_out = f64::INFINITY;

        let errors = video.validate().unwrap_err();
        for expected in ["opacity", "fade_in", "fade_out"] {
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "missing {expected:?} in {errors:?}"
            );
        }
    }

    #[cfg(feature = "video")]
    #[test]
    fn replacing_clip_files_requires_an_exact_count() {
        let mut video = from_yaml(
            "tracks: [{ name: t, clips: [{ id: a, start: 0, duration: 1, file: a }, { id: b, start: 1, duration: 1, file: b }] }]",
        );

        let error = video
            .replace_clip_files(vec!["only-one".to_string()])
            .unwrap_err();

        assert!(error.contains("expected 2 clip files, got 1"), "{error}");
        assert_eq!(video.tracks()[0].clips()[0].file(), "a");
        assert_eq!(video.tracks()[0].clips()[1].file(), "b");
    }
}
