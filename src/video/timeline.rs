// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! Resolved, backend-independent video timeline.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::config::video::VideoConfig;

/// A validated video timeline whose media paths are canonical and contained in
/// the song directory. It has no dependency on a playback backend.
///
/// `Song::new` resolves this model while the song registry loads, so missing or
/// escaping media fails that song with clip context. Editor preview keeps its
/// separate descriptor-bound validation to prevent validate/use path races.
#[derive(Clone, Debug)]
pub struct RuntimeVideoTimeline {
    #[cfg_attr(not(feature = "video"), allow(dead_code))]
    root: PathBuf,
    clips: Vec<RuntimeVideoClip>,
    end: Duration,
}

/// One resolved file clip. `z_order` is the source track's back-to-front index.
#[derive(Clone, Debug)]
pub struct RuntimeVideoClip {
    id: String,
    track_name: String,
    z_order: usize,
    file: PathBuf,
    start: Duration,
    duration: Duration,
    source_in: Duration,
    x: u32,
    y: u32,
    width: Option<u32>,
    height: Option<u32>,
    opacity: f64,
    fade_in: Duration,
    fade_out: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("song {song:?}: invalid video timeline: {message}")]
    InvalidConfig { song: String, message: String },
    #[error("song {song:?}: cannot resolve song directory {path}: {source}")]
    SongDirectory {
        song: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("song {song:?} video track {track:?} clip {clip:?}: file {path} does not exist or cannot be resolved: {source}")]
    MissingMedia {
        song: String,
        track: String,
        clip: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("song {song:?} video track {track:?} clip {clip:?}: file {path} resolves outside the song directory {root}")]
    OutsideSong {
        song: String,
        track: String,
        clip: String,
        path: PathBuf,
        root: PathBuf,
    },
    #[error(
        "song {song:?} video track {track:?} clip {clip:?}: file {path} is not a regular file"
    )]
    NotAFile {
        song: String,
        track: String,
        clip: String,
        path: PathBuf,
    },
    #[error("song {song:?} video track {track:?} clip {clip:?}: {field} cannot be represented: {source}")]
    InvalidDuration {
        song: String,
        track: String,
        clip: String,
        field: &'static str,
        #[source]
        source: std::time::TryFromFloatSecsError,
    },
}

impl RuntimeVideoTimeline {
    #[cfg(all(test, feature = "video"))]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            root: PathBuf::from("/"),
            clips: Vec::new(),
            end: Duration::ZERO,
        }
    }

    pub fn resolve(
        song: &str,
        base_path: &Path,
        config: &VideoConfig,
    ) -> Result<Self, TimelineError> {
        config
            .validate()
            .map_err(|errors| TimelineError::InvalidConfig {
                song: song.to_string(),
                message: errors.join("; "),
            })?;
        let root = base_path
            .canonicalize()
            .map_err(|source| TimelineError::SongDirectory {
                song: song.to_string(),
                path: base_path.to_path_buf(),
                source,
            })?;
        let mut clips = Vec::new();
        let mut end = Duration::ZERO;
        for (z_order, track) in config.tracks().iter().enumerate() {
            for clip in track.clips() {
                let unresolved = base_path.join(clip.file());
                let file =
                    unresolved
                        .canonicalize()
                        .map_err(|source| TimelineError::MissingMedia {
                            song: song.to_string(),
                            track: track.name().to_string(),
                            clip: clip.id().to_string(),
                            path: unresolved.clone(),
                            source,
                        })?;
                if !file.starts_with(&root) {
                    return Err(TimelineError::OutsideSong {
                        song: song.to_string(),
                        track: track.name().to_string(),
                        clip: clip.id().to_string(),
                        path: unresolved,
                        root,
                    });
                }
                if !file.is_file() {
                    return Err(TimelineError::NotAFile {
                        song: song.to_string(),
                        track: track.name().to_string(),
                        clip: clip.id().to_string(),
                        path: file,
                    });
                }
                let duration_value = |value, field| {
                    Duration::try_from_secs_f64(value).map_err(|source| {
                        TimelineError::InvalidDuration {
                            song: song.to_string(),
                            track: track.name().to_string(),
                            clip: clip.id().to_string(),
                            field,
                            source,
                        }
                    })
                };
                let clip_end = duration_value(clip.end(), "video end")?;
                let start = duration_value(clip.start(), "start")?;
                let duration = duration_value(clip.duration(), "duration")?;
                let source_in = duration_value(clip.source_in(), "source_in")?;
                let fade_in = duration_value(clip.fade_in(), "fade_in")?;
                let fade_out = duration_value(clip.fade_out(), "fade_out")?;
                end = end.max(clip_end);
                clips.push(RuntimeVideoClip {
                    id: clip.id().to_string(),
                    track_name: track.name().to_string(),
                    z_order,
                    file,
                    start,
                    duration,
                    source_in,
                    x: clip.x(),
                    y: clip.y(),
                    width: clip.width(),
                    height: clip.height(),
                    opacity: clip.opacity(),
                    fade_in,
                    fade_out,
                });
            }
        }
        clips.sort_by_key(|clip| clip.start);
        Ok(Self { root, clips, end })
    }

    pub fn clips(&self) -> &[RuntimeVideoClip] {
        &self.clips
    }
    pub fn end(&self) -> Duration {
        self.end
    }

    #[cfg_attr(not(feature = "video"), allow(dead_code))]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Clips active at `position`, ordered back-to-front. Clip bounds are
    /// half-open: the start is included and the end is excluded.
    pub fn active_clips_at(&self, position: Duration) -> Vec<&RuntimeVideoClip> {
        let mut active: Vec<_> = self
            .clips
            .iter()
            .filter(|clip| position >= clip.start && position < clip.end())
            .collect();
        active.sort_by_key(|clip| clip.z_order);
        active
    }
}

