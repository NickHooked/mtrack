// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! Optional, worker-threaded GStreamer file-video output.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use parking_lot::Mutex;

use crate::{
    config::video::{VideoClip, VideoConfig, MAX_FRAME_RATE, MIN_FRAME_RATE},
    video::{
        media_binding::bind_timeline,
        timeline::{RuntimeVideoClip, RuntimeVideoTimeline},
    },
};

const FRAME_QUEUE_CAPACITY: usize = 64;
const GST_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);
const DECODER_POLL_INTERVAL: gst::ClockTime = gst::ClockTime::from_mseconds(10);
/// Maximum time `start` waits for the GStreamer worker to report readiness.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum time transport calls and `Drop` wait for a worker to terminate.
/// Transport calls retain a still-running worker so it blocks replacement;
/// `Drop` remains bounded and necessarily detaches it at this deadline because
/// Rust cannot forcibly terminate a thread blocked in native code.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A transport-independent video timeline output.
///
/// Implementations own their worker and never perform GStreamer work on the
/// caller's (potentially real-time audio) thread after `start` returns.
pub trait VideoOutput: Send {
    fn start(&mut self, song_position: Duration) -> Result<(), VideoError>;
    fn play(&mut self) -> Result<(), VideoError>;
    fn pause(&mut self) -> Result<(), VideoError>;
    fn seek(&mut self, song_position: Duration) -> Result<(), VideoError>;
    fn stop(&mut self) -> Result<(), VideoError>;
    fn cancel(&mut self) -> Result<(), VideoError>;
    fn latest_frame(&self) -> Option<VideoFrame>;
    /// Takes the engine's single frame-consumer stream.
    ///
    /// Calling this more than once is a programming error. Receiver clones
    /// compete for frames; they do not broadcast them.
    fn frame_receiver(&self) -> Receiver<VideoFrame>;
    /// Receives failures that occur after `start` has completed successfully.
    fn runtime_error_receiver(&self) -> Receiver<VideoError>;
    fn is_running(&self) -> bool;
    /// Generation of the most recent worker that completed startup.
    fn generation(&self) -> u64;
}

/// One sampled program frame. `pts` is running time since this start; adding it
/// to the start position yields `song_position`.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub generation: u64,
    pub song_position: Duration,
    pub pts: Duration,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone, Debug)]
enum OutputMode {
    Headless,
    Preview,
    Program(String),
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum VideoError {
    #[error("invalid video configuration: {0}")]
    InvalidConfig(String),
    #[error("video clip {clip_id:?} media does not exist: {path}")]
    MissingMedia { clip_id: String, path: PathBuf },
    #[error("video backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error("invalid GStreamer output sink factory name {0:?}")]
    InvalidSinkName(String),
    #[error("failed to initialize GStreamer: {0}")]
    Initialization(String),
    #[error("failed to build {context} pipeline: {message}")]
    PipelineBuild {
        context: &'static str,
        message: String,
    },
    #[error("video worker failed during {context}: {message}")]
    Worker {
        context: &'static str,
        message: String,
    },
    #[error("video worker stopped before startup completed during start")]
    StartupDisconnected,
    #[error("video worker startup timed out after {timeout:?} during start")]
    StartupTimeout { timeout: Duration },
    #[error("video worker thread panicked during {operation}")]
    WorkerPanicked { operation: &'static str },
    #[error("video worker did not stop within {timeout:?} during {operation}; shutdown is still in progress")]
    ShutdownTimeout {
        operation: &'static str,
        timeout: Duration,
    },
}

/// GStreamer file timeline compositor. It is reusable after cancellation or
/// natural EOS; each start receives a monotonically increasing generation.
pub struct GstVideoEngine {
    config: VideoConfig,
    timeline: RuntimeVideoTimeline,
    base_path: PathBuf,
    /// Open descriptors backing trusted `/proc/self/fd` clip paths.
    _bound_media: Vec<std::fs::File>,
    output: OutputMode,
    latest: Arc<Mutex<Option<VideoFrame>>>,
    rendered_position: Arc<Mutex<Duration>>,
    frame_tx: Sender<VideoFrame>,
    frame_rx: Receiver<VideoFrame>,
    frame_receiver_taken: AtomicBool,
    runtime_error_tx: Sender<VideoError>,
    runtime_error_rx: Receiver<VideoError>,
    running: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    active_generation: AtomicU64,
    position: Duration,
    worker: Option<JoinHandle<()>>,
    cancel: Option<Arc<AtomicBool>>,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
    #[cfg(test)]
    worker_entry: fn(WorkerRequest, Sender<Result<(), VideoError>>),
    #[cfg(test)]
    test_worker_channels: Option<(Receiver<()>, Sender<Vec<u8>>)>,
}

impl std::fmt::Debug for GstVideoEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GstVideoEngine")
            .field("base_path", &self.base_path)
            .field("output", &self.output)
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl GstVideoEngine {
    /// Creates deterministic appsink output. No display server is required.
    pub fn headless(config: VideoConfig, base_path: &Path) -> Result<Self, VideoError> {
        Self::new(config, base_path, OutputMode::Headless)
    }

    /// Creates clock-synchronized compositor output for an editor preview.
    pub fn preview(config: VideoConfig, base_path: &Path) -> Result<Self, VideoError> {
        Self::new(config, base_path, OutputMode::Preview)
    }

    /// Creates editor preview output from a structurally validated config
    /// whose clip paths were replaced with canonical in-project targets.
    pub(crate) fn preview_resolved(
        config: VideoConfig,
        base_path: &Path,
        bound_media: Vec<std::fs::File>,
    ) -> Result<Self, VideoError> {
        let mut runtime_config = config.clone();
        let runtime_files = config
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .map(|clip| {
                let target = Path::new(clip.file()).canonicalize().map_err(|error| {
                    VideoError::InvalidConfig(format!(
                        "failed to resolve trusted bound clip {:?}: {error}",
                        clip.id()
                    ))
                })?;
                let relative = target.strip_prefix(base_path).map_err(|_| {
                    VideoError::InvalidConfig(format!(
                        "trusted bound clip {:?} resolves outside preview root {}",
                        clip.id(),
                        base_path.display()
                    ))
                })?;
                Ok(relative.to_string_lossy().into_owned())
            })
            .collect::<Result<Vec<_>, VideoError>>()?;
        runtime_config
            .replace_clip_files(runtime_files)
            .map_err(VideoError::InvalidConfig)?;
        let timeline = RuntimeVideoTimeline::resolve("editor preview", base_path, &runtime_config)
            .map_err(|error| VideoError::InvalidConfig(error.to_string()))?;
        Self::new_bound(
            config,
            timeline,
            base_path,
            OutputMode::Preview,
            bound_media,
        )
    }

    /// Creates program output using `autovideosink` when `sink_factory` is
    /// `None`, or a caller-selected video sink factory otherwise.
    pub fn program(
        config: VideoConfig,
        base_path: &Path,
        sink_factory: Option<&str>,
    ) -> Result<Self, VideoError> {
        let sink = sink_factory.unwrap_or("autovideosink");
        if sink.is_empty()
            || !sink
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(VideoError::InvalidSinkName(sink.to_string()));
        }
        Self::new(config, base_path, OutputMode::Program(sink.to_string()))
    }

    fn new(config: VideoConfig, base_path: &Path, output: OutputMode) -> Result<Self, VideoError> {
        let timeline =
            RuntimeVideoTimeline::resolve("video engine", base_path, &config).map_err(|error| {
                if matches!(
                    error,
                    crate::video::timeline::TimelineError::SongDirectory { .. }
                ) {
                    if let Some(clip) = config
                        .tracks()
                        .iter()
                        .flat_map(|track| track.clips())
                        .next()
                    {
                        return VideoError::MissingMedia {
                            clip_id: clip.id().to_string(),
                            path: base_path.join(clip.file()),
                        };
                    }
                }
                VideoError::InvalidConfig(error.to_string())
            })?;
        let bound =
            bind_timeline(&config, &timeline, "video").map_err(VideoError::InvalidConfig)?;
        Self::new_bound(bound.config, timeline, base_path, output, bound.files)
    }

    fn new_bound(
        config: VideoConfig,
        timeline: RuntimeVideoTimeline,
        base_path: &Path,
        output: OutputMode,
        bound_media: Vec<std::fs::File>,
    ) -> Result<Self, VideoError> {
        check_backend(&output, crate::video::gstreamer::diagnostics)?;

        let frame_queue_capacity = if matches!(output, OutputMode::Preview) {
            1
        } else {
            FRAME_QUEUE_CAPACITY
        };
        let (frame_tx, frame_rx) = bounded(frame_queue_capacity);
        let (runtime_error_tx, runtime_error_rx) = bounded(1);
        Ok(Self {
            config,
            timeline,
            base_path: base_path.to_path_buf(),
            _bound_media: bound_media,
            output,
            latest: Arc::new(Mutex::new(None)),
            rendered_position: Arc::new(Mutex::new(Duration::ZERO)),
            frame_tx,
            frame_rx,
            frame_receiver_taken: AtomicBool::new(false),
            runtime_error_tx,
            runtime_error_rx,
            running: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            active_generation: AtomicU64::new(0),
            position: Duration::ZERO,
            worker: None,
            cancel: None,
            startup_timeout: STARTUP_TIMEOUT,
            shutdown_timeout: SHUTDOWN_TIMEOUT,
            #[cfg(test)]
            worker_entry: run_worker,
            #[cfg(test)]
            test_worker_channels: None,
        })
    }

    fn stop_worker(&mut self, operation: &'static str) -> Result<(), VideoError> {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
        if let Some(worker) = &self.worker {
            let deadline = Instant::now() + self.shutdown_timeout;
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(JOIN_POLL_INTERVAL.min(self.shutdown_timeout));
            }
            if !worker.is_finished() {
                return Err(VideoError::ShutdownTimeout {
                    operation,
                    timeout: self.shutdown_timeout,
                });
            }
        }
        if let Some(worker) = self.worker.take() {
            self.cancel = None;
            worker
                .join()
                .map_err(|_| VideoError::WorkerPanicked { operation })?;
        }
        self.running.store(false, Ordering::Release);
        Ok(())
    }
}

