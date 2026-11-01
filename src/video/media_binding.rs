// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! Linux descriptor binding shared by runtime playback and editor preview.

use std::{collections::HashMap, fs::File, path::PathBuf};

use crate::{config::video::VideoConfig, video::timeline::RuntimeVideoTimeline};

pub(crate) const MAX_UNIQUE_MEDIA_FILES: usize = 256;

#[derive(Debug)]
pub(crate) struct BoundVideoMedia {
    pub(crate) config: VideoConfig,
    pub(crate) files: Vec<File>,
}

pub(crate) fn bind_timeline(
    config: &VideoConfig,
    timeline: &RuntimeVideoTimeline,
    context: &str,
) -> Result<BoundVideoMedia, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, timeline, context);
        return Err("video descriptor binding is supported only on Linux".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;

        let unique = timeline
            .clips()
            .iter()
            .map(|clip| clip.file())
            .collect::<std::collections::HashSet<_>>();
        if unique.len() > MAX_UNIQUE_MEDIA_FILES {
            return Err(format!(
                "{context} unique media files must not exceed {MAX_UNIQUE_MEDIA_FILES}"
            ));
        }

        let mut files = Vec::with_capacity(unique.len());
        let mut opened = HashMap::<PathBuf, String>::new();
        let mut replacements = Vec::with_capacity(timeline.clips().len());
        // Replacement order must match VideoConfig's track/clip order. The
        // runtime timeline is sorted by clip start and cannot be zipped back
        // onto the source config without swapping media between tracks.
        for configured_clip in config.tracks().iter().flat_map(|track| track.clips()) {
            let clip = timeline
                .clips()
                .iter()
                .find(|clip| clip.id() == configured_clip.id())
                .expect("validated runtime timeline retains every configured clip");
            if let Some(bound) = opened.get(clip.file()) {
                replacements.push(bound.clone());
                continue;
            }
            let file = File::open(clip.file()).map_err(|error| {
                format!(
                    "failed to open video clip {:?} at {}: {error}",
                    clip.id(),
                    clip.file().display()
                )
            })?;
            if !file
                .metadata()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                return Err(format!(
                    "video clip {:?} media is not a file: {}",
                    clip.id(),
                    clip.file().display()
                ));
            }
            let bound = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
            let opened_target = bound.canonicalize().map_err(|error| {
                format!(
                    "failed to verify opened video clip {:?}: {error}",
                    clip.id()
                )
            })?;
            if !opened_target.starts_with(timeline.root()) {
                return Err(format!(
                    "video clip {:?} opened outside the song directory: {}",
                    clip.id(),
                    opened_target.display()
                ));
            }
            let bound = bound.to_string_lossy().into_owned();
            replacements.push(bound.clone());
            opened.insert(clip.file().to_path_buf(), bound);
            files.push(file);
        }
        let mut config = config.clone();
        config.replace_clip_files(replacements)?;
        Ok(BoundVideoMedia { config, files })
    }
}
