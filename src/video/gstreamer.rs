// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! GStreamer installation diagnostics for the optional video backend.

use std::sync::OnceLock;

use gstreamer as gst;

/// A functional area of the V1 video backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementGroup {
    Core,
    FilePreview,
    Capture,
    ProgramOutput,
}

impl ElementGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Core => "core compositor",
            Self::FilePreview => "file decode/preview",
            Self::Capture => "Linux V4L2 capture",
            Self::ProgramOutput => "program output",
        }
    }
}

const CORE_ELEMENTS: &[&str] = &[
    "appsrc",
    "capsfilter",
    "compositor",
    "queue",
    "videoconvert",
];
const FILE_PREVIEW_ELEMENTS: &[&str] = &["appsink", "uridecodebin3", "videorate", "videoscale"];
#[cfg(target_os = "linux")]
const CAPTURE_ELEMENTS: &[&str] = &["v4l2src"];
#[cfg(not(target_os = "linux"))]
const CAPTURE_ELEMENTS: &[&str] = &[];
const PROGRAM_ELEMENTS: &[&str] = &["autovideosink"];

#[cfg(target_os = "linux")]
const ALL_DEFAULT_ELEMENTS: &[&str] = &[
    "appsrc",
    "appsink",
    "capsfilter",
    "compositor",
    "queue",
    "uridecodebin3",
    "videoconvert",
    "videoscale",
    "videorate",
    "v4l2src",
    "autovideosink",
];
#[cfg(not(target_os = "linux"))]
const ALL_DEFAULT_ELEMENTS: &[&str] = &[
    "appsrc",
    "appsink",
    "capsfilter",
    "compositor",
    "queue",
    "uridecodebin3",
    "videoconvert",
    "videoscale",
    "videorate",
    "autovideosink",
];

/// Which capabilities must be usable for the caller's operation.
#[derive(Clone, Debug)]
pub struct DiagnosticRequest {
    file_preview: bool,
    capture: bool,
    program_sink: Option<String>,
}

impl DiagnosticRequest {
    /// Requirements for file-backed browser/editor preview.
    pub fn file_preview() -> Self {
        Self {
            file_preview: true,
            capture: false,
            program_sink: None,
        }
    }

    /// Requirements for file-backed program output.
    pub fn program(sink: impl Into<String>) -> Self {
        Self {
            file_preview: true,
            capture: false,
            program_sink: Some(sink.into()),
        }
    }

    /// Requirements for Linux camera capture and preview.
    pub fn capture_preview() -> Self {
        Self {
            file_preview: false,
            capture: true,
            program_sink: None,
        }
    }

    fn inventory() -> Self {
        Self {
            file_preview: false,
            capture: false,
            program_sink: None,
        }
    }
}

/// Availability for one functional element group.
#[derive(Clone, Debug)]
pub struct ElementGroupDiagnostics {
    pub group: ElementGroup,
    pub required: bool,
    pub available: Vec<String>,
    pub missing: Vec<String>,
}

/// Cached snapshot of the GStreamer runtime and plugin registry.
#[derive(Clone, Debug)]
pub struct GstreamerDiagnostics {
    pub initialized: bool,
    pub initialization_error: Option<String>,
    pub version: Option<String>,
    pub feature: &'static str,
    pub target_os: &'static str,
    pub target_arch: &'static str,
    pub groups: Vec<ElementGroupDiagnostics>,
    pub ready: bool,
    pub jpeg_backend: &'static str,
}

impl GstreamerDiagnostics {
    pub fn missing_in(&self, group: ElementGroup) -> &[String] {
        self.groups
            .iter()
            .find(|diagnostics| diagnostics.group == group)
            .map_or(&[], |diagnostics| diagnostics.missing.as_slice())
    }

    pub fn missing_required(&self) -> Vec<&str> {
        self.groups
            .iter()
            .filter(|group| group.required)
            .flat_map(|group| group.missing.iter().map(String::as_str))
            .collect()
    }

    /// Human-readable failure with Debian/Ubuntu remediation.
    pub fn actionable_error(&self, context: &str) -> String {
        let mut message = format!(
            "GStreamer diagnostics for {context} (feature={}, target={}-{})",
            self.feature, self.target_os, self.target_arch
        );
        if let Some(version) = &self.version {
            message.push_str(&format!("; GStreamer {version}"));
        }
        if let Some(error) = &self.initialization_error {
            message.push_str(&format!("; initialization failed: {error}"));
        }
        for group in self.groups.iter().filter(|group| !group.missing.is_empty()) {
            let importance = if group.required {
                "required"
            } else {
                "optional"
            };
            message.push_str(&format!(
                "\n- {} ({importance}): missing {}",
                group.group.label(),
                group.missing.join(", ")
            ));
        }
        message.push_str(
            "\nDebian/Ubuntu: sudo apt-get install libgstreamer1.0-dev \
             libgstreamer-plugins-base1.0-dev gstreamer1.0-tools \
             gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
             gstreamer1.0-plugins-bad gstreamer1.0-libav",
        );
        message
            .push_str("\nJPEG previews use the Rust image encoder; gst jpegenc is not required.");
        message
    }