impl VideoOutput for GstVideoEngine {
    fn start(&mut self, song_position: Duration) -> Result<(), VideoError> {
        self.stop_worker("start")?;
        self.position = song_position;
        *self.rendered_position.lock() = song_position;
        *self.latest.lock() = None;
        while self.frame_rx.try_recv().is_ok() {}
        while self.runtime_error_rx.try_recv().is_ok() {}

        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let (startup_tx, startup_rx) = bounded(1);
        let (worker_config, bound_media) = clone_bound_media(&self.config, &self._bound_media)?;
        let request = WorkerRequest {
            config: worker_config,
            timeline: self.timeline.clone(),
            base_path: self.base_path.clone(),
            _bound_media: bound_media,
            output: self.output.clone(),
            song_position,
            generation,
            cancel: Arc::clone(&cancel),
            latest: Arc::clone(&self.latest),
            rendered_position: Arc::clone(&self.rendered_position),
            frame_tx: self.frame_tx.clone(),
            frame_rx: self.frame_rx.clone(),
            latest_only: matches!(self.output, OutputMode::Preview),
            runtime_error_tx: self.runtime_error_tx.clone(),
            #[cfg(test)]
            test_release: self
                .test_worker_channels
                .as_ref()
                .map(|channels| channels.0.clone()),
            #[cfg(test)]
            test_observed: self
                .test_worker_channels
                .as_ref()
                .map(|channels| channels.1.clone()),
        };
        self.running.store(true, Ordering::Release);
        #[cfg(test)]
        let worker_entry = self.worker_entry;
        #[cfg(not(test))]
        let worker_entry = run_worker;
        let worker_running = Arc::clone(&self.running);
        let worker = thread::Builder::new()
            .name(format!("mtrack-video-{generation}"))
            .spawn(move || {
                let _running_guard = WorkerRunningGuard(worker_running);
                worker_entry(request, startup_tx);
            })
            .map_err(|error| {
                self.running.store(false, Ordering::Release);
                VideoError::Worker {
                    context: "thread spawn",
                    message: error.to_string(),
                }
            })?;
        self.cancel = Some(cancel);
        self.worker = Some(worker);

        match startup_rx.recv_timeout(self.startup_timeout) {
            Ok(Ok(())) => {
                self.active_generation.store(generation, Ordering::Release);
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = self.stop_worker("startup failure");
                Err(error)
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = self.stop_worker("startup disconnect");
                Err(VideoError::StartupDisconnected)
            }
            Err(RecvTimeoutError::Timeout) => {
                let timeout = self.startup_timeout;
                let _ = self.stop_worker("startup timeout");
                Err(VideoError::StartupTimeout { timeout })
            }
        }
    }

    fn play(&mut self) -> Result<(), VideoError> {
        if self.is_running() {
            return Ok(());
        }
        // Natural EOS leaves a finished worker and its last presented position.
        // Resume from that position rather than silently replaying stale input.
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            self.position = *self.rendered_position.lock();
        }
        self.start(self.position)
    }

    fn pause(&mut self) -> Result<(), VideoError> {
        self.stop_worker("pause")?;
        self.position = *self.rendered_position.lock();
        Ok(())
    }

    fn seek(&mut self, song_position: Duration) -> Result<(), VideoError> {
        let resume = self.is_running();
        self.stop_worker("seek")?;
        self.position = song_position;
        if resume {
            self.start(song_position)
        } else {
            Ok(())
        }
    }

    fn stop(&mut self) -> Result<(), VideoError> {
        self.stop_worker("stop")?;
        self.position = Duration::ZERO;
        *self.latest.lock() = None;
        while self.frame_rx.try_recv().is_ok() {}
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), VideoError> {
        self.stop_worker("cancel")
    }

    fn latest_frame(&self) -> Option<VideoFrame> {
        self.latest.lock().clone()
    }

    fn frame_receiver(&self) -> Receiver<VideoFrame> {
        assert!(
            !self.frame_receiver_taken.swap(true, Ordering::AcqRel),
            "GstVideoEngine supports exactly one frame receiver"
        );
        self.frame_rx.clone()
    }

    fn runtime_error_receiver(&self) -> Receiver<VideoError> {
        self.runtime_error_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn generation(&self) -> u64 {
        self.active_generation.load(Ordering::Acquire)
    }
}

impl Drop for GstVideoEngine {
    fn drop(&mut self) {
        let _ = self.stop_worker("drop");
    }
}

fn check_backend(
    output: &OutputMode,
    diagnose: impl FnOnce(
        crate::video::gstreamer::DiagnosticRequest,
    ) -> crate::video::gstreamer::GstreamerDiagnostics,
) -> Result<(), VideoError> {
    use crate::video::gstreamer::DiagnosticRequest;

    let (request, context) = match output {
        OutputMode::Headless => (DiagnosticRequest::file_preview(), "headless file video"),
        OutputMode::Preview => (DiagnosticRequest::file_preview(), "file video preview"),
        OutputMode::Program(sink) => (
            DiagnosticRequest::program(sink.clone()),
            "file video program output",
        ),
    };
    let report = diagnose(request);
    if report.ready {
        Ok(())
    } else {
        Err(VideoError::BackendUnavailable(
            report.actionable_error(context),
        ))
    }
}

struct WorkerRequest {
    config: VideoConfig,
    timeline: RuntimeVideoTimeline,
    base_path: PathBuf,
    /// Worker-local descriptors keep every `/proc/self/fd` target alive even
    /// after a timed-out worker detaches from its engine.
    _bound_media: Vec<std::fs::File>,
    output: OutputMode,
    song_position: Duration,
    generation: u64,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<VideoFrame>>>,
    rendered_position: Arc<Mutex<Duration>>,
    frame_tx: Sender<VideoFrame>,
    frame_rx: Receiver<VideoFrame>,
    latest_only: bool,
    runtime_error_tx: Sender<VideoError>,
    #[cfg(test)]
    test_release: Option<Receiver<()>>,
    #[cfg(test)]
    test_observed: Option<Sender<Vec<u8>>>,
}

