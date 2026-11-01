// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! Pure video timeline model and optional GStreamer backend.

pub mod timeline;

#[cfg(feature = "video")]
mod engine;
#[cfg(feature = "video")]
pub mod gstreamer;
#[cfg(feature = "video")]
pub(crate) mod media_binding;
#[cfg(feature = "video")]
pub use engine::*;