    fn with_request(mut self, request: &DiagnosticRequest, probe: &impl ElementProbe) -> Self {
        for group in &mut self.groups {
            group.required = match group.group {
                ElementGroup::Core => true,
                ElementGroup::FilePreview => request.file_preview,
                ElementGroup::Capture => request.capture,
                ElementGroup::ProgramOutput => request.program_sink.is_some(),
            };
        }
        if let Some(sink) = &request.program_sink {
            let program = self
                .groups
                .iter_mut()
                .find(|group| group.group == ElementGroup::ProgramOutput)
                .expect("program output group is always present");
            program.available.clear();
            program.missing.clear();
            if self.initialized && probe.has_element(sink) {
                program.available.push(sink.clone());
            } else {
                program.missing.push(sink.clone());
            }
        }
        self.ready = self.initialized
            && self
                .groups
                .iter()
                .filter(|group| group.required)
                .all(|group| group.missing.is_empty());
        self
    }
}

trait ElementProbe {
    fn initialize(&self) -> Result<String, String>;
    fn has_element(&self, name: &str) -> bool;
}

struct SystemProbe;

impl ElementProbe for SystemProbe {
    fn initialize(&self) -> Result<String, String> {
        gst::init().map_err(|error| error.to_string())?;
        let (major, minor, micro, nano) = gst::version();
        Ok(format!("{major}.{minor}.{micro}.{nano}"))
    }

    fn has_element(&self, name: &str) -> bool {
        gst::ElementFactory::find(name).is_some()
    }
}

#[derive(Default)]
struct DiagnosticsCache {
    report: OnceLock<GstreamerDiagnostics>,
}

impl DiagnosticsCache {
    fn get_or_init<'a>(&'a self, probe: &impl ElementProbe) -> &'a GstreamerDiagnostics {
        self.report
            .get_or_init(|| diagnose_with_probe(probe, DiagnosticRequest::inventory()))
    }
}

static DIAGNOSTICS: DiagnosticsCache = DiagnosticsCache {
    report: OnceLock::new(),
};

/// Returns a cached registry scan, marked for the requested operation.
///
/// The process initializes GStreamer and enumerates the default V1 factories
/// once. A caller-selected program sink is the only additional lookup.
pub fn diagnostics(request: DiagnosticRequest) -> GstreamerDiagnostics {
    let probe = SystemProbe;
    DIAGNOSTICS
        .get_or_init(&probe)
        .clone()
        .with_request(&request, &probe)
}