fn clone_bound_media(
    config: &VideoConfig,
    originals: &[std::fs::File],
) -> Result<(VideoConfig, Vec<std::fs::File>), VideoError> {
    let clones = originals
        .iter()
        .enumerate()
        .map(|(index, file)| {
            file.try_clone().map_err(|error| VideoError::Worker {
                context: "cloning bound media before thread spawn",
                message: format!("descriptor {index}: {error}"),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut worker_config = config.clone();
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let descriptor_paths = originals
            .iter()
            .zip(&clones)
            .map(|(original, clone)| {
                (
                    format!("/proc/self/fd/{}", original.as_raw_fd()),
                    format!("/proc/self/fd/{}", clone.as_raw_fd()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        let files = worker_config
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .map(|clip| {
                descriptor_paths
                    .get(clip.file())
                    .cloned()
                    .unwrap_or_else(|| clip.file().to_string())
            })
            .collect();
        worker_config
            .replace_clip_files(files)
            .map_err(VideoError::InvalidConfig)?;
    }
    Ok((worker_config, clones))
}

struct WorkerRunningGuard(Arc<AtomicBool>);

impl Drop for WorkerRunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn run_worker(request: WorkerRequest, startup: Sender<Result<(), VideoError>>) {
    let result = render_timeline(&request, &startup);
    if let Err(error) = result {
        let _ = startup.try_send(Err(error.clone()));
        let _ = request.runtime_error_tx.try_send(error);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FrameTiming {
    fps_n: i32,
    fps_d: i32,
    period: Duration,
}

fn frame_timing(frame_rate: f64) -> Result<FrameTiming, VideoError> {
    if !frame_rate.is_finite() || !(MIN_FRAME_RATE..=MAX_FRAME_RATE).contains(&frame_rate) {
        return Err(VideoError::InvalidConfig(format!(
            "canvas frame_rate {frame_rate:?} is outside the supported range {MIN_FRAME_RATE}..={MAX_FRAME_RATE} fps"
        )));
    }

    Ok(FrameTiming {
        fps_n: gstreamer_fraction_numerator(frame_rate)?,
        fps_d: 1_000,
        period: frame_period(frame_rate)?,
    })
}

fn gstreamer_fraction_numerator(frame_rate: f64) -> Result<i32, VideoError> {
    let scaled = frame_rate * 1_000.0;
    let rounded = scaled.round();
    if !rounded.is_finite() || rounded < 1.0 || rounded > f64::from(i32::MAX) {
        return Err(VideoError::InvalidConfig(format!(
            "canvas frame_rate {frame_rate:?} cannot be represented as a positive GStreamer fraction numerator"
        )));
    }
    Ok(rounded as i32)
}

fn frame_period(frame_rate: f64) -> Result<Duration, VideoError> {
    let period = Duration::try_from_secs_f64(1.0 / frame_rate).map_err(|error| {
        VideoError::InvalidConfig(format!(
            "canvas frame_rate {frame_rate:?} has an unrepresentable frame period: {error}"
        ))
    })?;
    if period.is_zero() {
        return Err(VideoError::InvalidConfig(format!(
            "canvas frame_rate {frame_rate:?} has a zero frame period"
        )));
    }
    Ok(period)
}

fn render_timeline(
    request: &WorkerRequest,
    startup: &Sender<Result<(), VideoError>>,
) -> Result<(), VideoError> {
    let canvas = request.config.canvas();
    let frame_rate = canvas.frame_rate();
    let timing = frame_timing(frame_rate)?;
    let fps_n = timing.fps_n;
    let fps_d = timing.fps_d;
    let period = timing.period;
    let width = canvas.width();
    let height = canvas.height();
    let output_pipeline = build_output_pipeline(
        &request.output,
        request.config.tracks().len(),
        width,
        height,
        fps_n,
        fps_d,
    )?;
    let pipeline = &output_pipeline.pipeline;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| worker_error("starting output", error))?;
    startup
        .send(Ok(()))
        .map_err(|_| VideoError::StartupDisconnected)?;

    let background = solid_frame(width, height, parse_color(canvas.background())?)?;
    let transparent = vec![0; rgba_buffer_len(width, height)?];
    let timeline_end = request.timeline.end();
    let mut decoders: Vec<Option<ClipDecoder>> =
        (0..request.config.tracks().len()).map(|_| None).collect();
    let mut active_ids: Vec<Option<String>> = vec![None; request.config.tracks().len()];
    let mut frame_index = 0u64;

    'render: while !request.cancel.load(Ordering::Acquire) {
        let running_pts = period.mul_f64(frame_index as f64);
        let song_position = request.song_position.saturating_add(running_pts);
        let preview_first_frame = matches!(request.output, OutputMode::Preview) && frame_index == 0;
        if song_position >= timeline_end && !preview_first_frame {
            break;
        }
        if output_pipeline.synchronized
            && !wait_for_program_lead(request, pipeline, running_pts, period)
        {
            break;
        }

        push_frame(
            &output_pipeline.sources[0],
            &background,
            running_pts,
            period,
        )?;
        for (track_index, track) in request.config.tracks().iter().enumerate() {
            let active = request
                .timeline
                .active_clips_at(song_position)
                .into_iter()
                .find(|clip| clip.z_order() == track_index);
            let pixels = if let Some(runtime_clip) = active {
                let clip = track
                    .clips()
                    .iter()
                    .find(|clip| clip.id() == runtime_clip.id())
                    .expect("bound config retains every runtime clip");
                configure_clip_pad(
                    &output_pipeline.track_pads[track_index],
                    runtime_clip,
                    song_position,
                    width,
                    height,
                );
                if active_ids[track_index].as_deref() != Some(clip.id()) {
                    let decoder = ClipDecoder::open(
                        clip,
                        runtime_clip,
                        &request.base_path,
                        width,
                        height,
                        fps_n,
                        fps_d,
                        song_position,
                        &request.cancel,
                    )?;
                    decoders[track_index] = match decoder {
                        DecoderPoll::Ready(decoder) => Some(decoder),
                        DecoderPoll::Cancelled => break 'render,
                    };
                    active_ids[track_index] = Some(clip.id().to_string());
                }
                match decoders[track_index]
                    .as_mut()
                    .expect("decoder created for active clip")
                    .next_frame(&request.cancel)?
                {
                    DecoderPoll::Ready(frame) => frame.unwrap_or_else(|| transparent.clone()),
                    DecoderPoll::Cancelled => break 'render,
                }
            } else {
                output_pipeline.track_pads[track_index].set_property("alpha", 0.0_f64);
                decoders[track_index] = None;
                active_ids[track_index] = None;
                transparent.clone()
            };
            push_frame(
                &output_pipeline.sources[track_index + 1],
                &pixels,
                running_pts,
                period,
            )?;
        }

        if let Some(sink) = &output_pipeline.appsink {
            let Some(sample) = pull_composed_sample(request, pipeline, sink, period)? else {
                break;
            };
            let buffer = sample.buffer().ok_or_else(|| VideoError::Worker {
                context: "reading composed frame",
                message: "appsink sample had no buffer".to_string(),
            })?;
            let map = buffer.map_readable().map_err(|error| VideoError::Worker {
                context: "mapping composed frame",
                message: error.to_string(),
            })?;
            let frame = VideoFrame {
                generation: request.generation,
                song_position,
                pts: running_pts,
                width,
                height,
                rgba: Arc::from(map.as_slice()),
            };
            *request.latest.lock() = Some(frame.clone());
            deliver_frame(request, frame);
        }
        if output_pipeline.appsink.is_some() {
            *request.rendered_position.lock() = song_position;
        } else if output_pipeline.program {
            update_program_position(
                pipeline,
                request.song_position,
                timeline_end,
                &request.rendered_position,
            );
        }
        frame_index += 1;
    }

    let cancelled = request.cancel.load(Ordering::Acquire);
    if !cancelled && output_pipeline.appsink.is_some() {
        // The last emitted frame is end-exclusive, but transport resume must
        // begin at EOS rather than replaying that final frame.
        *request.rendered_position.lock() = timeline_end;
    }
    if output_pipeline.program && output_pipeline.appsink.is_none() {
        update_program_position(
            pipeline,
            request.song_position,
            timeline_end,
            &request.rendered_position,
        );
    }
    for source in &output_pipeline.sources {
        let _ = source.end_of_stream();
    }
    if output_pipeline.program && output_pipeline.appsink.is_none() && !cancelled {
        wait_for_program_eos(request, pipeline, timeline_end)?;
    }
    for decoder in decoders.into_iter().flatten() {
        decoder.stop();
    }
    Ok(())
}

fn pull_composed_sample(
    request: &WorkerRequest,
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    frame_period: Duration,
) -> Result<Option<gst::Sample>, VideoError> {
    pull_composed_sample_with_timeout(
        request,
        pipeline,
        sink,
        frame_period,
        Duration::from_nanos(GST_TIMEOUT.nseconds()),
    )
}

fn pull_composed_sample_with_timeout(
    request: &WorkerRequest,
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    frame_period: Duration,
    stall_timeout: Duration,
) -> Result<Option<gst::Sample>, VideoError> {
    let deadline = Instant::now() + frame_period + stall_timeout;
    let context = match request.output {
        OutputMode::Headless => "pulling composed headless output frame",
        OutputMode::Preview => "pulling composed preview output frame",
        OutputMode::Program(_) => "pulling composed program output frame",
    };
    while !request.cancel.load(Ordering::Acquire) {
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
            return Ok(Some(sample));
        }
        if output_bus_reached_eos(pipeline, context)? || sink.is_eos() {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(VideoError::Worker {
                context,
                message: "timed out waiting for a composed frame".to_string(),
            });
        }
    }
    Ok(None)
}

fn output_bus_reached_eos(
    pipeline: &gst::Pipeline,
    context: &'static str,
) -> Result<bool, VideoError> {
    let bus = pipeline.bus().ok_or_else(|| VideoError::Worker {
        context,
        message: "output pipeline has no bus".to_string(),
    })?;
    while let Some(message) = bus.pop() {
        match message.view() {
            gst::MessageView::Eos(_) => return Ok(true),
            gst::MessageView::Error(error) => {
                let source = error
                    .src()
                    .map(|source| source.path_string())
                    .unwrap_or_else(|| "unknown source".into());
                let debug = error
                    .debug()
                    .map(|debug| format!("; debug: {debug}"))
                    .unwrap_or_default();
                return Err(VideoError::Worker {
                    context,
                    message: format!("{} (source {source}){debug}", error.error()),
                });
            }
            _ => {}
        }
    }
    Ok(false)
}

fn deliver_frame(request: &WorkerRequest, frame: VideoFrame) {
    if !request.latest_only {
        let _ = request.frame_tx.try_send(frame);
        return;
    }

    let mut pending = frame;
    loop {
        match request.frame_tx.try_send(pending) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(frame)) => {
                pending = frame;
                let _ = request.frame_rx.try_recv();
            }
        }
    }
}

fn wait_for_program_lead(
    request: &WorkerRequest,
    pipeline: &gst::Pipeline,
    frame_pts: Duration,
    lead: Duration,
) -> bool {
    let target = frame_pts.saturating_sub(lead);
    if target.is_zero() {
        return !request.cancel.load(Ordering::Acquire);
    }
    loop {
        if request.cancel.load(Ordering::Acquire) {
            return false;
        }
        if let (Some(clock), Some(base_time)) = (pipeline.clock(), pipeline.base_time()) {
            let running_nanos = clock.time().nseconds().saturating_sub(base_time.nseconds());
            if Duration::from_nanos(running_nanos) >= target {
                return true;
            }
        }
        thread::sleep(JOIN_POLL_INTERVAL);
    }
}

fn update_program_position(
    pipeline: &gst::Pipeline,
    song_start: Duration,
    timeline_end: Duration,
    rendered_position: &Mutex<Duration>,
) {
    let Some(position) = pipeline.query_position::<gst::ClockTime>() else {
        return;
    };
    let running_position = Duration::from_nanos(position.nseconds());
    *rendered_position.lock() = song_start
        .saturating_add(running_position)
        .min(timeline_end);
}

fn wait_for_program_eos(
    request: &WorkerRequest,
    pipeline: &gst::Pipeline,
    timeline_end: Duration,
) -> Result<(), VideoError> {
    let bus = pipeline.bus().ok_or_else(|| VideoError::Worker {
        context: "waiting for program output EOS",
        message: "output pipeline has no bus".to_string(),
    })?;
    let remaining_timeline = timeline_end.saturating_sub(request.song_position);
    let deadline = Instant::now() + remaining_timeline + Duration::from_secs(5);
    loop {
        if request.cancel.load(Ordering::Acquire) {
            update_program_position(
                pipeline,
                request.song_position,
                timeline_end,
                &request.rendered_position,
            );
            return Ok(());
        }
        update_program_position(
            pipeline,
            request.song_position,
            timeline_end,
            &request.rendered_position,
        );
        if let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(10)) {
            match message.view() {
                gst::MessageView::Eos(_) => {
                    *request.rendered_position.lock() = timeline_end;
                    return Ok(());
                }
                gst::MessageView::Error(error) => {
                    let source = error
                        .src()
                        .map(|source| source.path_string())
                        .unwrap_or_else(|| "unknown source".into());
                    let debug = error
                        .debug()
                        .map(|debug| format!("; debug: {debug}"))
                        .unwrap_or_default();
                    return Err(VideoError::Worker {
                        context: "waiting for program output EOS",
                        message: format!("{} (source {source}){debug}", error.error()),
                    });
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(VideoError::Worker {
                context: "waiting for program output EOS",
                message: "timed out before the synchronized sink reached EOS".to_string(),
            });
        }
    }
}

struct OutputPipeline {
    pipeline: PipelineGuard,
    sources: Vec<gst_app::AppSrc>,
    track_pads: Vec<gst::Pad>,
    appsink: Option<gst_app::AppSink>,
    program: bool,
    synchronized: bool,
}

fn build_output_pipeline(
    output: &OutputMode,
    track_count: usize,
    width: u32,
    height: u32,
    fps_n: i32,
    fps_d: i32,
) -> Result<OutputPipeline, VideoError> {
    let caps =
        format!("video/x-raw,format=RGBA,width={width},height={height},framerate={fps_n}/{fps_d}");
    let mut branches = Vec::with_capacity(track_count + 1);
    for index in 0..=track_count {
        branches.push(format!(
            "appsrc name=source{index} is-live=false format=time block=false max-buffers=2 caps=\"{caps}\" ! queue max-size-buffers=2 ! mix.sink_{index}"
        ));
    }
    let sink = match output {
        OutputMode::Headless => {
            format!("appsink name=program sync=false max-buffers=1 drop=true caps=\"{caps}\"")
        }
        OutputMode::Preview => {
            format!("appsink name=program sync=true max-buffers=1 drop=true caps=\"{caps}\"")
        }
        OutputMode::Program(factory) => format!("{factory} name=program sync=true"),
    };
    let description = format!(
        "{} compositor name=mix background=transparent ! videoconvert ! {caps} ! {sink}",
        branches.join(" ")
    );
    let element = gst::parse::launch(&description).map_err(|error| VideoError::PipelineBuild {
        context: "program output",
        message: error.to_string(),
    })?;
    let pipeline = element
        .downcast::<gst::Pipeline>()
        .map_err(|_| VideoError::PipelineBuild {
            context: "program output",
            message: "description did not produce a pipeline".to_string(),
        })?;
    let mut sources = Vec::with_capacity(track_count + 1);
    for index in 0..=track_count {
        let source = pipeline
            .by_name(&format!("source{index}"))
            .and_then(|element| element.downcast::<gst_app::AppSrc>().ok())
            .ok_or_else(|| VideoError::PipelineBuild {
                context: "program output",
                message: format!("appsrc source{index} was not created"),
            })?;
        sources.push(source);
    }
    let appsink = pipeline
        .by_name("program")
        .and_then(|element| element.downcast::<gst_app::AppSink>().ok());
    let compositor = pipeline
        .by_name("mix")
        .ok_or_else(|| VideoError::PipelineBuild {
            context: "program output",
            message: "compositor was not created".to_string(),
        })?;
    for pad in compositor.sink_pads() {
        let Some(index) = pad
            .name()
            .strip_prefix("sink_")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        pad.set_property("zorder", index);
    }
    let track_pads = (1..=track_count)
        .map(|index| {
            compositor
                .static_pad(&format!("sink_{index}"))
                .ok_or_else(|| VideoError::PipelineBuild {
                    context: "program output",
                    message: format!("compositor pad sink_{index} was not created"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OutputPipeline {
        pipeline: PipelineGuard(pipeline),
        sources,
        track_pads,
        appsink,
        program: matches!(output, OutputMode::Program(_)),
        synchronized: matches!(output, OutputMode::Preview | OutputMode::Program(_)),
    })
}

fn configure_clip_pad(
    pad: &gst::Pad,
    clip: &RuntimeVideoClip,
    song_position: Duration,
    canvas_width: u32,
    canvas_height: u32,
) {
    pad.set_property("xpos", clip.x() as i32);
    pad.set_property("ypos", clip.y() as i32);
    pad.set_property("width", clip.width().unwrap_or(canvas_width) as i32);
    pad.set_property("height", clip.height().unwrap_or(canvas_height) as i32);
    pad.set_property("alpha", clip.opacity_at(song_position));
}

struct ClipDecoder {
    pipeline: PipelineGuard,
    sink: gst_app::AppSink,
    clip_id: String,
    path: PathBuf,
    bus_eos: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum DecoderPoll<T> {
    Ready(T),
    Cancelled,
}

fn wait_for_decoder_preroll(
    pipeline: &gst::Pipeline,
    cancel: &AtomicBool,
    clip_id: &str,
    path: &Path,
) -> Result<DecoderPoll<()>, VideoError> {
    let deadline = Instant::now() + Duration::from_nanos(GST_TIMEOUT.nseconds());
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(DecoderPoll::Cancelled);
        }
        let (state_result, current, pending) = pipeline.state(DECODER_POLL_INTERVAL);
        if cancel.load(Ordering::Acquire) {
            return Ok(DecoderPoll::Cancelled);
        }
        state_result.map_err(|error| VideoError::Worker {
            context: "prerolling file decoder",
            message: format!(
                "clip {clip_id:?} file {} did not reach paused state (current {current:?}, pending {pending:?}): {error}",
                path.display()
            ),
        })?;
        if current == gst::State::Paused {
            return Ok(DecoderPoll::Ready(()));
        }
        if Instant::now() >= deadline {
            return Err(VideoError::Worker {
                context: "prerolling file decoder",
                message: format!(
                    "clip {clip_id:?} file {} timed out before paused state (current {current:?}, pending {pending:?})",
                    path.display()
                ),
            });
        }
    }
}

impl ClipDecoder {
    // Decoder construction mirrors the explicit factories and caps in the
    // pipeline string; keeping those values together makes diagnostics and
    // pipeline requirements auditable.
    #[allow(clippy::too_many_arguments)]
    fn open(
        clip: &VideoClip,
        runtime_clip: &RuntimeVideoClip,
        base_path: &Path,
        width: u32,
        height: u32,
        fps_n: i32,
        fps_d: i32,
        song_position: Duration,
        cancel: &AtomicBool,
    ) -> Result<DecoderPoll<Self>, VideoError> {
        let uri_path = base_path.join(clip.file());
        let path = runtime_clip.file().to_path_buf();
        let uri = gst::glib::filename_to_uri(&uri_path, None).map_err(|error| {
            VideoError::PipelineBuild {
                context: "file decoder",
                message: format!("clip {:?} path {}: {error}", clip.id(), path.display()),
            }
        })?;
        let caps = format!(
            "video/x-raw,format=RGBA,width={width},height={height},framerate={fps_n}/{fps_d}"
        );
        let description = format!(
            "uridecodebin3 uri=\"{uri}\" name=decode decode. ! queue ! videoconvert ! videoscale ! videorate ! {caps} ! appsink name=decoded sync=false max-buffers=2 drop=false"
        );
        let element =
            gst::parse::launch(&description).map_err(|error| VideoError::PipelineBuild {
                context: "file decoder",
                message: format!("clip {:?}: {error}", clip.id()),
            })?;
        let pipeline =
            element
                .downcast::<gst::Pipeline>()
                .map_err(|_| VideoError::PipelineBuild {
                    context: "file decoder",
                    message: format!(
                        "clip {:?}: description did not produce a pipeline",
                        clip.id()
                    ),
                })?;
        let pipeline = PipelineGuard(pipeline);
        let sink = pipeline
            .by_name("decoded")
            .and_then(|element| element.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| VideoError::PipelineBuild {
                context: "file decoder",
                message: format!("clip {:?}: appsink was not created", clip.id()),
            })?;
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|error| VideoError::Worker {
                context: "pausing file decoder",
                message: format!("clip {:?} file {}: {error}", clip.id(), path.display()),
            })?;
        if wait_for_decoder_preroll(&pipeline, cancel, clip.id(), &path)? == DecoderPoll::Cancelled
        {
            return Ok(DecoderPoll::Cancelled);
        }
        let seek = runtime_clip.source_position_at(song_position);
        pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds(seek.as_nanos().min(u64::MAX as u128) as u64),
            )
            .map_err(|error| VideoError::Worker {
                context: "seeking file clip",
                message: format!(
                    "clip {:?} file {} to {seek:?}: {error}",
                    clip.id(),
                    path.display()
                ),
            })?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| VideoError::Worker {
                context: "starting file decoder",
                message: format!("clip {:?} file {}: {error}", clip.id(), path.display()),
            })?;
        Ok(DecoderPoll::Ready(Self {
            pipeline,
            sink,
            clip_id: clip.id().to_string(),
            path,
            bus_eos: false,
        }))
    }

    fn next_frame(
        &mut self,
        cancel: &AtomicBool,
    ) -> Result<DecoderPoll<Option<Vec<u8>>>, VideoError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Ok(DecoderPoll::Cancelled);
            }
            self.inspect_bus()?;
            if let Some(sample) = self.sink.try_pull_sample(DECODER_POLL_INTERVAL) {
                let buffer = sample.buffer().ok_or_else(|| VideoError::Worker {
                    context: "reading decoded frame",
                    message: format!(
                        "clip {:?} file {}: appsink sample had no buffer",
                        self.clip_id,
                        self.path.display()
                    ),
                })?;
                let map = buffer.map_readable().map_err(|error| VideoError::Worker {
                    context: "mapping decoded frame",
                    message: format!(
                        "clip {:?} file {}: {error}",
                        self.clip_id,
                        self.path.display()
                    ),
                })?;
                return Ok(DecoderPoll::Ready(Some(map.as_slice().to_vec())));
            }

            if cancel.load(Ordering::Acquire) {
                return Ok(DecoderPoll::Cancelled);
            }
            self.inspect_bus()?;
            if self.sink.is_eos() || self.bus_eos {
                return Ok(DecoderPoll::Ready(None));
            }
            if Instant::now() >= deadline {
                return Err(VideoError::Worker {
                    context: "pulling decoded frame",
                    message: format!(
                        "clip {:?} file {} timed out waiting for a decoded frame",
                        self.clip_id,
                        self.path.display()
                    ),
                });
            }
        }
    }

    fn inspect_bus(&mut self) -> Result<(), VideoError> {
        let Some(bus) = self.pipeline.bus() else {
            return Ok(());
        };
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Error(error) => {
                    let source = error
                        .src()
                        .map(|source| source.path_string())
                        .unwrap_or_else(|| "unknown source".into());
                    let debug = error
                        .debug()
                        .map(|debug| format!("; debug: {debug}"))
                        .unwrap_or_default();
                    return Err(VideoError::Worker {
                        context: "decoding file clip",
                        message: format!(
                            "clip {:?} file {}: {} (source {source}){debug}",
                            self.clip_id,
                            self.path.display(),
                            error.error()
                        ),
                    });
                }
                gst::MessageView::Eos(_) => self.bus_eos = true,
                _ => {}
            }
        }
        Ok(())
    }

    fn stop(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

struct PipelineGuard(gst::Pipeline);

impl std::ops::Deref for PipelineGuard {
    type Target = gst::Pipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for PipelineGuard {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

fn push_frame(
    source: &gst_app::AppSrc,
    pixels: &[u8],
    pts: Duration,
    duration: Duration,
) -> Result<(), VideoError> {
    let mut buffer = gst::Buffer::with_size(pixels.len()).map_err(|error| VideoError::Worker {
        context: "allocating input frame",
        message: error.to_string(),
    })?;
    {
        let buffer = buffer.get_mut().expect("new buffer is uniquely owned");
        buffer.set_pts(gst::ClockTime::from_nseconds(
            pts.as_nanos().min(u64::MAX as u128) as u64,
        ));
        buffer.set_duration(gst::ClockTime::from_nseconds(
            duration.as_nanos().min(u64::MAX as u128) as u64,
        ));
        let mut map = buffer.map_writable().map_err(|error| VideoError::Worker {
            context: "mapping input frame",
            message: error.to_string(),
        })?;
        map.as_mut_slice().copy_from_slice(pixels);
    }
    source
        .push_buffer(buffer)
        .map(|_| ())
        .map_err(|error| VideoError::Worker {
            context: "pushing input frame",
            message: error.to_string(),
        })
}

fn rgba_buffer_len(width: u32, height: u32) -> Result<usize, VideoError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            VideoError::InvalidConfig(format!(
                "canvas RGBA allocation size overflows for {width}x{height}"
            ))
        })
}

fn solid_frame(width: u32, height: u32, color: [u8; 4]) -> Result<Vec<u8>, VideoError> {
    let mut frame = vec![0; rgba_buffer_len(width, height)?];
    for pixel in frame.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    Ok(frame)
}

#[cfg(test)]
fn active_clip(clips: &[VideoClip], song_position: Duration) -> Option<&VideoClip> {
    clips.iter().find(|clip| {
        let start = Duration::from_secs_f64(clip.start());
        let end = Duration::from_secs_f64(clip.end());
        song_position >= start && song_position < end
    })
}

#[cfg(test)]
fn source_position(clip: &VideoClip, song_position: Duration) -> Duration {
    Duration::from_secs_f64(clip.source_in())
        .saturating_add(song_position.saturating_sub(Duration::from_secs_f64(clip.start())))
}

fn parse_color(color: &str) -> Result<[u8; 4], VideoError> {
    let value = u32::from_str_radix(&color[1..], 16)
        .map_err(|_| VideoError::InvalidConfig(format!("invalid canvas color {color:?}")))?;
    Ok([
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
        0,
    ])
}

fn worker_error(context: &'static str, error: impl std::fmt::Display) -> VideoError {
    VideoError::Worker {
        context,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Config, File, FileFormat};
    use std::process::Command;

    fn config(yaml: &str) -> VideoConfig {
        Config::builder()
            .add_source(File::from_str(yaml, FileFormat::Yaml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[test]
    fn backend_check_reports_grouped_diagnostics_before_pipeline_build() {
        let error = check_backend(&OutputMode::Preview, |_| {
            use crate::video::gstreamer::{
                ElementGroup, ElementGroupDiagnostics, GstreamerDiagnostics,
            };
            GstreamerDiagnostics {
                initialized: true,
                initialization_error: None,
                version: Some("1.24.2".to_string()),
                feature: "video",
                target_os: "linux",
                target_arch: "x86_64",
                groups: vec![ElementGroupDiagnostics {
                    group: ElementGroup::Core,
                    required: true,
                    available: vec!["appsrc".to_string()],
                    missing: vec!["compositor".to_string()],
                }],
                ready: false,
                jpeg_backend: "Rust image crate",
            }
        })
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("file video preview"), "{message}");
        assert!(message.contains("compositor"), "{message}");
        assert!(message.contains("sudo apt-get install"), "{message}");
    }

    #[test]
    fn worker_request_rebinds_media_to_worker_owned_descriptor() {
        fn delayed_open(request: WorkerRequest, startup: Sender<Result<(), VideoError>>) {
            let path = request.config.tracks()[0].clips()[0].file().to_string();
            startup.send(Ok(())).unwrap();
            let release = request.test_release.as_ref().unwrap();
            release.recv().unwrap();
            let bytes = std::fs::read(path).unwrap();
            request.test_observed.as_ref().unwrap().send(bytes).unwrap();
        }

        let directory = tempfile::tempdir().unwrap();
        let media_path = directory.path().join("media.mkv");
        std::fs::write(&media_path, b"original inode").unwrap();
        let video = config(
            r##"canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: [{ name: screen, clips: [{ id: clip, start: 0, duration: 1, file: media.mkv }] }]"##,
        );
        let mut engine = GstVideoEngine::headless(video, directory.path()).unwrap();
        assert_eq!(engine.timeline.clips()[0].file(), media_path.as_path());
        let (release_tx, release_rx) = bounded(1);
        let (observed_tx, observed_rx) = bounded(1);
        engine.worker_entry = delayed_open;
        engine.test_worker_channels = Some((release_rx, observed_tx));
        engine.shutdown_timeout = Duration::from_millis(10);
        engine.start(Duration::ZERO).unwrap();

        drop(engine);
        std::fs::remove_file(&media_path).unwrap();
        std::fs::write(&media_path, b"replacement inode").unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            b"original inode"
        );
    }

    #[test]
    fn start_times_out_when_worker_never_reports_startup() {
        fn stall_startup(_request: WorkerRequest, _startup: Sender<Result<(), VideoError>>) {
            // Simulate a native call that ignores cancellation long enough to
            // exceed both lifecycle deadlines, then end so the test leaks no thread.
            thread::sleep(Duration::from_millis(250));
        }

        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
        );
        let directory = tempfile::tempdir().unwrap();
        let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
        engine.worker_entry = stall_startup;
        engine.startup_timeout = Duration::from_millis(50);
        engine.shutdown_timeout = Duration::from_millis(50);
        let (result_tx, result_rx) = bounded(1);

        thread::spawn(move || {
            let result = engine.start(Duration::ZERO);
            result_tx.send(result).unwrap();
        });

        let error = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("start call exceeded its documented deadline")
            .unwrap_err();
        assert!(matches!(error, VideoError::StartupTimeout { .. }));
    }

    #[test]
    fn active_generation_advances_only_after_successful_start() {
        fn reject_startup(_request: WorkerRequest, startup: Sender<Result<(), VideoError>>) {
            startup
                .send(Err(VideoError::Worker {
                    context: "controlled startup",
                    message: "rejected".to_string(),
                }))
                .unwrap();
        }

        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
        );
        let directory = tempfile::tempdir().unwrap();
        let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
        assert_eq!(engine.generation(), 0);
        engine.start(Duration::ZERO).unwrap();
        assert_eq!(engine.generation(), 1);
        engine.cancel().unwrap();

        engine.worker_entry = reject_startup;
        assert!(engine.start(Duration::ZERO).is_err());
        assert_eq!(engine.generation(), 1);
    }

    fn engine_with_stuck_worker() -> (GstVideoEngine, Sender<()>) {
        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
        );
        let directory = tempfile::tempdir().unwrap();
        let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
        let (release_tx, release_rx) = bounded(1);
        engine.cancel = Some(Arc::new(AtomicBool::new(false)));
        engine.worker = Some(thread::spawn(move || {
            let _ = release_rx.recv();
        }));
        engine.running.store(true, Ordering::Release);
        engine.shutdown_timeout = Duration::from_millis(50);
        (engine, release_tx)
    }

    #[test]
    fn timed_out_worker_blocks_replacement_until_it_can_be_reaped() {
        let (mut engine, release) = engine_with_stuck_worker();

        let first = engine.cancel().unwrap_err();
        assert!(matches!(first, VideoError::ShutdownTimeout { .. }));
        assert!(engine.worker.is_some(), "timed-out worker handle was lost");
        assert!(
            engine.is_running(),
            "live timed-out worker was marked stopped"
        );

        let generation = engine.generation.load(Ordering::Acquire);
        let restart = engine.start(Duration::ZERO).unwrap_err();
        assert!(matches!(
            restart,
            VideoError::ShutdownTimeout {
                operation: "start",
                ..
            }
        ));
        assert_eq!(engine.generation.load(Ordering::Acquire), generation);
        assert!(engine.worker.is_some(), "restart discarded the old worker");

        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !engine.worker.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        engine.worker_entry = run_worker;
        engine.start(Duration::ZERO).unwrap();
        assert_eq!(engine.generation.load(Ordering::Acquire), generation + 1);
    }

    #[test]
    fn worker_panic_resets_running_and_allows_a_later_restart() {
        fn panic_worker(_request: WorkerRequest, _startup: Sender<Result<(), VideoError>>) {
            panic!("controlled worker panic");
        }

        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
        );
        let directory = tempfile::tempdir().unwrap();
        let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
        engine.worker_entry = panic_worker;

        assert!(engine.start(Duration::ZERO).is_err());
        assert!(!engine.is_running(), "panic left lifecycle marked running");
        engine.worker_entry = run_worker;
        engine.start(Duration::ZERO).unwrap();
    }

    #[test]
    fn drop_is_bounded_when_worker_ignores_cancellation() {
        let (engine, release) = engine_with_stuck_worker();
        let (dropped_tx, dropped_rx) = bounded(1);
        thread::spawn(move || {
            drop(engine);
            dropped_tx.send(()).unwrap();
        });
        dropped_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("drop exceeded its shutdown deadline");
        release.send(()).unwrap();
    }

    #[test]
    fn decoder_preroll_observes_cancellation() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch("appsrc ! fakesink")
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        pipeline.set_state(gst::State::Paused).unwrap();
        let pipeline = PipelineGuard(pipeline);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_from_thread = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            cancel_from_thread.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let outcome = wait_for_decoder_preroll(
            &pipeline,
            &cancel,
            "stalled-preroll",
            Path::new("stalled-preroll.mkv"),
        )
        .unwrap();

        assert_eq!(outcome, DecoderPoll::Cancelled);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "decoder preroll ignored cancellation for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn preview_cancel_interrupts_stalled_source_frame_acquisition() {
        fn stall_on_source_frame(request: WorkerRequest, startup: Sender<Result<(), VideoError>>) {
            gst::init().unwrap();
            let pipeline = gst::parse::launch(
                "appsrc name=source ! appsink name=decoded sync=false max-buffers=2 drop=false",
            )
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
            let sink = pipeline
                .by_name("decoded")
                .unwrap()
                .downcast::<gst_app::AppSink>()
                .unwrap();
            pipeline.set_state(gst::State::Playing).unwrap();
            let mut decoder = ClipDecoder {
                pipeline: PipelineGuard(pipeline.clone()),
                sink,
                clip_id: "stalled-source".to_string(),
                path: PathBuf::from("stalled-source.mkv"),
                bus_eos: false,
            };

            startup.send(Ok(())).unwrap();
            let result = decoder.next_frame(&request.cancel);
            decoder.stop();
            assert_eq!(pipeline.current_state(), gst::State::Null);
            match result {
                Ok(DecoderPoll::Cancelled) => {}
                Ok(outcome) => panic!("stalled source returned unexpected outcome: {outcome:?}"),
                Err(error) => {
                    let _ = request.runtime_error_tx.try_send(error);
                }
            }
        }

        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
        );
        let directory = tempfile::tempdir().unwrap();
        let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
        let errors = engine.runtime_error_receiver();
        engine.worker_entry = stall_on_source_frame;
        engine.start(Duration::ZERO).unwrap();

        let started = Instant::now();
        engine.cancel().unwrap();

        assert!(
            started.elapsed() < SHUTDOWN_TIMEOUT,
            "source acquisition exceeded preview shutdown bound: {:?}",
            started.elapsed()
        );
        assert!(
            errors.try_recv().is_err(),
            "cancellation was reported as a runtime decoder error"
        );
    }

    #[test]
    fn decoder_reports_bus_error_after_decoding_has_started() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(
            "appsrc name=source ! appsink name=decoded sync=false max-buffers=2 drop=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let source = pipeline
            .by_name("source")
            .unwrap()
            .downcast::<gst_app::AppSrc>()
            .unwrap();
        let sink = pipeline
            .by_name("decoded")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        source
            .push_buffer(gst::Buffer::from_slice(vec![1, 2, 3, 4]))
            .unwrap();
        let mut decoder = ClipDecoder {
            pipeline: PipelineGuard(pipeline.clone()),
            sink,
            clip_id: "late-failure".to_string(),
            path: PathBuf::from("late-failure.mkv"),
            bus_eos: false,
        };

        let cancel = AtomicBool::new(false);
        assert_eq!(
            decoder.next_frame(&cancel).unwrap(),
            DecoderPoll::Ready(Some(vec![1, 2, 3, 4]))
        );
        let failing_pipeline = pipeline.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            failing_pipeline
                .post_message(
                    gst::message::Error::builder(
                        gst::StreamError::Decode,
                        "controlled late decoder failure",
                    )
                    .src(&failing_pipeline)
                    .debug("controlled test decoder error")
                    .build(),
                )
                .unwrap();
        });

        let started = Instant::now();
        let error = decoder.next_frame(&cancel).unwrap_err().to_string();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "decoder bus error was delayed for {:?}",
            started.elapsed()
        );
        assert!(error.contains("late-failure"), "{error}");
        assert!(error.contains("late-failure.mkv"), "{error}");
        assert!(error.contains("decod"), "{error}");
        assert!(error.contains("controlled late decoder failure"), "{error}");
    }

    #[test]
    fn decoder_treats_clean_eos_as_end_of_media() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(
            "appsrc name=source ! appsink name=decoded sync=false max-buffers=2 drop=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let source = pipeline
            .by_name("source")
            .unwrap()
            .downcast::<gst_app::AppSrc>()
            .unwrap();
        let sink = pipeline
            .by_name("decoded")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        source
            .push_buffer(gst::Buffer::from_slice(vec![1, 2, 3, 4]))
            .unwrap();
        let mut decoder = ClipDecoder {
            pipeline: PipelineGuard(pipeline),
            sink,
            clip_id: "clean-eos".to_string(),
            path: PathBuf::from("clean-eos.mkv"),
            bus_eos: false,
        };

        let cancel = AtomicBool::new(false);
        assert!(matches!(
            decoder.next_frame(&cancel).unwrap(),
            DecoderPoll::Ready(Some(_))
        ));
        source.end_of_stream().unwrap();
        assert_eq!(
            decoder.next_frame(&cancel).unwrap(),
            DecoderPoll::Ready(None)
        );
    }

    fn make_test_video(directory: &Path, duration_frames: &str) {
        let path = directory.join("test.mkv");
        let status = Command::new("timeout")
            .args([
                "10s",
                "gst-launch-1.0",
                "-q",
                "videotestsrc",
                "pattern=red",
                &format!("num-buffers={duration_frames}"),
                "!",
                "video/x-raw,format=RGBA,width=2,height=2,framerate=20/1",
                "!",
                "videoconvert",
                "!",
                "avenc_ffv1",
                "!",
                "matroskamux",
                "!",
                "filesink",
                &format!("location={}", path.display()),
            ])
            .status()
            .expect("failed to run bounded GStreamer fixture generator");
        assert!(status.success(), "fixture generation failed: {status}");
    }

    fn program_config(duration: f64) -> VideoConfig {
        config(&format!(
            r##"
canvas: {{ width: 2, height: 2, frame_rate: 20, background: "#000000" }}
tracks:
  - name: program
    clips:
      - {{ id: test, start: 0, duration: {duration}, file: test.mkv }}
"##
        ))
    }

    fn wait_for_rendered_at_least(engine: &GstVideoEngine, target: Duration) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while *engine.rendered_position.lock() < target && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            *engine.rendered_position.lock() >= target,
            "program progress did not reach {target:?} before its watchdog"
        );
    }

    #[test]
    fn program_output_waits_for_synchronized_eos_before_finishing() {
        let directory = tempfile::tempdir().unwrap();
        make_test_video(directory.path(), "40");
        let mut engine =
            GstVideoEngine::program(program_config(0.6), directory.path(), Some("fakesink"))
                .unwrap();

        let started = Instant::now();
        engine.start(Duration::ZERO).unwrap();
        while engine.is_running() && started.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(5));
        }

        assert!(!engine.is_running(), "program EOS exceeded its watchdog");
        assert!(
            started.elapsed() >= Duration::from_millis(450),
            "worker tore down before the synchronized sink consumed the timeline: {:?}",
            started.elapsed()
        );
        assert!(
            *engine.rendered_position.lock() >= Duration::from_millis(550),
            "final presented position was {:?}",
            *engine.rendered_position.lock()
        );
    }

    #[test]
    fn program_pause_and_resume_use_synchronized_pipeline_progress() {
        let directory = tempfile::tempdir().unwrap();
        make_test_video(directory.path(), "80");
        let mut engine =
            GstVideoEngine::program(program_config(4.0), directory.path(), Some("fakesink"))
                .unwrap();

        engine.start(Duration::from_secs(1)).unwrap();
        wait_for_rendered_at_least(&engine, Duration::from_millis(1_125));
        let pause_started = Instant::now();
        engine.pause().unwrap();
        assert!(pause_started.elapsed() < Duration::from_secs(1));
        let first_pause = engine.position;
        assert!(
            (Duration::from_millis(1_075)..Duration::from_millis(1_700)).contains(&first_pause),
            "pause captured producer-ahead position {first_pause:?}"
        );

        engine.play().unwrap();
        wait_for_rendered_at_least(&engine, first_pause + Duration::from_millis(100));
        engine.pause().unwrap();
        let second_pause = engine.position;
        assert!(second_pause >= first_pause, "resume moved backwards");
        assert!(
            second_pause < first_pause + Duration::from_millis(650),
            "resume started from producer-ahead position: first={first_pause:?}, second={second_pause:?}"
        );
    }

    #[test]
    fn clip_intervals_are_start_inclusive_and_end_exclusive() {
        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: first, start: 1, duration: 2, file: first.ogv, source_in: 4 }
      - { id: second, start: 3, duration: 1, file: second.ogv }
