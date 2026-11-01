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
        if !self.canvas.frame_rate.is_finite() || self.canvas.frame_rate <= 0.0 {
            errors.push("canvas: frame_rate must be finite and positive".to_string());
        }
        if !is_hex_color(&self.canvas.background) {
            errors.push(format!(
                "canvas: background must be a hex color like \"#000000\", got {:?}",
                self.canvas.background
            ));
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
}