fn diagnose_with_probe(
    probe: &impl ElementProbe,
    request: DiagnosticRequest,
) -> GstreamerDiagnostics {
    let initialization = probe.initialize();
    let initialized = initialization.is_ok();
    let (version, initialization_error) = match initialization {
        Ok(version) => (Some(version), None),
        Err(error) => (None, Some(error)),
    };
    let groups = [
        (ElementGroup::Core, CORE_ELEMENTS),
        (ElementGroup::FilePreview, FILE_PREVIEW_ELEMENTS),
        (ElementGroup::Capture, CAPTURE_ELEMENTS),
        (ElementGroup::ProgramOutput, PROGRAM_ELEMENTS),
    ]
    .into_iter()
    .map(|(group, elements)| {
        let mut available = Vec::new();
        let mut missing = Vec::new();
        for element in elements {
            if initialized && probe.has_element(element) {
                available.push((*element).to_string());
            } else {
                missing.push((*element).to_string());
            }
        }
        ElementGroupDiagnostics {
            group,
            required: false,
            available,
            missing,
        }
    })
    .collect::<Vec<_>>();
    // Keep this explicit so adding an element to the inventory cannot silently
    // leave it out of the grouped diagnostics.
    debug_assert_eq!(
        groups
            .iter()
            .map(|group| group.available.len() + group.missing.len())
            .sum::<usize>(),
        ALL_DEFAULT_ELEMENTS.len()
    );
    let report = GstreamerDiagnostics {
        initialized,
        initialization_error,
        version,
        feature: "video",
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
        groups,
        ready: false,
        jpeg_backend: "Rust image crate (JPEG); no GStreamer jpegenc required",
    };
    report.with_request(&request, probe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct FakeProbe {
        available: HashSet<&'static str>,
        initializations: AtomicUsize,
    }

    impl FakeProbe {
        fn with(available: &[&'static str]) -> Self {
            Self {
                available: available.iter().copied().collect(),
                initializations: AtomicUsize::new(0),
            }
        }
    }

    impl ElementProbe for FakeProbe {
        fn initialize(&self) -> Result<String, String> {
            self.initializations.fetch_add(1, Ordering::SeqCst);
            Ok("1.24.2".to_string())
        }

        fn has_element(&self, name: &str) -> bool {
            self.available.contains(name)
        }
    }

    #[test]
    fn missing_file_preview_elements_are_grouped_with_package_hint() {
        let probe = FakeProbe::with(&["appsrc", "queue", "videoconvert"]);

        let report = diagnose_with_probe(&probe, DiagnosticRequest::file_preview());

        assert!(!report.ready);
        assert_eq!(report.version.as_deref(), Some("1.24.2"));
        assert_eq!(
            report.missing_in(ElementGroup::Core),
            &["capsfilter", "compositor"]
        );
        assert_eq!(
            report.missing_in(ElementGroup::FilePreview),
            &["appsink", "uridecodebin3", "videorate", "videoscale"]
        );
        let message = report.actionable_error("file video preview");
        assert!(message.contains("gstreamer1.0-plugins-base"), "{message}");
        assert!(message.contains("sudo apt-get install"), "{message}");
        assert!(
            message.contains("JPEG previews use the Rust image encoder"),
            "{message}"
        );
    }

    #[test]
    fn fully_available_report_keeps_optional_capture_separate() {
        let probe = FakeProbe::with(&[
            "appsrc",
            "appsink",
            "capsfilter",
            "compositor",
            "queue",
            "uridecodebin3",
            "videoconvert",
            "videoscale",
            "videorate",
            "autovideosink",
        ]);

        let report = diagnose_with_probe(&probe, DiagnosticRequest::file_preview());

        assert!(report.ready, "{}", report.actionable_error("preview"));
        assert!(report.missing_required().is_empty());
        assert_eq!(report.missing_in(ElementGroup::Capture), &["v4l2src"]);
        assert!(report
            .groups
            .iter()
            .find(|group| group.group == ElementGroup::Capture)
            .is_some_and(|group| !group.required));
        assert!(ALL_DEFAULT_ELEMENTS
            .iter()
            .all(|element| *element != "jpegenc"));
    }

    #[test]
    fn capture_request_requires_v4l2_without_requiring_file_decode() {
        let probe = FakeProbe::with(&[
            "appsrc",
            "capsfilter",
            "compositor",
            "queue",
            "videoconvert",
        ]);

        let report = diagnose_with_probe(&probe, DiagnosticRequest::capture_preview());

        let capture = report
            .groups
            .iter()
            .find(|group| group.group == ElementGroup::Capture)
            .unwrap();
        assert!(capture.required);
        #[cfg(target_os = "linux")]
        assert_eq!(capture.missing, ["v4l2src"]);
        assert!(report
            .groups
            .iter()
            .find(|group| group.group == ElementGroup::FilePreview)
            .is_some_and(|group| !group.required));
    }

    #[test]
    fn diagnostics_cache_initializes_and_scans_once() {
        let probe = FakeProbe::with(ALL_DEFAULT_ELEMENTS);
        let cache = DiagnosticsCache::default();

        let first = cache.get_or_init(&probe);
        let second = cache.get_or_init(&probe);

        assert!(std::ptr::eq(first, second));
        assert_eq!(probe.initializations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn file_preview_group_matches_the_factories_used_by_the_engine() {
        let probe = FakeProbe::with(ALL_DEFAULT_ELEMENTS);
        let report = diagnose_with_probe(&probe, DiagnosticRequest::file_preview());
        let file_preview = report
            .groups
            .iter()
            .find(|group| group.group == ElementGroup::FilePreview)
            .unwrap();

        assert_eq!(
            file_preview.available,
            ["appsink", "uridecodebin3", "videorate", "videoscale"]
        );
        assert!(file_preview.required);
        assert_eq!(report.groups.len(), 4);
    }

    #[test]
    fn cached_inventory_does_not_freeze_the_first_program_sink() {
        let probe = FakeProbe::with(&[
            "appsrc",
            "appsink",
            "capsfilter",
            "compositor",
            "queue",
            "uridecodebin3",
            "videoconvert",
            "videoscale",
            "videorate",
            "fakesink",
        ]);
        let cache = DiagnosticsCache::default();

        let first = cache
            .get_or_init(&probe)
            .clone()
            .with_request(&DiagnosticRequest::program("fakesink"), &probe);
        let second = cache
            .get_or_init(&probe)
            .clone()
            .with_request(&DiagnosticRequest::program("waylandsink"), &probe);

        assert!(first.ready);
        assert_eq!(
            second.missing_in(ElementGroup::ProgramOutput),
            &["waylandsink"]
        );
        assert!(!second.ready);
        assert_eq!(probe.initializations.load(Ordering::SeqCst), 1);
    }
}