"##,
        );
        let clips = config.tracks()[0].clips();

        assert!(active_clip(clips, Duration::from_millis(999)).is_none());
        assert_eq!(
            active_clip(clips, Duration::from_secs(1)).unwrap().id(),
            "first"
        );
        assert_eq!(
            active_clip(clips, Duration::from_millis(2999))
                .unwrap()
                .id(),
            "first"
        );
        assert_eq!(
            active_clip(clips, Duration::from_secs(3)).unwrap().id(),
            "second"
        );
        assert!(active_clip(clips, Duration::from_secs(4)).is_none());
    }

    #[test]
    fn source_position_includes_source_in_and_song_offset() {
        let config = config(
            r##"
canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: clip, start: 2, duration: 3, file: clip.ogv, source_in: 4.25 }
"##,
        );
        let clip = &config.tracks()[0].clips()[0];

        assert_eq!(
            source_position(clip, Duration::from_secs_f64(3.5)),
            Duration::from_secs_f64(5.75)
        );
    }

    #[test]
    fn compositor_uses_track_order_for_z_order() {
        gst::init().unwrap();
        let output = build_output_pipeline(&OutputMode::Headless, 2, 2, 2, 1, 1).unwrap();
        output.pipeline.set_state(gst::State::Playing).unwrap();
        let period = Duration::from_secs(1);
        push_frame(
            &output.sources[0],
            &solid_frame(2, 2, [0, 0, 0, 255]).unwrap(),
            Duration::ZERO,
            period,
        )
        .unwrap();
        push_frame(
            &output.sources[1],
            &solid_frame(2, 2, [255, 0, 0, 255]).unwrap(),
            Duration::ZERO,
            period,
        )
        .unwrap();
        push_frame(
            &output.sources[2],
            &solid_frame(2, 2, [0, 255, 0, 255]).unwrap(),
            Duration::ZERO,
            period,
        )
        .unwrap();

        let sample = output
            .appsink
            .as_ref()
            .unwrap()
            .try_pull_sample(gst::ClockTime::from_seconds(2))
            .expect("compositor did not produce a frame within two seconds");
        let buffer = sample.buffer().unwrap();
        let map = buffer.map_readable().unwrap();
        assert!(map
            .as_slice()
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 255, 0, 255]));
        output.pipeline.set_state(gst::State::Null).unwrap();
    }

    fn output_wait_test_request() -> WorkerRequest {
        let (frame_tx, frame_rx) = bounded(1);
        let (runtime_error_tx, _runtime_error_rx) = bounded(1);
        WorkerRequest {
            config: config(
                r##"canvas: { width: 2, height: 2, frame_rate: 2, background: "#000000" }
tracks: []
"##,
            ),
            timeline: RuntimeVideoTimeline::empty_for_test(),
            base_path: PathBuf::new(),
            _bound_media: Vec::new(),
            output: OutputMode::Preview,
            song_position: Duration::ZERO,
            generation: 1,
            cancel: Arc::new(AtomicBool::new(false)),
            latest: Arc::new(Mutex::new(None)),
            rendered_position: Arc::new(Mutex::new(Duration::ZERO)),
            frame_tx,
            frame_rx,
            latest_only: true,
            runtime_error_tx,
            test_release: None,
            test_observed: None,
        }
    }

    fn stalled_output_pipeline() -> (gst::Pipeline, gst_app::AppSink) {
        gst::init().unwrap();
        let pipeline = gst::parse::launch("appsrc ! appsink name=program sync=true")
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        let sink = pipeline
            .by_name("program")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        (pipeline, sink)
    }

    #[test]
    fn composed_sample_wait_reports_output_bus_error_immediately() {
        let request = output_wait_test_request();
        let (pipeline, sink) = stalled_output_pipeline();
        pipeline
            .post_message(
                gst::message::Error::builder(
                    gst::StreamError::Failed,
                    "controlled preview output failure",
                )
                .src(&pipeline)
                .debug("controlled output bus error")
                .build(),
            )
            .unwrap();

        let started = Instant::now();
        let error = pull_composed_sample(&request, &pipeline, &sink, Duration::from_secs(10))
            .unwrap_err()
            .to_string();

        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(error.contains("preview output"), "{error}");
        assert!(
            error.contains("controlled preview output failure"),
            "{error}"
        );
        assert!(error.contains("controlled output bus error"), "{error}");
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn composed_sample_wait_preserves_clean_output_eos() {
        let request = output_wait_test_request();
        let (pipeline, sink) = stalled_output_pipeline();
        pipeline
            .post_message(gst::message::Eos::builder().src(&pipeline).build())
            .unwrap();

        assert!(
            pull_composed_sample(&request, &pipeline, &sink, Duration::from_secs(10))
                .unwrap()
                .is_none()
        );
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn composed_sample_wait_remains_cancellable_past_timeout_when_period_is_longer() {
        let request = output_wait_test_request();
        let cancel = Arc::clone(&request.cancel);
        let (pipeline, sink) = stalled_output_pipeline();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let sample = pull_composed_sample_with_timeout(
            &request,
            &pipeline,
            &sink,
            Duration::from_millis(200),
            Duration::from_millis(50),
        )
        .unwrap();

        assert!(sample.is_none());
        assert!(started.elapsed() >= Duration::from_millis(75));
        assert!(started.elapsed() < Duration::from_millis(250));
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn rgba_allocation_size_rejects_overflow() {
        assert_eq!(rgba_buffer_len(4096, 2160).unwrap(), 35_389_440);
        assert!(matches!(
            rgba_buffer_len(u32::MAX, u32::MAX),
            Err(VideoError::InvalidConfig(_))
        ));
    }

    #[test]
    fn frame_timing_accepts_supported_edges() {
        let minimum = frame_timing(crate::config::video::MIN_FRAME_RATE).unwrap();
        assert_eq!(minimum.fps_n, 1);
        assert_eq!(minimum.fps_d, 1_000);
        assert_eq!(minimum.period, Duration::from_secs(1_000));

        let maximum = frame_timing(crate::config::video::MAX_FRAME_RATE).unwrap();
        assert_eq!(maximum.fps_n, 240_000);
        assert_eq!(maximum.fps_d, 1_000);
        assert!(maximum.period > Duration::ZERO);
    }

    #[test]
    fn frame_timing_rejects_unrepresentable_numerator_and_period() {
        for frame_rate in [0.000_4, f64::MAX] {
            assert!(
                matches!(
                    gstreamer_fraction_numerator(frame_rate),
                    Err(VideoError::InvalidConfig(_))
                ),
                "unrepresentable numerator for {frame_rate:?} was accepted"
            );
        }
        for frame_rate in [f64::MIN_POSITIVE, f64::MAX] {
            assert!(
                matches!(frame_period(frame_rate), Err(VideoError::InvalidConfig(_))),
                "unrepresentable period for {frame_rate:?} was accepted"
            );
        }
    }

    #[test]
    fn canvas_color_is_fully_transparent() {
        assert_eq!(parse_color("#123456").unwrap(), [0x12, 0x34, 0x56, 0]);
    }
}