impl RuntimeVideoClip {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }
    pub fn z_order(&self) -> usize {
        self.z_order
    }
    pub fn file(&self) -> &Path {
        &self.file
    }
    pub fn start(&self) -> Duration {
        self.start
    }
    pub fn duration(&self) -> Duration {
        self.duration
    }
    pub fn end(&self) -> Duration {
        self.start + self.duration
    }
    pub fn source_in(&self) -> Duration {
        self.source_in
    }
    /// Source-media position corresponding to a song timeline position.
    pub fn source_position_at(&self, position: Duration) -> Duration {
        self.source_in
            .saturating_add(position.saturating_sub(self.start))
    }
    /// Effective opacity at a song position, combining base opacity and linear
    /// fade envelopes. Outside the clip bounds it is zero.
    pub fn opacity_at(&self, position: Duration) -> f64 {
        if position < self.start || position >= self.end() {
            return 0.0;
        }
        let elapsed = position.saturating_sub(self.start).as_secs_f64();
        let remaining = self.end().saturating_sub(position).as_secs_f64();
        let fade_in = self.fade_in.as_secs_f64();
        let fade_out = self.fade_out.as_secs_f64();
        let in_factor = if fade_in > 0.0 {
            (elapsed / fade_in).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let out_factor = if fade_out > 0.0 {
            (remaining / fade_out).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (self.opacity * in_factor * out_factor).clamp(0.0, 1.0)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::video::VideoConfig;
    use config::{Config, File, FileFormat};
    use std::{fs, path::Path};

    fn config(yaml: &str) -> VideoConfig {
        Config::builder()
            .add_source(File::from_str(yaml, FileFormat::Yaml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    fn create_media(root: &Path, names: &[&str]) {
        for name in names {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"placeholder").unwrap();
        }
    }

    #[test]
    fn resolves_absolute_paths_and_sorts_by_start_stably() {
        let root = tempfile::tempdir().unwrap();
        create_media(root.path(), &["later.mp4", "back.mp4", "front.mp4"]);
        let timeline = RuntimeVideoTimeline::resolve(
            "Timeline Song",
            root.path(),
            &config(
                r#"
tracks:
  - name: back
    clips:
      - { id: later, start: 5, duration: 1, file: later.mp4 }
      - { id: back-now, start: 1, duration: 1, file: back.mp4 }
  - name: front
    clips:
      - { id: front-now, start: 1, duration: 1, file: front.mp4 }
"#,
            ),
        )
        .unwrap();

        assert_eq!(
            timeline
                .clips()
                .iter()
                .map(|clip| clip.id())
                .collect::<Vec<_>>(),
            ["back-now", "front-now", "later"]
        );
        assert_eq!(timeline.clips()[0].track_name(), "back");
        assert_eq!(timeline.clips()[0].z_order(), 0);
        assert_eq!(timeline.clips()[1].z_order(), 1);
        assert!(timeline.clips()[0].file().is_absolute());
        assert_eq!(
            timeline.clips()[0].file(),
            root.path().canonicalize().unwrap().join("back.mp4")
        );
    }

    #[test]
    fn active_clips_use_half_open_bounds_and_z_order() {
        let root = tempfile::tempdir().unwrap();
        create_media(root.path(), &["back.mp4", "front.mp4"]);
        let timeline = RuntimeVideoTimeline::resolve(
            "Bounds",
            root.path(),
            &config(
                r#"
tracks:
  - name: back
    clips:
      - { id: back, start: 2, duration: 3, file: back.mp4 }
  - name: front
    clips:
      - { id: front, start: 1, duration: 5, file: front.mp4 }
"#,
            ),
        )
        .unwrap();

        let ids = |at| {
            timeline
                .active_clips_at(Duration::from_secs_f64(at))
                .into_iter()
                .map(|clip| clip.id())
                .collect::<Vec<_>>()
        };
        assert!(ids(0.999).is_empty());
        assert_eq!(ids(1.0), ["front"]);
        assert_eq!(ids(2.0), ["back", "front"]);
        assert_eq!(ids(4.999), ["back", "front"]);
        assert_eq!(ids(5.0), ["front"]);
        assert!(ids(6.0).is_empty());
    }

    #[test]
    fn source_position_adds_elapsed_clip_time_to_source_in() {
        let root = tempfile::tempdir().unwrap();
        create_media(root.path(), &["clip.mp4"]);
        let timeline = RuntimeVideoTimeline::resolve(
            "Seek",
            root.path(),
            &config(
                "tracks: [{ name: screen, clips: [{ id: clip, start: 10, duration: 5, file: clip.mp4, source_in: 2.5 }] }]",
            ),
        )
        .unwrap();
        let clip = &timeline.clips()[0];

        assert_eq!(
            clip.source_position_at(Duration::from_secs(10)),
            Duration::from_secs_f64(2.5)
        );
        assert_eq!(
            clip.source_position_at(Duration::from_secs_f64(12.25)),
            Duration::from_secs_f64(4.75)
        );
    }

    #[test]
    fn opacity_combines_base_value_with_linear_fades_and_keeps_layout() {
        let root = tempfile::tempdir().unwrap();
        create_media(root.path(), &["clip.mp4"]);
        let timeline = RuntimeVideoTimeline::resolve(
            "Fade",
            root.path(),
            &config(
                "tracks: [{ name: screen, clips: [{ id: clip, start: 10, duration: 10, file: clip.mp4, opacity: 0.8, fade_in: 2, fade_out: 4, x: 12, y: 34, width: 320, height: 180 }] }]",
            ),
        )
        .unwrap();
        let clip = &timeline.clips()[0];

        for (position, expected) in [
            (10.0, 0.0),
            (11.0, 0.4),
            (12.0, 0.8),
            (16.0, 0.8),
            (18.0, 0.4),
            (20.0, 0.0),
        ] {
            let actual = clip.opacity_at(Duration::from_secs_f64(position));
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "at {position}: {actual}"
            );
            assert!(actual.is_finite());
        }
        assert_eq!(
            (clip.x(), clip.y(), clip.width(), clip.height()),
            (12, 34, Some(320), Some(180))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_whose_target_escapes_the_song_directory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.mkv"), b"secret").unwrap();
        symlink(
            outside.path().join("secret.mkv"),
            root.path().join("escape.mkv"),
        )
        .unwrap();

        let error = RuntimeVideoTimeline::resolve(
            "Escape",
            root.path(),
            &config("tracks: [{ name: screen, clips: [{ id: escape, start: 0, duration: 1, file: escape.mkv }] }]")
        ).unwrap_err();

        assert!(
            matches!(error, TimelineError::OutsideSong { .. }),
            "{error}"
        );
    }

    #[test]
    fn overlapping_fades_multiply_their_envelopes() {
        let root = tempfile::tempdir().unwrap();
        create_media(root.path(), &["clip.mp4"]);
        let timeline = RuntimeVideoTimeline::resolve(
            "Overlap",
            root.path(),
            &config("tracks: [{ name: screen, clips: [{ id: clip, start: 0, duration: 2, file: clip.mp4, opacity: 0.8, fade_in: 2, fade_out: 2 }] }]")
        ).unwrap();

        assert!((timeline.clips()[0].opacity_at(Duration::from_secs(1)) - 0.2).abs() < 1e-12);
    }
}
