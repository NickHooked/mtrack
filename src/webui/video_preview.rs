// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

//! Dedicated editor video-preview WebSocket and binary frame protocol.

const FRAME_HEADER_LEN: usize = 24;
const FRAME_MAGIC: &[u8; 4] = b"MTRK";
const FRAME_VERSION: u8 = 1;
#[cfg(all(feature = "video", test))]
const MAX_UNIQUE_MEDIA_FILES: usize = crate::video::media_binding::MAX_UNIQUE_MEDIA_FILES;

#[derive(Debug, PartialEq, Eq)]
/// Borrowed fields decoded from one binary preview frame.
pub struct DecodedFrame<'a> {
    pub epoch: u64,
    pub position_ms: u64,
    pub jpeg: &'a [u8],
}

/// Builds the 24-byte `MTRK` header followed by the JPEG payload.
pub fn encode_frame_message(epoch: u64, position_ms: u64, jpeg: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(FRAME_HEADER_LEN + jpeg.len());
    message.extend_from_slice(FRAME_MAGIC);
    message.push(FRAME_VERSION);
    message.extend_from_slice(&[0; 3]);
    message.extend_from_slice(&epoch.to_be_bytes());
    message.extend_from_slice(&position_ms.to_be_bytes());
    message.extend_from_slice(jpeg);
    message
}

/// Validates and decodes one binary `MTRK` preview frame.
pub fn decode_frame_message(message: &[u8]) -> Result<DecodedFrame<'_>, &'static str> {
    if message.len() < FRAME_HEADER_LEN {
        return Err("video preview frame is shorter than its 24-byte header");
    }
    if &message[..4] != FRAME_MAGIC {
        return Err("invalid video preview frame magic");
    }
    if message[4] != FRAME_VERSION {
        return Err("unsupported video preview frame version");
    }
    if message[5..8] != [0; 3] {
        return Err("video preview frame reserved bytes must be zero");
    }
    Ok(DecodedFrame {
        epoch: u64::from_be_bytes(message[8..16].try_into().expect("fixed epoch field")),
        position_ms: u64::from_be_bytes(message[16..24].try_into().expect("fixed position field")),
        jpeg: &message[24..],
    })
}

#[cfg(feature = "video")]
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientCommand {
    Start {
        #[serde(deserialize_with = "deserialize_request_id")]
        request_id: serde_json::Value,
        config: crate::config::video::VideoConfig,
        position_ms: u64,
        autoplay: bool,
    },
    Play {
        #[serde(deserialize_with = "deserialize_request_id")]
        request_id: serde_json::Value,
    },
    Pause {
        #[serde(deserialize_with = "deserialize_request_id")]
        request_id: serde_json::Value,
    },
    Seek {
        #[serde(deserialize_with = "deserialize_request_id")]
        request_id: serde_json::Value,
        position_ms: u64,
    },
    Stop {
        #[serde(deserialize_with = "deserialize_request_id")]
        request_id: serde_json::Value,
    },
}

#[cfg(feature = "video")]
impl ClientCommand {
    fn request_id(&self) -> &serde_json::Value {
        match self {
            Self::Start { request_id, .. }
            | Self::Play { request_id }
            | Self::Pause { request_id }
            | Self::Seek { request_id, .. }
            | Self::Stop { request_id } => request_id,
        }
    }

    fn may_have_lifecycle(&self) -> bool {
        matches!(
            self,
            Self::Start { autoplay: true, .. } | Self::Play { .. } | Self::Seek { .. }
        )
    }

    fn is_excluded_during_player_playback(&self) -> bool {
        matches!(
            self,
            Self::Start { .. } | Self::Play { .. } | Self::Seek { .. }
        )
    }

    fn player_exclusion_error(&self, epoch: u64, state: &str, current_position_ms: u64) -> String {
        let (action, position_ms) = match self {
            Self::Start { position_ms, .. } => ("start", *position_ms),
            Self::Play { .. } => ("play", current_position_ms),
            Self::Seek { .. } => ("seek", current_position_ms),
            Self::Pause { .. } | Self::Stop { .. } => {
                unreachable!("only excluded commands have exclusion errors")
            }
        };
        serde_json::json!({
            "type": "error",
            "request_id": self.request_id(),
            "epoch": epoch,
            "state": state,
            "position_ms": position_ms,
            "error": format!(
                "cannot {action} editor video preview while the player is playing"
            ),
        })
        .to_string()
    }
}

#[cfg(feature = "video")]
fn deserialize_request_id<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    if value.is_null() {
        return Err(serde::de::Error::custom("request_id must not be null"));
    }
    Ok(value)
}

#[cfg(feature = "video")]
enum ActorCommand {
    Start {
        request_id: serde_json::Value,
        config: crate::config::video::VideoConfig,
        base_path: std::path::PathBuf,
        position_ms: u64,
        autoplay: bool,
    },
    Play {
        request_id: serde_json::Value,
    },
    Pause {
        request_id: serde_json::Value,
    },
    Seek {
        request_id: serde_json::Value,
        position_ms: u64,
    },
    Stop {
        request_id: serde_json::Value,
    },
    Shutdown,
    #[cfg(test)]
    Barrier(crossbeam_channel::Sender<()>),
}

#[cfg(feature = "video")]
struct PreviewActor {
    commands: crossbeam_channel::Sender<ActorCommand>,
    controls: crossbeam_channel::Receiver<String>,
    frames: crossbeam_channel::Receiver<Vec<u8>>,
    join: Option<std::thread::JoinHandle<()>>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "video")]
impl PreviewActor {
    fn shutdown(mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        // Wake the actor if every selected input is idle. The atomic flag alone
        // cannot wake a thread blocked in `crossbeam_channel::select!`.
        let _ = self.commands.try_send(ActorCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(feature = "video")]
impl Drop for PreviewActor {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(feature = "video")]
fn spawn_preview_actor(lifetime_guard: Option<Box<dyn Send>>) -> PreviewActor {
    let origin = std::time::Instant::now();
    let mut dependencies = ActorDependencies::new(
        |config, base_path, bound_media| {
            crate::video::GstVideoEngine::preview_resolved(config, base_path, bound_media)
                .map(|engine| Box::new(engine) as Box<dyn crate::video::VideoOutput>)
                .map_err(|error| error.to_string())
        },
        encode_preview_jpeg,
        move || origin.elapsed(),
        crossbeam_channel::tick(std::time::Duration::from_millis(5)),
    );
    dependencies._lifetime_guard = lifetime_guard;
    spawn_preview_actor_with(dependencies)
}

#[cfg(feature = "video")]
type PreviewEngineFactory = dyn FnMut(
        crate::config::video::VideoConfig,
        &std::path::Path,
        Vec<std::fs::File>,
    ) -> Result<Box<dyn crate::video::VideoOutput>, String>
    + Send;

#[cfg(feature = "video")]
type PreviewEncoder = dyn FnMut(&[u8], u32, u32, &str) -> Result<Vec<u8>, String> + Send;

#[cfg(feature = "video")]
struct ActorDependencies {
    engine_factory: Box<PreviewEngineFactory>,
    encoder: Box<PreviewEncoder>,
    now: Box<dyn FnMut() -> std::time::Duration + Send>,
    ticker: crossbeam_channel::Receiver<std::time::Instant>,
    /// Dropped only when the actor thread exits (normally or by panic).
    _lifetime_guard: Option<Box<dyn Send>>,
}

#[cfg(feature = "video")]
impl ActorDependencies {
    fn new(
        engine_factory: impl FnMut(
                crate::config::video::VideoConfig,
                &std::path::Path,
                Vec<std::fs::File>,
            ) -> Result<Box<dyn crate::video::VideoOutput>, String>
            + Send
            + 'static,
        encoder: impl FnMut(&[u8], u32, u32, &str) -> Result<Vec<u8>, String> + Send + 'static,
        now: impl FnMut() -> std::time::Duration + Send + 'static,
        ticker: crossbeam_channel::Receiver<std::time::Instant>,
    ) -> Self {
        Self {
            engine_factory: Box::new(engine_factory),
            encoder: Box::new(encoder),
            now: Box::new(now),
            ticker,
            _lifetime_guard: None,
        }
    }
}

#[cfg(feature = "video")]
fn spawn_preview_actor_with(dependencies: ActorDependencies) -> PreviewActor {
    let (command_tx, command_rx) = crossbeam_channel::bounded(8);
    let (control_tx, control_rx) = crossbeam_channel::bounded(32);
    let (frame_tx, frame_rx) = crossbeam_channel::bounded(1);
    let actor_frame_rx = frame_rx.clone();
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let actor_shutdown = std::sync::Arc::clone(&shutdown);
    let join = std::thread::Builder::new()
        .name("mtrack-editor-video-preview".to_string())
        .spawn(move || {
            run_preview_actor(
                command_rx,
                control_tx,
                frame_tx,
                actor_frame_rx,
                actor_shutdown,
                dependencies,
            )
        })
        .expect("failed to spawn editor video preview actor");
    PreviewActor {
        commands: command_tx,
        controls: control_rx,
        frames: frame_rx,
        join: Some(join),
        shutdown,
    }
}

#[cfg(feature = "video")]
#[derive(Default)]
struct EncodeLimiter {
    last: Option<std::time::Duration>,
}

#[cfg(feature = "video")]
impl EncodeLimiter {
    const INTERVAL: std::time::Duration = std::time::Duration::from_nanos(83_333_334);

    fn take(&mut self, now: std::time::Duration) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_sub(last) < Self::INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

#[cfg(feature = "video")]
struct PendingOneShot {
    request_id: serde_json::Value,
}

#[cfg(feature = "video")]
struct PendingFrame {
    epoch: u64,
    frame: crate::video::VideoFrame,
}

#[cfg(feature = "video")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameIdentity {
    generation: u64,
    song_position: std::time::Duration,
    pts: std::time::Duration,
}

#[cfg(feature = "video")]
impl From<&crate::video::VideoFrame> for FrameIdentity {
    fn from(frame: &crate::video::VideoFrame) -> Self {
        Self {
            generation: frame.generation,
            song_position: frame.song_position,
            pts: frame.pts,
        }
    }
}

#[cfg(feature = "video")]
impl PendingOneShot {
    fn cancel(pending: &mut Option<Self>) -> Option<serde_json::Value> {
        pending.take().map(|pending| pending.request_id)
    }
}

#[cfg(feature = "video")]
fn supersede_pending(
    controls: &crossbeam_channel::Sender<String>,
    shutdown: &std::sync::atomic::AtomicBool,
    pending: &mut Option<PendingOneShot>,
    epoch: u64,
    state: &str,
    position_ms: u64,
    command: &str,
) {
    if let Some(request_id) = PendingOneShot::cancel(pending) {
        send_error(
            controls,
            shutdown,
            Some(request_id),
            epoch,
            state,
            position_ms,
            format!("superseded by {command}"),
        );
    }
}

#[cfg(feature = "video")]
fn run_preview_actor(
    commands: crossbeam_channel::Receiver<ActorCommand>,
    controls: crossbeam_channel::Sender<String>,
    frames: crossbeam_channel::Sender<Vec<u8>>,
    frame_eviction_rx: crossbeam_channel::Receiver<Vec<u8>>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut dependencies: ActorDependencies,
) {
    use crate::video::{VideoFrame, VideoOutput};
    use std::time::Duration;

    let never_frames = crossbeam_channel::never::<VideoFrame>();

    let mut frame_source = never_frames.clone();
    let mut runtime_errors = crossbeam_channel::never();
    let mut engine: Option<Box<dyn VideoOutput>> = None;
    let mut expected_generation = None;
    let mut epoch = 0_u64;
    let mut position_ms = 0_u64;
    let mut playing = false;
    let mut background = "#000000".to_string();
    let mut pause_after_frame: Option<PendingOneShot> = None;
    let mut lifecycle_request_id: Option<serde_json::Value> = None;
    let mut encode_limiter = EncodeLimiter::default();
    let mut pending_frame: Option<PendingFrame> = None;
    let mut last_presented_frame: Option<FrameIdentity> = None;

    loop {
        if shutdown.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        crossbeam_channel::select! {
            recv(commands) -> command => {
                let Ok(command) = command else { break };
                match command {
                    ActorCommand::Start { request_id, mut config, base_path, position_ms: requested, autoplay } => {
                        supersede_pending(&controls, &shutdown, &mut pause_after_frame, epoch, if playing { "playing" } else { "paused" }, position_ms, "start");
                        epoch = epoch.saturating_add(1);
                        position_ms = requested;
                        playing = false;
                        pending_frame = None;
                        last_presented_frame = None;
                        while frame_source.try_recv().is_ok() {}
                        if let Some(old) = engine.as_mut() {
                            if let Err(error) = old.cancel() {
                                expected_generation = None;
                                send_error(&controls, &shutdown, Some(request_id), epoch, "shutting_down", position_ms, error.to_string());
                                frame_source = never_frames.clone();
                                runtime_errors = crossbeam_channel::never();
                                continue;
                            }
                            engine = None;
                        }
                        lifecycle_request_id = None;
                        if let Err(errors) = config.validate() {
                            send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, errors.join("; "));
                            frame_source = never_frames.clone();
                            continue;
                        }
                        let bound_media = match sanitize_clip_paths(&base_path, &mut config) {
                            Ok(media) => media,
                            Err(error) => {
                                send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, error);
                                frame_source = never_frames.clone();
                                continue;
                            }
                        };
                        background = config.canvas().background().to_string();
                        match (dependencies.engine_factory)(config, &bound_media.base_path, bound_media.files) {
                            Ok(mut new_engine) => {
                                frame_source = new_engine.frame_receiver();
                                runtime_errors = new_engine.runtime_error_receiver();
                                match new_engine.start(Duration::from_millis(requested)) {
                                    Ok(()) => {
                                        expected_generation = Some(new_engine.generation());
                                        playing = true;
                                        if autoplay {
                                            lifecycle_request_id = Some(request_id.clone());
                                            send_state(&controls, &shutdown, Some(request_id), epoch, "playing", position_ms);
                                        } else {
                                            pause_after_frame = Some(PendingOneShot { request_id });
                                        }
                                        engine = Some(new_engine);
                                    }
                                    Err(error) => {
                                        expected_generation = None;
                                        let state = if new_engine.is_running() { "shutting_down" } else { "stopped" };
                                        send_error(&controls, &shutdown, Some(request_id), epoch, state, position_ms, error.to_string());
                                        frame_source = never_frames.clone();
                                        runtime_errors = crossbeam_channel::never();
                                        if new_engine.is_running() {
                                            engine = Some(new_engine);
                                        }
                                    }
                                }
                            }
                            Err(error) => send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, error.to_string()),
                        }
                    }
                    ActorCommand::Play { request_id } => {
                        if let Some(engine) = engine.as_mut() {
                            match engine.play() {
                                Ok(()) => {
                                    expected_generation = Some(engine.generation());
                                    playing = true;
                                    if let Some(pending_id) = PendingOneShot::cancel(&mut pause_after_frame) {
                                        send_state(&controls, &shutdown, Some(pending_id), epoch, "playing", position_ms);
                                    }
                                    lifecycle_request_id = Some(request_id.clone());
                                    send_state(&controls, &shutdown, Some(request_id), epoch, "playing", position_ms);
                                }
                                Err(error) => send_error(&controls, &shutdown, Some(request_id), epoch, "paused", position_ms, error.to_string()),
                            }
                        } else {
                            send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, "preview has not been started".to_string());
                        }
                    }
                    ActorCommand::Pause { request_id } => {
                        if let Some(pending_id) = PendingOneShot::cancel(&mut pause_after_frame) {
                            send_error(&controls, &shutdown, Some(pending_id), epoch, "paused", position_ms, "pending preview frame was cancelled by pause".to_string());
                        }
                        if let Some(engine) = engine.as_mut() {
                            match engine.pause() {
                                Ok(()) => {
                                    playing = false;
                                    lifecycle_request_id = None;
                                    send_state(&controls, &shutdown, Some(request_id), epoch, "paused", position_ms);
                                }
                                Err(error) => send_error(&controls, &shutdown, Some(request_id), epoch, if playing { "playing" } else { "paused" }, position_ms, error.to_string()),
                            }
                        } else {
                            send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, "preview has not been started".to_string());
                        }
                    }
                    ActorCommand::Seek { request_id, position_ms: requested } => {
                        supersede_pending(&controls, &shutdown, &mut pause_after_frame, epoch, if playing { "playing" } else { "paused" }, position_ms, "seek");
                        epoch = epoch.saturating_add(1);
                        position_ms = requested;
                        pending_frame = None;
                        last_presented_frame = None;
                        while frame_source.try_recv().is_ok() {}
                        if let Some(engine) = engine.as_mut() {
                            let result = if playing {
                                engine.seek(Duration::from_millis(requested))
                            } else {
                                engine.start(Duration::from_millis(requested))
                            };
                            match result {
                                Ok(()) if playing => {
                                    expected_generation = Some(engine.generation());
                                    lifecycle_request_id = Some(request_id.clone());
                                    send_state(&controls, &shutdown, Some(request_id), epoch, "playing", position_ms);
                                }
                                Ok(()) => {
                                    expected_generation = Some(engine.generation());
                                    playing = true;
                                    pause_after_frame = Some(PendingOneShot { request_id });
                                }
                                Err(error) => {
                                    expected_generation = None;
                                    pending_frame = None;
                                    send_error(&controls, &shutdown, Some(request_id), epoch, "shutting_down", position_ms, error.to_string());
                                }
                            }
                        } else {
                            send_error(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms, "preview has not been started".to_string());
                        }
                    }
                    ActorCommand::Stop { request_id } => {
                        supersede_pending(&controls, &shutdown, &mut pause_after_frame, epoch, if playing { "playing" } else { "paused" }, position_ms, "stop");
                        epoch = epoch.saturating_add(1);
                        pending_frame = None;
                        last_presented_frame = None;
                        expected_generation = None;
                        lifecycle_request_id = None;
                        while frame_source.try_recv().is_ok() {}
                        if let Some(active) = engine.as_mut() {
                            if let Err(error) = active.stop() {
                                playing = false;
                                pending_frame = None;
                                frame_source = never_frames.clone();
                                runtime_errors = crossbeam_channel::never();
                                send_error(&controls, &shutdown, Some(request_id), epoch, "shutting_down", position_ms, error.to_string());
                                continue;
                            }
                            engine = None;
                        }
                        playing = false;
                        position_ms = 0;
                        pause_after_frame = None;
                        frame_source = never_frames.clone();
                        runtime_errors = crossbeam_channel::never();
                        send_state(&controls, &shutdown, Some(request_id), epoch, "stopped", position_ms);
                    }
                    ActorCommand::Shutdown => break,
                    #[cfg(test)]
                    ActorCommand::Barrier(done) => {
                        let _ = done.send(());
                    }

                }
            }
            recv(frame_source) -> frame => {
                let Ok(frame) = frame else {
                    frame_source = never_frames.clone();
                    continue;
                };
                if Some(frame.generation) != expected_generation {
                    continue;
                }
                let requested_frame = pause_after_frame.is_none()
                    || frame.song_position == Duration::from_millis(position_ms);
                if requested_frame {
                    pending_frame = Some(PendingFrame { epoch, frame });
                }
            }
            recv(runtime_errors) -> error => {
                if let Ok(error) = error {
                    if let Some(pending) = pause_after_frame.take() {
                        playing = false;
                        send_error(&controls, &shutdown, Some(pending.request_id), epoch, "paused", position_ms, error.to_string());
                    } else {
                        send_error(
                            &controls,
                            &shutdown,
                            lifecycle_request_id.clone(),
                            epoch,
                            "shutting_down",
                            position_ms,
                            error.to_string(),
                        );
                        shutdown.store(true, std::sync::atomic::Ordering::Release);
                    }
                } else {
                    runtime_errors = crossbeam_channel::never();
                }
            }
            recv(dependencies.ticker) -> _ => {
                let now = (dependencies.now)();
                let natural_eos = playing
                    && lifecycle_request_id.is_some()
                    && engine.as_ref().is_some_and(|active| {
                        Some(active.generation()) == expected_generation && !active.is_running()
                    });
                let mut eos_latest = None;
                if natural_eos {
                    // The worker can publish `running = false` immediately after
                    // producing its final frame, before this receiver is selected.
                    // Drain the stream and consult `latest_frame` while the
                    // generation is still admitted, then let the normal limiter
                    // hand that frame off before emitting terminal state.
                    while let Ok(frame) = frame_source.try_recv() {
                        if Some(frame.generation) != expected_generation {
                            continue;
                        }
                        let requested_frame = pause_after_frame.is_none()
                            || frame.song_position == Duration::from_millis(position_ms);
                        if requested_frame {
                            pending_frame = Some(PendingFrame { epoch, frame });
                        }
                    }
                    eos_latest = engine
                        .as_ref()
                        .and_then(|active| active.latest_frame())
                        .filter(|frame| Some(frame.generation) == expected_generation);
                    if let Some(frame) = eos_latest.as_ref() {
                        if last_presented_frame != Some(FrameIdentity::from(frame)) {
                            pending_frame = Some(PendingFrame {
                                epoch,
                                frame: frame.clone(),
                            });
                        }
                    }
                }
                if pending_frame.is_some() && encode_limiter.take(now) {
                    if let Some(PendingFrame { epoch: frame_epoch, frame }) = pending_frame.take() {
                        if frame_epoch != epoch {
                            continue;
                        }
                        position_ms = frame.song_position.as_millis().min(u128::from(u64::MAX)) as u64;
                        match (dependencies.encoder)(&frame.rgba, frame.width, frame.height, &background) {
                            Ok(jpeg) => {
                                send_latest_frame(&frames, &frame_eviction_rx, encode_frame_message(epoch, position_ms, &jpeg));
                                last_presented_frame = Some(FrameIdentity::from(&frame));
                                if let Some(pending) = pause_after_frame.take() {
                                    if let Some(engine) = engine.as_mut() {
                                        match engine.pause() {
                                            Ok(()) => {
                                                playing = false;
                                                send_state(&controls, &shutdown, Some(pending.request_id), epoch, "paused", position_ms);
                                            }
                                            Err(error) => send_error(&controls, &shutdown, Some(pending.request_id), epoch, "playing", position_ms, error.to_string()),
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                if let Some(pending) = pause_after_frame.take() {
                                    if let Some(engine) = engine.as_mut() {
                                        if engine.pause().is_ok() {
                                            playing = false;
                                        }
                                    }
                                    send_error(&controls, &shutdown, Some(pending.request_id), epoch, if playing { "playing" } else { "paused" }, position_ms, error);
                                } else {
                                    send_error(
                                        &controls,
                                        &shutdown,
                                        lifecycle_request_id.clone(),
                                        epoch,
                                        "shutting_down",
                                        position_ms,
                                        error,
                                    );
                                    shutdown.store(true, std::sync::atomic::Ordering::Release);
                                }
                            }
                        }
                    }
                }
                let eos_frame_presented = eos_latest.as_ref().is_none_or(|frame| {
                    last_presented_frame == Some(FrameIdentity::from(frame))
                });
                if natural_eos
                    && pending_frame.is_none()
                    && eos_frame_presented
                    && !shutdown.load(std::sync::atomic::Ordering::Acquire)
                {
                    playing = false;
                    expected_generation = None;
                    send_state(
                        &controls,
                        &shutdown,
                        lifecycle_request_id.take(),
                        epoch,
                        "stopped",
                        position_ms,
                    );
                }
            }
        }
    }
    if let Some(mut engine) = engine {
        while engine.cancel().is_err() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(feature = "video")]
fn send_latest_frame(
    sender: &crossbeam_channel::Sender<Vec<u8>>,
    receiver: &crossbeam_channel::Receiver<Vec<u8>>,
    mut frame: Vec<u8>,
) {
    loop {
        match sender.try_send(frame) {
            Ok(()) | Err(crossbeam_channel::TrySendError::Disconnected(_)) => return,
            Err(crossbeam_channel::TrySendError::Full(returned)) => {
                frame = returned;
                let _ = receiver.try_recv();
            }
        }
    }
}

#[cfg(feature = "video")]
fn send_state(
    sender: &crossbeam_channel::Sender<String>,
    shutdown: &std::sync::atomic::AtomicBool,
    request_id: Option<serde_json::Value>,
    epoch: u64,
    state: &str,
    position_ms: u64,
) {
    send_control(
        sender,
        shutdown,
        serde_json::json!({
            "type": "state",
            "request_id": request_id,
            "epoch": epoch,
            "state": state,
            "position_ms": position_ms,
        })
        .to_string(),
    );
}

#[cfg(feature = "video")]
fn send_error(
    sender: &crossbeam_channel::Sender<String>,
    shutdown: &std::sync::atomic::AtomicBool,
    request_id: Option<serde_json::Value>,
    epoch: u64,
    state: &str,
    position_ms: u64,
    error: String,
) {
    send_control(
        sender,
        shutdown,
        serde_json::json!({
            "type": "error",
            "request_id": request_id,
            "epoch": epoch,
            "state": state,
            "position_ms": position_ms,
            "error": error,
        })
        .to_string(),
    );
}

#[cfg(feature = "video")]
fn send_control(
    sender: &crossbeam_channel::Sender<String>,
    shutdown: &std::sync::atomic::AtomicBool,
    message: String,
) {
    if sender.try_send(message).is_err() {
        shutdown.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(feature = "video")]
#[derive(Debug)]
struct BoundPreviewMedia {
    base_path: std::path::PathBuf,
    files: Vec<std::fs::File>,
}

#[cfg(feature = "video")]
fn sanitize_clip_paths(
    base_path: &std::path::Path,
    config: &mut crate::config::video::VideoConfig,
) -> Result<BoundPreviewMedia, String> {
    let timeline =
        crate::video::timeline::RuntimeVideoTimeline::resolve("editor preview", base_path, config)
            .map_err(|error| error.to_string())?;
    let canonical_base = timeline.root().to_path_buf();
    let bound = crate::video::media_binding::bind_timeline(config, &timeline, "video preview")?;
    *config = bound.config;
    Ok(BoundPreviewMedia {
        base_path: canonical_base,
        files: bound.files,
    })
}

#[cfg(feature = "video")]
fn encode_preview_jpeg(
    rgba: &[u8],
    width: u32,
    height: u32,
    background: &str,
) -> Result<Vec<u8>, String> {
    use image::{codecs::jpeg::JpegEncoder, DynamicImage, RgbImage};

    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "preview frame dimensions overflow".to_string())?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(format!(
            "invalid preview RGBA buffer: {} bytes for {width}x{height}",
            rgba.len()
        ));
    }
    let background = parse_hex_rgb(background)?;
    let rgb_capacity = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| "preview RGB frame dimensions overflow".to_string())?;
    let mut rgb = Vec::with_capacity(rgb_capacity);
    for pixel in rgba.chunks_exact(4) {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            let value = (u16::from(pixel[channel]) * alpha
                + u16::from(background[channel]) * (255 - alpha)
                + 127)
                / 255;
            rgb.push(value as u8);
        }
    }
    let image = RgbImage::from_raw(width, height, rgb)
        .ok_or_else(|| "failed to construct preview RGB image".to_string())?;
    let scale = (960.0_f64 / f64::from(width))
        .min(540.0_f64 / f64::from(height))
        .min(1.0);
    let target_width = (f64::from(width) * scale).round().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).round().max(1.0) as u32;
    let image = if (target_width, target_height) == (width, height) {
        image
    } else {
        image::imageops::resize(
            &image,
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 78)
        .encode_image(&DynamicImage::ImageRgb8(image))
        .map_err(|error| format!("failed to encode preview JPEG: {error}"))?;
    Ok(jpeg)
}

#[cfg(feature = "video")]
fn parse_hex_rgb(color: &str) -> Result<[u8; 3], String> {
    if color.len() != 7 || !color.starts_with('#') {
        return Err(format!("invalid canvas background {color:?}"));
    }
    let value = u32::from_str_radix(&color[1..], 16)
        .map_err(|_| format!("invalid canvas background {color:?}"))?;
    Ok([
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ])
}

#[cfg(not(feature = "video"))]
pub(crate) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    axum::extract::Path(_name): axum::extract::Path<String>,
    axum::extract::State(_state): axum::extract::State<super::server::WebUiState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        let response = serde_json::json!({
            "type": "error",
            "request_id": "session",
            "epoch": 0,
            "state": "disabled",
            "position_ms": 0,
            "error": "video preview is disabled in this build",
        });
        let _ = socket
            .send(axum::extract::ws::Message::Text(
                response.to_string().into(),
            ))
            .await;
        let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
    })
}

#[cfg(feature = "video")]
pub(crate) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<super::server::WebUiState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, name, state))
}

#[cfg(feature = "video")]
#[derive(Default)]
struct ResponseReservations {
    by_id: std::collections::HashMap<String, ResponseReservation>,
    active_lifecycle: Option<String>,
}

#[cfg(feature = "video")]
struct ResponseReservation {
    initial: Option<tokio::sync::OwnedSemaphorePermit>,
    terminal: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[cfg(feature = "video")]
fn request_id_key(value: &serde_json::Value) -> String {
    fn normalized(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(normalized).collect())
            }
            serde_json::Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                let mut result = serde_json::Map::new();
                for key in keys {
                    result.insert(key.clone(), normalized(&values[key]));
                }
                serde_json::Value::Object(result)
            }
            value => value.clone(),
        }
    }

    normalized(value).to_string()
}

#[cfg(feature = "video")]
impl ResponseReservations {
    fn contains(&self, request_id: &serde_json::Value) -> bool {
        self.by_id.contains_key(&request_id_key(request_id))
    }

    fn insert(
        &mut self,
        request_id: &serde_json::Value,
        mut reservation: tokio::sync::OwnedSemaphorePermit,
        may_have_lifecycle: bool,
    ) {
        let terminal = may_have_lifecycle.then(|| {
            reservation
                .split(1)
                .expect("two response permits were acquired")
        });
        let replaced = self.by_id.insert(
            request_id_key(request_id),
            ResponseReservation {
                initial: Some(reservation),
                terminal,
            },
        );
        debug_assert!(
            replaced.is_none(),
            "duplicate request ID reached actor admission"
        );
    }

    fn retire_active_lifecycle(&mut self, replacement: Option<&str>) {
        let Some(active) = self.active_lifecycle.take() else {
            return;
        };
        if replacement != Some(active.as_str()) {
            self.by_id.remove(&active);
        }
    }

    fn take_for_control(&mut self, control: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let value = serde_json::from_str::<serde_json::Value>(control).ok()?;
        let key = request_id_key(value.get("request_id")?);
        let state = value.get("state").and_then(serde_json::Value::as_str);
        let initial = self.by_id.get_mut(&key)?.initial.take();

        if let Some(initial) = initial {
            if value.get("type").and_then(serde_json::Value::as_str) == Some("state") {
                self.retire_active_lifecycle(Some(&key));
                if state == Some("playing")
                    && self
                        .by_id
                        .get(&key)
                        .is_some_and(|entry| entry.terminal.is_some())
                {
                    self.active_lifecycle = Some(key);
                } else {
                    self.by_id.remove(&key);
                }
            } else {
                self.by_id.remove(&key);
            }
            return Some(initial);
        }

        if self.active_lifecycle.as_deref() != Some(&key) {
            return None;
        }
        let terminal = self.by_id.get_mut(&key)?.terminal.take()?;
        self.active_lifecycle = None;
        self.by_id.remove(&key);
        Some(terminal)
    }
}

#[cfg(feature = "video")]
fn take_response_reservation(
    control: &str,
    reservations: &mut ResponseReservations,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    reservations.take_for_control(control)
}

#[cfg(feature = "video")]
async fn handle_socket(
    mut socket: axum::extract::ws::WebSocket,
    name: String,
    state: super::server::WebUiState,
) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};

    let permit = match state.video_preview_permit.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            let message = protocol_error(
                serde_json::json!("session"),
                "busy",
                "another video preview session is already active",
            );
            let _ = socket.send(Message::Text(message.into())).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let song = match state.player.get_playlist().get_song(&name) {
        Some(song) => song,
        None => {
            let message = protocol_error(
                serde_json::json!("session"),
                "stopped",
                &format!("song {name:?} is not loaded"),
            );
            let _ = socket.send(Message::Text(message.into())).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let base_path = song.base_path().to_path_buf();
    // The actor thread, not this cancellable socket future, owns the global
    // permit for its complete native-worker teardown lifetime.
    let actor = spawn_preview_actor(Some(Box::new(permit)));
    let (mut socket_writer, mut socket_reader) = socket.split();
    struct ReservedControl {
        message: Message,
        _reservation: tokio::sync::OwnedSemaphorePermit,
    }
    let response_capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
    let mut reservations = ResponseReservations::default();
    let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<ReservedControl>(32);
    let (frame_tx, mut frame_rx) = tokio::sync::watch::channel(None::<Vec<u8>>);
    let mut writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                control = control_rx.recv() => {
                    let Some(control) = control else { break };
                    if socket_writer.send(control.message).await.is_err() { break; }
                }
                changed = frame_rx.changed() => {
                    if changed.is_err() { break; }
                    let frame = frame_rx.borrow_and_update().clone();
                    if let Some(frame) = frame {
                        if socket_writer.send(Message::Binary(frame.into())).await.is_err() { break; }
                    }
                }
            }
        }
    });

    enum DeferredAdmission {
        Command {
            command: ClientCommand,
            duplicate: bool,
        },
        Outbound(Message),
    }

    impl DeferredAdmission {
        fn permits(&self) -> u32 {
            match self {
                Self::Command {
                    command,
                    duplicate: false,
                } if command.may_have_lifecycle() => 2,
                _ => 1,
            }
        }
    }

    let mut deferred = None;
    let mut drain_writer = false;
    let mut preview_epoch = 0_u64;
    let mut preview_state = "stopped".to_string();
    let mut preview_position_ms = 0_u64;
    'session: loop {
        let permits = deferred.as_ref().map_or(1, DeferredAdmission::permits);
        tokio::select! {
            incoming = socket_reader.next(), if deferred.is_none() => {
                let Some(incoming) = incoming else { break };
                let Ok(message) = incoming else { break };
                deferred = match message {
                    Message::Text(text) => {
                        let fallback_request_id = request_id_from_text(&text);
                        match serde_json::from_str::<ClientCommand>(&text) {
                            Ok(command) => {
                                let duplicate = reservations.contains(command.request_id());
                                Some(DeferredAdmission::Command { command, duplicate })
                            }
                            Err(error) => Some(DeferredAdmission::Outbound(Message::Text(
                                protocol_error(
                                    fallback_request_id,
                                    "stopped",
                                    &format!("invalid video preview command: {error}"),
                                )
                                .into(),
                            ))),
                        }
                    }
                    Message::Binary(_) => Some(DeferredAdmission::Outbound(Message::Text(
                        protocol_error(
                            serde_json::json!("protocol"),
                            "stopped",
                            "video preview accepts JSON text commands only",
                        )
                        .into(),
                    ))),
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        Some(DeferredAdmission::Outbound(Message::Pong(payload)))
                    }
                    Message::Pong(_) => None,
                };
            }
            reservation = response_capacity.clone().acquire_many_owned(permits), if deferred.is_some() => {
                let reservation = match reservation {
                    Ok(reservation) => reservation,
                    Err(_) => break 'session,
                };
                match deferred.take().expect("guarded deferred admission") {
                    DeferredAdmission::Outbound(message) => {
                        if control_tx.try_send(ReservedControl { message, _reservation: reservation }).is_err() {
                            break;
                        }
                    }
                    DeferredAdmission::Command { command, duplicate } => {
                        let request_id = command.request_id().clone();
                        if duplicate {
                            let response = protocol_error(
                                request_id,
                                "busy",
                                "duplicate video preview request_id is still active",
                            );
                            if control_tx.try_send(ReservedControl { message: Message::Text(response.into()), _reservation: reservation }).is_err() {
                                break;
                            }
                            continue;
                        }
                        // Check as late as possible before actor submission. The
                        // Player API is asynchronous, so this has the same small
                        // post-check race as other Player admission paths.
                        if command.is_excluded_during_player_playback()
                            && state.player.is_playing().await
                        {
                            let response = command.player_exclusion_error(
                                preview_epoch,
                                &preview_state,
                                preview_position_ms,
                            );
                            if control_tx.try_send(ReservedControl {
                                message: Message::Text(response.into()),
                                _reservation: reservation,
                            }).is_err() {
                                break;
                            }
                            continue;
                        }
                        let may_have_lifecycle = command.may_have_lifecycle();
                        let actor_command = match command {
                            ClientCommand::Start { request_id, config, position_ms, autoplay } => {
                                ActorCommand::Start { request_id, config, base_path: base_path.clone(), position_ms, autoplay }
                            }
                            ClientCommand::Play { request_id } => ActorCommand::Play { request_id },
                            ClientCommand::Pause { request_id } => ActorCommand::Pause { request_id },
                            ClientCommand::Seek { request_id, position_ms } => ActorCommand::Seek { request_id, position_ms },
                            ClientCommand::Stop { request_id } => ActorCommand::Stop { request_id },
                        };
                        if actor.commands.try_send(actor_command).is_err() {
                            let response = protocol_error(request_id, "busy", "video preview command queue is full");
                            if control_tx.try_send(ReservedControl { message: Message::Text(response.into()), _reservation: reservation }).is_err() {
                                break;
                            }
                        } else {
                            reservations.insert(&request_id, reservation, may_have_lifecycle);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {
                while let Ok(control) = actor.controls.try_recv() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&control) {
                        if let Some(value) = value.get("epoch").and_then(serde_json::Value::as_u64) {
                            preview_epoch = value;
                        }
                        if let Some(value) = value.get("state").and_then(serde_json::Value::as_str) {
                            preview_state = value.to_string();
                        }
                        if let Some(value) = value.get("position_ms").and_then(serde_json::Value::as_u64) {
                            preview_position_ms = value;
                        }
                    }
                    let Some(reservation) = take_response_reservation(
                        &control,
                        &mut reservations,
                    ) else {
                        break 'session;
                    };
                    if control_tx.try_send(ReservedControl { message: Message::Text(control.into()), _reservation: reservation }).is_err() {
                        break 'session;
                    }
                }
                if let Ok(frame) = actor.frames.try_recv() {
                    frame_tx.send_replace(Some(frame));
                }
                if actor.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    drain_writer = true;
                    break;
                }
            }
            _ = &mut writer => break,
        }
    }

    drop(control_tx);
    drop(frame_tx);
    if drain_writer {
        if tokio::time::timeout(std::time::Duration::from_secs(1), &mut writer)
            .await
            .is_err()
        {
            writer.abort();
        }
    } else {
        writer.abort();
    }
    finish_actor(actor).await;
}

#[cfg(feature = "video")]
async fn finish_actor(actor: PreviewActor) {
    let shutdown = tokio::task::spawn_blocking(move || {
        actor.shutdown();
    });
    let _ = shutdown.await;
}

#[cfg(feature = "video")]
fn request_id_from_text(text: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("request_id").cloned())
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| serde_json::json!("protocol"))
}

#[cfg(feature = "video")]
fn protocol_error(request_id: serde_json::Value, state: &str, error: &str) -> String {
    serde_json::json!({
        "type": "error",
        "request_id": request_id,
        "epoch": 0,
        "state": state,
        "position_ms": 0,
        "error": error,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "video")]
    struct FakePreviewEngine {
        frames: crossbeam_channel::Receiver<crate::video::VideoFrame>,
        runtime_errors: crossbeam_channel::Receiver<crate::video::VideoError>,
        running: bool,
        generation: u64,
    }

    #[cfg(feature = "video")]
    impl FakePreviewEngine {
        fn idle() -> Self {
            Self {
                frames: crossbeam_channel::never(),
                runtime_errors: crossbeam_channel::never(),
                running: false,
                generation: 0,
            }
        }
    }

    #[cfg(feature = "video")]
    impl crate::video::VideoOutput for FakePreviewEngine {
        fn start(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            self.running = true;
            self.generation += 1;
            Ok(())
        }

        fn play(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = true;
            Ok(())
        }

        fn pause(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn seek(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            self.generation += 1;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn cancel(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn latest_frame(&self) -> Option<crate::video::VideoFrame> {
            None
        }

        fn frame_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoFrame> {
            self.frames.clone()
        }

        fn runtime_error_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoError> {
            self.runtime_errors.clone()
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn generation(&self) -> u64 {
            self.generation
        }
    }

    #[cfg(feature = "video")]
    struct StalledTeardownEngine {
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
        frames: crossbeam_channel::Receiver<crate::video::VideoFrame>,
        runtime_errors: crossbeam_channel::Receiver<crate::video::VideoError>,
        running: bool,
        generation: u64,
    }

    #[cfg(feature = "video")]
    impl StalledTeardownEngine {
        fn new(released: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
            Self {
                released,
                frames: crossbeam_channel::never(),
                runtime_errors: crossbeam_channel::never(),
                running: false,
                generation: 0,
            }
        }

        fn teardown(&mut self, operation: &'static str) -> Result<(), crate::video::VideoError> {
            if !self.released.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::video::VideoError::ShutdownTimeout {
                    operation,
                    timeout: std::time::Duration::from_millis(1),
                });
            }
            self.running = false;
            Ok(())
        }
    }

    #[cfg(feature = "video")]
    impl crate::video::VideoOutput for StalledTeardownEngine {
        fn start(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            self.running = true;
            self.generation += 1;
            Ok(())
        }

        fn play(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = true;
            Ok(())
        }

        fn pause(&mut self) -> Result<(), crate::video::VideoError> {
            self.teardown("pause")
        }

        fn seek(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::video::VideoError> {
            self.teardown("stop")
        }

        fn cancel(&mut self) -> Result<(), crate::video::VideoError> {
            self.teardown("cancel")
        }

        fn latest_frame(&self) -> Option<crate::video::VideoFrame> {
            None
        }

        fn frame_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoFrame> {
            self.frames.clone()
        }

        fn runtime_error_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoError> {
            self.runtime_errors.clone()
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn generation(&self) -> u64 {
            self.generation
        }
    }

    #[cfg(feature = "video")]
    struct RetrySeekEngine {
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
        frames: crossbeam_channel::Receiver<crate::video::VideoFrame>,
        running: bool,
        generation: u64,
    }

    #[cfg(feature = "video")]
    impl crate::video::VideoOutput for RetrySeekEngine {
        fn start(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            self.running = true;
            self.generation += 1;
            Ok(())
        }

        fn play(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = true;
            Ok(())
        }

        fn pause(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn seek(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            if !self.released.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::video::VideoError::ShutdownTimeout {
                    operation: "seek",
                    timeout: std::time::Duration::from_millis(1),
                });
            }
            self.generation += 1;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn cancel(&mut self) -> Result<(), crate::video::VideoError> {
            self.running = false;
            Ok(())
        }

        fn latest_frame(&self) -> Option<crate::video::VideoFrame> {
            None
        }

        fn frame_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoFrame> {
            self.frames.clone()
        }

        fn runtime_error_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoError> {
            crossbeam_channel::never()
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn generation(&self) -> u64 {
            self.generation
        }
    }

    #[cfg(feature = "video")]
    fn valid_empty_config() -> crate::config::video::VideoConfig {
        config::Config::builder()
            .add_source(config::File::from_str(
                r##"canvas: { width: 4, height: 4, frame_rate: 4, background: "#000000" }
tracks: []"##,
                config::FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap()
    }

    #[cfg(feature = "video")]
    fn fake_actor() -> PreviewActor {
        spawn_preview_actor_with(ActorDependencies::new(
            |_config, _base_path, _bound_media| Ok(Box::new(FakePreviewEngine::idle())),
            encode_preview_jpeg,
            || std::time::Duration::ZERO,
            crossbeam_channel::never(),
        ))
    }

    #[cfg(feature = "video")]
    fn stalled_actor(
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
        creations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> PreviewActor {
        stalled_actor_with_optional_guard(released, creations, None)
    }

    #[cfg(feature = "video")]
    fn stalled_actor_with_guard(
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
        creations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        guard: Box<dyn Send>,
    ) -> PreviewActor {
        stalled_actor_with_optional_guard(released, creations, Some(guard))
    }

    #[cfg(feature = "video")]
    fn stalled_actor_with_optional_guard(
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
        creations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        guard: Option<Box<dyn Send>>,
    ) -> PreviewActor {
        let mut dependencies = ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                creations.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                Ok(
                    Box::new(StalledTeardownEngine::new(std::sync::Arc::clone(&released)))
                        as Box<dyn crate::video::VideoOutput>,
                )
            },
            |_rgba, _width, _height, _background| Ok(vec![0xff, 0xd8, 0xff, 0xd9]),
            || std::time::Duration::ZERO,
            crossbeam_channel::never(),
        );
        dependencies._lifetime_guard = guard;
        spawn_preview_actor_with(dependencies)
    }

    #[cfg(feature = "video")]
    fn send_start(actor: &PreviewActor, directory: &std::path::Path, request_id: &str) {
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!(request_id),
                config: valid_empty_config(),
                base_path: directory.to_path_buf(),
                position_ms: 23,
                autoplay: true,
            })
            .unwrap();
    }

    #[cfg(feature = "video")]
    fn recv_control(actor: &PreviewActor) -> serde_json::Value {
        serde_json::from_str(
            &actor
                .controls
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
        )
        .unwrap()
    }

    #[cfg(feature = "video")]
    struct NaturalEosEngine {
        frame_sender: crossbeam_channel::Sender<crate::video::VideoFrame>,
        frames: crossbeam_channel::Receiver<crate::video::VideoFrame>,
        running: std::sync::Arc<std::sync::atomic::AtomicBool>,
        latest: crate::video::VideoFrame,
        generation: u64,
    }

    #[cfg(feature = "video")]
    impl crate::video::VideoOutput for NaturalEosEngine {
        fn start(
            &mut self,
            _song_position: std::time::Duration,
        ) -> Result<(), crate::video::VideoError> {
            self.generation += 1;
            self.running
                .store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }
        fn play(&mut self) -> Result<(), crate::video::VideoError> {
            self.generation += 1;
            self.running
                .store(true, std::sync::atomic::Ordering::Release);
            let mut frame = self.latest.clone();
            frame.generation = self.generation;
            self.latest = frame.clone();
            self.frame_sender.send(frame).unwrap();
            self.running
                .store(false, std::sync::atomic::Ordering::Release);
            Ok(())
        }
        fn pause(&mut self) -> Result<(), crate::video::VideoError> {
            Ok(())
        }
        fn seek(&mut self, _position: std::time::Duration) -> Result<(), crate::video::VideoError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), crate::video::VideoError> {
            Ok(())
        }
        fn cancel(&mut self) -> Result<(), crate::video::VideoError> {
            Ok(())
        }
        fn latest_frame(&self) -> Option<crate::video::VideoFrame> {
            Some(self.latest.clone())
        }
        fn frame_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoFrame> {
            self.frames.clone()
        }
        fn runtime_error_receiver(&self) -> crossbeam_channel::Receiver<crate::video::VideoError> {
            crossbeam_channel::never()
        }
        fn is_running(&self) -> bool {
            self.running.load(std::sync::atomic::Ordering::Acquire)
        }
        fn generation(&self) -> u64 {
            self.generation
        }
    }

    #[cfg(feature = "video")]
    #[test]
    fn autoplay_natural_eos_emits_correlated_terminal_state() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let directory = tempfile::tempdir().unwrap();
        let running = Arc::new(AtomicBool::new(false));
        let engine_running = Arc::clone(&running);
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let delayed_frame_tx = frame_tx.clone();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _media| {
                Ok(Box::new(NaturalEosEngine {
                    frame_sender: frame_tx.clone(),
                    frames: frame_rx.clone(),
                    running: Arc::clone(&engine_running),
                    latest: crate::video::VideoFrame {
                        generation: 1,
                        song_position: std::time::Duration::from_millis(750),
                        pts: std::time::Duration::from_millis(750),
                        width: 1,
                        height: 1,
                        rgba: Arc::from([0_u8, 0, 0, 255]),
                    },
                    generation: 0,
                }))
            },
            |_rgba, _width, _height, _background| Ok(Vec::new()),
            || std::time::Duration::ZERO,
            tick_rx,
        ));
        send_start(&actor, directory.path(), "eos-start");
        let playing = recv_control(&actor);
        assert_eq!(playing["state"], "playing");
        running.store(false, Ordering::Release);
        tick_tx.send(std::time::Instant::now()).unwrap();
        let frame = actor
            .frames
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("final EOS frame must be handed off before terminal state");
        let frame = decode_frame_message(&frame).unwrap();
        assert_eq!((frame.epoch, frame.position_ms), (1, 750));
        let terminal = recv_control(&actor);
        assert_eq!(terminal["request_id"], "eos-start");
        assert_eq!(terminal["state"], "stopped");
        assert_eq!(terminal["position_ms"], 750);
        delayed_frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(750),
                pts: std::time::Duration::from_millis(750),
                width: 1,
                height: 1,
                rgba: Arc::from([0_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        assert!(actor
            .controls
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        assert!(
            actor
                .frames
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "receiver delivery duplicated the EOS frame already sourced from latest_frame"
        );
        actor
            .commands
            .send(ActorCommand::Stop {
                request_id: serde_json::json!("after-eos"),
            })
            .unwrap();
        let stopped = recv_control(&actor);
        assert_eq!(stopped["request_id"], "after-eos");
        assert_eq!(stopped["state"], "stopped");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn play_after_natural_eos_receives_the_new_generation_frame() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        let directory = tempfile::tempdir().unwrap();
        let running = Arc::new(AtomicBool::new(false));
        let engine_running = Arc::clone(&running);
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _media| {
                Ok(Box::new(NaturalEosEngine {
                    frame_sender: frame_tx.clone(),
                    frames: frame_rx.clone(),
                    running: Arc::clone(&engine_running),
                    latest: crate::video::VideoFrame {
                        generation: 1,
                        song_position: std::time::Duration::from_millis(750),
                        pts: std::time::Duration::ZERO,
                        width: 1,
                        height: 1,
                        rgba: Arc::from([0_u8, 0, 0, 255]),
                    },
                    generation: 0,
                }))
            },
            |_rgba, _width, _height, _background| Ok(vec![0xff, 0xd8, 0xff, 0xd9]),
            || std::time::Duration::ZERO,
            tick_rx,
        ));

        send_start(&actor, directory.path(), "initial");
        assert_eq!(recv_control(&actor)["state"], "playing");
        running.store(false, Ordering::Release);
        tick_tx.send(std::time::Instant::now()).unwrap();
        assert_eq!(recv_control(&actor)["state"], "stopped");

        actor
            .commands
            .send(ActorCommand::Play {
                request_id: serde_json::json!("replay"),
            })
            .unwrap();
        let replaying = recv_control(&actor);
        assert_eq!(replaying["request_id"], "replay");
        assert_eq!(replaying["state"], "playing");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let frame = loop {
            tick_tx.send(std::time::Instant::now()).unwrap();
            if let Ok(frame) = actor
                .frames
                .recv_timeout(std::time::Duration::from_millis(20))
            {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "play after EOS reported playing but produced no frame"
            );
        };
        let decoded = decode_frame_message(&frame).unwrap();
        assert_eq!(decoded.position_ms, 750);
        assert!(decoded.jpeg.starts_with(&[0xff, 0xd8]));
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn replacement_start_correlates_superseded_paused_start_before_new_response() {
        let directory = tempfile::tempdir().unwrap();
        let actor = fake_actor();
        for request_id in ["first", "replacement"] {
            actor
                .commands
                .send(ActorCommand::Start {
                    request_id: serde_json::json!(request_id),
                    config: valid_empty_config(),
                    base_path: directory.path().to_path_buf(),
                    position_ms: if request_id == "first" { 17 } else { 29 },
                    autoplay: request_id == "replacement",
                })
                .unwrap();
        }

        let superseded = recv_control(&actor);
        assert_eq!(superseded["type"], "error");
        assert_eq!(superseded["request_id"], "first");
        assert_eq!(superseded["epoch"], 1);
        assert_eq!(superseded["state"], "playing");
        assert_eq!(superseded["position_ms"], 17);
        assert_eq!(superseded["error"], "superseded by start");

        let replacement = recv_control(&actor);
        assert_eq!(replacement["type"], "state");
        assert_eq!(replacement["request_id"], "replacement");
        assert_eq!(replacement["epoch"], 2);
        assert_eq!(replacement["state"], "playing");
        assert_eq!(replacement["position_ms"], 29);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn stop_correlates_superseded_paused_start_before_stopped_response() {
        let directory = tempfile::tempdir().unwrap();
        let actor = fake_actor();
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 41,
                autoplay: false,
            })
            .unwrap();
        actor
            .commands
            .send(ActorCommand::Stop {
                request_id: serde_json::json!("stop"),
            })
            .unwrap();

        let superseded = recv_control(&actor);
        assert_eq!(superseded["request_id"], "start");
        assert_eq!(superseded["epoch"], 1);
        assert_eq!(superseded["position_ms"], 41);
        assert_eq!(superseded["error"], "superseded by stop");
        let stopped = recv_control(&actor);
        assert_eq!(stopped["request_id"], "stop");
        assert_eq!(stopped["epoch"], 2);
        assert_eq!(stopped["state"], "stopped");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn stop_correlates_superseded_paused_seek_before_stopped_response() {
        let directory = tempfile::tempdir().unwrap();
        let actor = fake_actor();
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 11,
                autoplay: true,
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "start");
        actor
            .commands
            .send(ActorCommand::Pause {
                request_id: serde_json::json!("pause"),
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "pause");
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("seek"),
                position_ms: 52,
            })
            .unwrap();
        actor
            .commands
            .send(ActorCommand::Stop {
                request_id: serde_json::json!("stop"),
            })
            .unwrap();

        let seek_superseded = recv_control(&actor);
        assert_eq!(seek_superseded["request_id"], "seek");
        assert_eq!(seek_superseded["epoch"], 2);
        assert_eq!(seek_superseded["position_ms"], 52);
        assert_eq!(seek_superseded["error"], "superseded by stop");
        let stopped = recv_control(&actor);
        assert_eq!(stopped["request_id"], "stop");
        assert_eq!(stopped["epoch"], 3);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn replacement_start_correlates_superseded_paused_seek_before_new_response() {
        let directory = tempfile::tempdir().unwrap();
        let actor = fake_actor();
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 11,
                autoplay: true,
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "start");
        actor
            .commands
            .send(ActorCommand::Pause {
                request_id: serde_json::json!("pause"),
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "pause");
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("seek"),
                position_ms: 52,
            })
            .unwrap();
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("replacement"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 88,
                autoplay: true,
            })
            .unwrap();

        let superseded = recv_control(&actor);
        assert_eq!(superseded["request_id"], "seek");
        assert_eq!(superseded["epoch"], 2);
        assert_eq!(superseded["position_ms"], 52);
        assert_eq!(superseded["error"], "superseded by start");
        let replacement = recv_control(&actor);
        assert_eq!(replacement["request_id"], "replacement");
        assert_eq!(replacement["epoch"], 3);
        assert_eq!(replacement["state"], "playing");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn seek_correlates_superseded_paused_start_before_its_response() {
        let directory = tempfile::tempdir().unwrap();
        let actor = fake_actor();
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 11,
                autoplay: false,
            })
            .unwrap();
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("seek"),
                position_ms: 52,
            })
            .unwrap();

        let superseded = recv_control(&actor);
        assert_eq!(superseded["request_id"], "start");
        assert_eq!(superseded["epoch"], 1);
        assert_eq!(superseded["error"], "superseded by seek");
        let seek = recv_control(&actor);
        assert_eq!(seek["request_id"], "seek");
        assert_eq!(seek["epoch"], 2);
        assert_eq!(seek["state"], "playing");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn failed_replacement_teardown_rejects_start_and_keeps_old_engine_owned() {
        let directory = tempfile::tempdir().unwrap();
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let creations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let actor = stalled_actor(
            std::sync::Arc::clone(&released),
            std::sync::Arc::clone(&creations),
        );
        send_start(&actor, directory.path(), "first");
        assert_eq!(recv_control(&actor)["request_id"], "first");
        send_start(&actor, directory.path(), "replacement");

        let rejected = recv_control(&actor);
        assert_eq!(rejected["type"], "error");
        assert_eq!(rejected["request_id"], "replacement");
        assert_eq!(rejected["state"], "shutting_down");
        assert!(rejected["error"].as_str().unwrap().contains("did not stop"));
        assert_eq!(creations.load(std::sync::atomic::Ordering::Acquire), 1);

        released.store(true, std::sync::atomic::Ordering::Release);
        send_start(&actor, directory.path(), "retry");
        let retry = recv_control(&actor);
        assert_eq!(retry["type"], "state");
        assert_eq!(retry["request_id"], "retry");
        assert_eq!(creations.load(std::sync::atomic::Ordering::Acquire), 2);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn failed_stop_reports_error_and_permit_stays_busy_until_reaped() {
        let (state, _state_directory) = crate::webui::api::test_helpers::test_state();
        let permit = state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let creations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let actor = stalled_actor_with_guard(
            std::sync::Arc::clone(&released),
            creations,
            Box::new(permit),
        );
        send_start(&actor, directory.path(), "start");
        assert_eq!(recv_control(&actor)["request_id"], "start");
        actor
            .commands
            .send(ActorCommand::Stop {
                request_id: serde_json::json!("stop"),
            })
            .unwrap();

        let failed = recv_control(&actor);
        assert_eq!(failed["type"], "error");
        assert_eq!(failed["request_id"], "stop");
        assert_eq!(failed["state"], "shutting_down");
        assert_ne!(failed["state"], "stopped");

        let finishing = tokio::spawn(finish_actor(actor));
        tokio::task::yield_now().await;
        assert!(state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_err());
        released.store(true, std::sync::atomic::Ordering::Release);
        tokio::time::timeout(std::time::Duration::from_secs(1), finishing)
            .await
            .unwrap()
            .unwrap();
        assert!(state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_ok());
    }

    #[cfg(feature = "video")]
    #[test]
    fn jpeg_encode_limiter_never_exceeds_twelve_per_second() {
        let mut limiter = EncodeLimiter::default();
        let calls = (0..100)
            .filter(|step| limiter.take(std::time::Duration::from_millis(step * 10)))
            .count();
        assert_eq!(calls, 12);
        assert!(!limiter.take(std::time::Duration::from_millis(1_000)));
        assert!(limiter.take(std::time::Duration::from_millis(1_080)));
    }

    #[cfg(feature = "video")]
    #[test]
    fn actor_limits_actual_encoder_calls_and_completes_pending_one_shot() {
        use std::sync::{
            atomic::{AtomicU64, AtomicUsize, Ordering},
            Arc,
        };

        let directory = tempfile::tempdir().unwrap();
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let now_ms = Arc::new(AtomicU64::new(0));
        let encode_calls = Arc::new(AtomicUsize::new(0));
        let actor_now = Arc::clone(&now_ms);
        let actor_calls = Arc::clone(&encode_calls);
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                Ok(Box::new(FakePreviewEngine {
                    frames: frame_rx.clone(),
                    runtime_errors: crossbeam_channel::never(),
                    running: false,
                    generation: 0,
                }) as Box<dyn crate::video::VideoOutput>)
            },
            move |_rgba, _width, _height, _background| {
                actor_calls.fetch_add(1, Ordering::AcqRel);
                Ok(vec![0xff, 0xd8, 0xff, 0xd9])
            },
            move || std::time::Duration::from_millis(actor_now.load(Ordering::Acquire)),
            tick_rx,
        ));
        send_start(&actor, directory.path(), "start");
        assert_eq!(recv_control(&actor)["request_id"], "start");

        let mut expected_generation = 1;
        for step in 0_u64..100 {
            if step > 0 && step % 30 == 0 {
                actor
                    .commands
                    .send(ActorCommand::Start {
                        request_id: serde_json::json!(format!("start-{step}")),
                        config: valid_empty_config(),
                        base_path: directory.path().to_path_buf(),
                        position_ms: step * 10,
                        autoplay: true,
                    })
                    .unwrap();
                expected_generation = 1;
            }
            if step > 0 && step % 25 == 0 {
                actor
                    .commands
                    .send(ActorCommand::Seek {
                        request_id: serde_json::json!(format!("seek-{step}")),
                        position_ms: step * 10,
                    })
                    .unwrap();
                expected_generation += 1;
            }
            let (step_barrier_tx, step_barrier_rx) = crossbeam_channel::bounded(1);
            actor
                .commands
                .send(ActorCommand::Barrier(step_barrier_tx))
                .unwrap();
            step_barrier_rx.recv().unwrap();
            now_ms.store(step * 10, Ordering::Release);
            frame_tx
                .send(crate::video::VideoFrame {
                    generation: expected_generation,
                    song_position: std::time::Duration::from_millis(step * 10),
                    pts: std::time::Duration::from_millis(step * 10),
                    width: 1,
                    height: 1,
                    rgba: Arc::from([0_u8, 0, 0, 255]),
                })
                .unwrap();
            tick_tx.send(std::time::Instant::now()).unwrap();
            if step % 9 == 0 {
                let expected = (step / 9 + 1) as usize;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while encode_calls.load(Ordering::Acquire) < expected
                    && std::time::Instant::now() < deadline
                {
                    tick_tx.send(std::time::Instant::now()).unwrap();
                    std::thread::yield_now();
                }
                assert_eq!(encode_calls.load(Ordering::Acquire), expected);
            }
        }
        assert_eq!(encode_calls.load(Ordering::Acquire), 12);

        let (churn_barrier_tx, churn_barrier_rx) = crossbeam_channel::bounded(1);
        actor
            .commands
            .send(ActorCommand::Barrier(churn_barrier_tx))
            .unwrap();
        churn_barrier_rx.recv().unwrap();
        while actor.controls.try_recv().is_ok() {}
        actor
            .commands
            .send(ActorCommand::Pause {
                request_id: serde_json::json!("pause-before-final-seek"),
            })
            .unwrap();
        assert_eq!(
            recv_control(&actor)["request_id"],
            "pause-before-final-seek"
        );
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("final-seek"),
                position_ms: 1_080,
            })
            .unwrap();
        let (barrier_tx, barrier_rx) = crossbeam_channel::bounded(1);
        actor
            .commands
            .send(ActorCommand::Barrier(barrier_tx))
            .unwrap();
        barrier_rx.recv().unwrap();
        now_ms.store(1_080, Ordering::Release);
        frame_tx
            .send(crate::video::VideoFrame {
                generation: 2,
                song_position: std::time::Duration::from_millis(1_080),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([0_u8, 0, 0, 255]),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while encode_calls.load(Ordering::Acquire) < 13 && std::time::Instant::now() < deadline {
            tick_tx.send(std::time::Instant::now()).unwrap();
            std::thread::yield_now();
        }
        assert_eq!(encode_calls.load(Ordering::Acquire), 13);
        let completed = recv_control(&actor);
        assert_eq!(completed["type"], "state");
        assert_eq!(completed["request_id"], "final-seek");
        assert_eq!(completed["state"], "paused");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn paused_seek_discards_a_queued_pre_seek_frame() {
        use std::sync::Arc;

        let directory = tempfile::tempdir().unwrap();
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                Ok(Box::new(FakePreviewEngine {
                    frames: frame_rx.clone(),
                    runtime_errors: crossbeam_channel::never(),
                    running: false,
                    generation: 0,
                }))
            },
            |_rgba, _width, _height, _background| Ok(vec![0xff, 0xd8, 0xff, 0xd9]),
            || std::time::Duration::ZERO,
            tick_rx,
        ));
        send_start(&actor, directory.path(), "start");
        assert_eq!(recv_control(&actor)["request_id"], "start");
        actor
            .commands
            .send(ActorCommand::Pause {
                request_id: serde_json::json!("pause"),
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "pause");

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(23),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([1_u8, 0, 0, 255]),
            })
            .unwrap();
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("seek"),
                position_ms: 500,
            })
            .unwrap();
        let (barrier_tx, barrier_rx) = crossbeam_channel::bounded(1);
        actor
            .commands
            .send(ActorCommand::Barrier(barrier_tx))
            .unwrap();
        barrier_rx.recv().unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(
            actor.frames.try_recv().is_err(),
            "pre-seek frame escaped under the new epoch"
        );
        assert!(
            actor.controls.try_recv().is_err(),
            "paused seek completed before its requested frame"
        );

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 2,
                song_position: std::time::Duration::from_millis(500),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([2_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        let frame = actor
            .frames
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let decoded = decode_frame_message(&frame).unwrap();
        assert_eq!((decoded.epoch, decoded.position_ms), (2, 500));
        let completed = recv_control(&actor);
        assert_eq!(completed["request_id"], "seek");
        assert_eq!(completed["state"], "paused");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn failed_seek_discards_late_old_generation_until_matching_generation_starts() {
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };

        let directory = tempfile::tempdir().unwrap();
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let released = Arc::new(AtomicBool::new(false));
        let engine_released = Arc::clone(&released);
        let encode_calls = Arc::new(AtomicUsize::new(0));
        let actor_encode_calls = Arc::clone(&encode_calls);
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                Ok(Box::new(RetrySeekEngine {
                    released: Arc::clone(&engine_released),
                    frames: frame_rx.clone(),
                    running: false,
                    generation: 0,
                }))
            },
            move |_rgba, _width, _height, _background| {
                actor_encode_calls.fetch_add(1, Ordering::AcqRel);
                Ok(vec![0xff, 0xd8, 0xff, 0xd9])
            },
            || std::time::Duration::ZERO,
            tick_rx,
        ));
        send_start(&actor, directory.path(), "start");
        assert_eq!(recv_control(&actor)["state"], "playing");

        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("failed-seek"),
                position_ms: 500,
            })
            .unwrap();
        let failed = recv_control(&actor);
        assert_eq!(failed["request_id"], "failed-seek");
        assert_eq!(failed["state"], "shutting_down");

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(500),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([1_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(encode_calls.load(Ordering::Acquire), 0);
        assert!(actor.frames.try_recv().is_err());

        released.store(true, Ordering::Release);
        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("replacement-seek"),
                position_ms: 600,
            })
            .unwrap();
        let replacement = recv_control(&actor);
        assert_eq!(replacement["request_id"], "replacement-seek");
        assert_eq!(replacement["state"], "playing");

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(600),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([1_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(encode_calls.load(Ordering::Acquire), 0);

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 2,
                song_position: std::time::Duration::from_millis(600),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([2_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();
        let frame = actor
            .frames
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(decode_frame_message(&frame).unwrap().position_ms, 600);
        assert_eq!(encode_calls.load(Ordering::Acquire), 1);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn jpeg_failure_is_the_only_terminal_response_for_paused_start() {
        use std::sync::Arc;

        let directory = tempfile::tempdir().unwrap();
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                Ok(Box::new(FakePreviewEngine {
                    frames: frame_rx.clone(),
                    runtime_errors: crossbeam_channel::never(),
                    running: false,
                    generation: 0,
                }))
            },
            |_rgba, _width, _height, _background| Err("controlled JPEG failure".to_string()),
            || std::time::Duration::ZERO,
            tick_rx,
        ));
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("paused-start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 77,
                autoplay: false,
            })
            .unwrap();
        frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(77),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([0_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();

        let failed = recv_control(&actor);
        assert_eq!(failed["type"], "error");
        assert_eq!(failed["request_id"], "paused-start");
        assert_eq!(failed["error"], "controlled JPEG failure");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(
            actor.controls.try_recv().is_err(),
            "false paused success followed JPEG error"
        );
        assert!(
            actor.frames.try_recv().is_err(),
            "failed JPEG emitted a binary frame"
        );
        actor
            .commands
            .send(ActorCommand::Play {
                request_id: serde_json::json!("later-play"),
            })
            .unwrap();
        assert_eq!(recv_control(&actor)["request_id"], "later-play");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn autoplay_jpeg_failure_reports_correlated_terminal_error_before_shutdown() {
        use std::sync::Arc;

        let directory = tempfile::tempdir().unwrap();
        let (frame_tx, frame_rx) = crossbeam_channel::unbounded();
        let (tick_tx, tick_rx) = crossbeam_channel::unbounded();
        let actor = spawn_preview_actor_with(ActorDependencies::new(
            move |_config, _base_path, _bound_media| {
                Ok(Box::new(FakePreviewEngine {
                    frames: frame_rx.clone(),
                    runtime_errors: crossbeam_channel::never(),
                    running: false,
                    generation: 0,
                }))
            },
            |_rgba, _width, _height, _background| {
                Err("controlled autoplay JPEG failure".to_string())
            },
            || std::time::Duration::ZERO,
            tick_rx,
        ));
        send_start(&actor, directory.path(), "autoplay-start");
        let playing = recv_control(&actor);
        assert_eq!(playing["type"], "state");
        assert_eq!(playing["request_id"], "autoplay-start");
        assert_eq!(playing["state"], "playing");

        frame_tx
            .send(crate::video::VideoFrame {
                generation: 1,
                song_position: std::time::Duration::from_millis(23),
                pts: std::time::Duration::ZERO,
                width: 1,
                height: 1,
                rgba: Arc::from([0_u8, 0, 0, 255]),
            })
            .unwrap();
        tick_tx.send(std::time::Instant::now()).unwrap();

        let failed = recv_control(&actor);
        assert_eq!(failed["type"], "error");
        assert_eq!(failed["request_id"], "autoplay-start");
        assert_eq!(failed["state"], "shutting_down");
        assert_eq!(failed["position_ms"], 23);
        assert_eq!(failed["error"], "controlled autoplay JPEG failure");
        assert!(actor.frames.try_recv().is_err());
        assert!(actor.shutdown.load(std::sync::atomic::Ordering::Acquire));
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn duplicate_id_does_not_steal_active_lifecycle_reservation() {
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let start = capacity.clone().acquire_many_owned(2).await.unwrap();
        let request_id = serde_json::json!("X");
        let mut reservations = ResponseReservations::default();
        reservations.insert(&request_id, start, true);
        let reordered_same_id: serde_json::Value =
            serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let ordered_id: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(
            request_id_key(&ordered_id),
            request_id_key(&reordered_same_id)
        );

        let acknowledgement = serde_json::json!({
            "type": "state", "request_id": "X", "state": "playing"
        })
        .to_string();
        drop(take_response_reservation(&acknowledgement, &mut reservations).unwrap());
        assert!(reservations.contains(&request_id));

        // This is the direct bounded response permit used when transport
        // admission rejects a duplicate before submitting it to the actor.
        let duplicate_error = capacity.clone().acquire_owned().await.unwrap();
        assert!(reservations.contains(&request_id));
        drop(duplicate_error);

        let terminal = serde_json::json!({
            "type": "state", "request_id": "X", "state": "stopped"
        })
        .to_string();
        drop(take_response_reservation(&terminal, &mut reservations).unwrap());
        assert!(!reservations.contains(&request_id));

        let next_id = serde_json::json!("next");
        let next = capacity.clone().acquire_owned().await.unwrap();
        reservations.insert(&next_id, next, false);
        let next_response = serde_json::json!({
            "type": "state", "request_id": "next", "state": "stopped"
        })
        .to_string();
        drop(take_response_reservation(&next_response, &mut reservations).unwrap());
        assert_eq!(capacity.available_permits(), 4);
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn blocked_frame_path_retains_controls_and_only_latest_frame() {
        use axum::extract::ws::Message;

        let (controls, mut control_rx) = tokio::sync::mpsc::channel(2);
        let (frames, frame_rx) = tokio::sync::watch::channel(None::<Vec<u8>>);
        for value in 0_u8..100 {
            frames.send_replace(Some(vec![value]));
        }
        controls.try_send(Message::Text("first".into())).unwrap();
        controls.try_send(Message::Text("second".into())).unwrap();

        assert_eq!(
            control_rx.recv().await.unwrap().into_text().unwrap(),
            "first"
        );
        assert_eq!(
            control_rx.recv().await.unwrap().into_text().unwrap(),
            "second"
        );
        assert_eq!(frame_rx.borrow().as_deref(), Some(&[99][..]));
    }

    #[cfg(feature = "video")]
    #[test]
    fn full_control_queue_cancels_without_blocking_the_actor() {
        let (control_tx, control_rx) = crossbeam_channel::bounded(1);
        control_tx.send("already queued".to_string()).unwrap();
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shutdown = std::sync::Arc::clone(&shutdown);
        let (finished_tx, finished_rx) = crossbeam_channel::bounded(1);

        let sender = std::thread::spawn(move || {
            send_error(
                &control_tx,
                &thread_shutdown,
                Some(serde_json::json!("overflow")),
                1,
                "busy",
                0,
                "control queue overflow".to_string(),
            );
            finished_tx.send(()).unwrap();
        });

        let completed = finished_rx.recv_timeout(std::time::Duration::from_millis(250));
        let _ = control_rx.try_recv();
        sender.join().unwrap();
        assert!(completed.is_ok(), "control overflow blocked the actor");
        assert!(shutdown.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            control_rx.try_recv().is_err(),
            "overflow response was queued"
        );
    }

    #[cfg(feature = "video")]
    #[test]
    fn blocked_writer_capacity_never_orphans_an_accepted_command() {
        let actor = fake_actor();
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
        let mut reservations = Vec::new();
        let mut accepted = Vec::new();

        for index in 0..64 {
            let Ok(reservation) = capacity.clone().try_acquire_owned() else {
                break;
            };
            let request_id = format!("blocked-{index}");
            actor
                .commands
                .send(ActorCommand::Play {
                    request_id: serde_json::json!(request_id),
                })
                .unwrap();
            accepted.push(request_id);
            reservations.push(reservation);
        }

        assert_eq!(accepted.len(), 32);
        assert!(capacity.clone().try_acquire_owned().is_err());
        let mut responded = Vec::new();
        for _ in 0..accepted.len() {
            responded.push(
                recv_control(&actor)["request_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        responded.sort();
        accepted.sort();
        assert_eq!(responded, accepted);
        assert!(!actor.shutdown.load(std::sync::atomic::Ordering::Acquire));
        drop(reservations);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn permit_is_retained_until_a_stalled_actor_really_exits() {
        let (state, _directory) = crate::webui::api::test_helpers::test_state();
        let permit = state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .unwrap();
        let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
        let (command_tx, _command_rx) = crossbeam_channel::bounded(1);
        let (_control_tx, control_rx) = crossbeam_channel::bounded(1);
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded(1);
        let actor = PreviewActor {
            commands: command_tx,
            controls: control_rx,
            frames: frame_rx,
            join: Some(std::thread::spawn(move || {
                let _permit = permit;
                let _ = release_rx.recv();
            })),
            shutdown: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        let finishing = tokio::spawn(finish_actor(actor));
        tokio::task::yield_now().await;
        assert!(state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_err());
        release_tx.send(()).unwrap();
        finishing.await.unwrap();
        assert!(state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_ok());
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn aborting_async_owner_keeps_permit_until_actor_really_exits() {
        let (state, _directory) = crate::webui::api::test_helpers::test_state();
        let permit = state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let creations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let actor = stalled_actor_with_guard(
            std::sync::Arc::clone(&released),
            creations,
            Box::new(permit),
        );
        send_start(&actor, directory.path(), "start");
        assert_eq!(recv_control(&actor)["state"], "playing");

        let owner = tokio::spawn(async move {
            let _actor = actor;
            std::future::pending::<()>().await;
        });
        owner.abort();
        let _ = owner.await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_err());

        released.store(true, std::sync::atomic::Ordering::Release);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if state
                .video_preview_permit
                .clone()
                .try_acquire_owned()
                .is_ok()
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[cfg(feature = "video")]
    #[test]
    fn pending_one_shot_is_explicitly_correlated_when_play_or_pause_wins_race() {
        for (request_id, transport) in [("start", "play"), ("seek", "pause")] {
            let mut pending = Some(PendingOneShot {
                request_id: serde_json::json!(request_id),
            });
            assert_eq!(
                PendingOneShot::cancel(&mut pending),
                Some(serde_json::json!(request_id)),
                "{transport}"
            );
            assert!(
                pending.is_none(),
                "{transport} left pause-after-frame armed"
            );
        }
    }

    #[cfg(feature = "video")]
    #[test]
    fn invalid_start_returns_correlated_error() {
        use config::{Config, File, FileFormat};
        let invalid = Config::builder()
            .add_source(File::from_str(
                r##"canvas: { width: 0, height: 4, frame_rate: 4, background: "#000000" }
tracks: []"##,
                FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let actor = spawn_preview_actor(None);
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("invalid-start"),
                config: invalid,
                base_path: directory.path().to_path_buf(),
                position_ms: 77,
                autoplay: false,
            })
            .unwrap();

        let value: serde_json::Value = serde_json::from_str(
            &actor
                .controls
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["request_id"], "invalid-start");
        assert_eq!(value["position_ms"], 77);
        actor.shutdown();
    }

    #[test]
    fn binary_header_round_trips_epoch_position_and_jpeg() {
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let encoded = encode_frame_message(0x0102_0304_0506_0708, 42_123, &jpeg);

        assert_eq!(&encoded[..4], b"MTRK");
        assert_eq!(encoded[4], 1);
        assert_eq!(&encoded[5..8], &[0, 0, 0]);
        let decoded = decode_frame_message(&encoded).unwrap();
        assert_eq!(decoded.epoch, 0x0102_0304_0506_0708);
        assert_eq!(decoded.position_ms, 42_123);
        assert_eq!(decoded.jpeg, jpeg);
    }

    #[cfg(feature = "video")]
    #[test]
    fn paused_empty_timeline_start_emits_frame_and_completes() {
        let directory = tempfile::tempdir().unwrap();
        let actor = spawn_preview_actor(None);
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("empty-start"),
                config: valid_empty_config(),
                base_path: directory.path().to_path_buf(),
                position_ms: 375,
                autoplay: false,
            })
            .unwrap();

        let frame = actor
            .frames
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let decoded = decode_frame_message(&frame).unwrap();
        assert_eq!((decoded.epoch, decoded.position_ms), (1, 375));
        let paused = recv_control(&actor);
        assert_eq!(paused["request_id"], "empty-start");
        assert_eq!(paused["state"], "paused");
        assert_eq!(paused["position_ms"], 375);
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[test]
    fn actor_forwards_first_frame_then_pauses_and_advances_epochs() {
        use config::{Config, File, FileFormat};
        use std::process::Command;
        use std::time::Duration;

        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("clip.mkv");
        let status = Command::new("timeout")
            .args([
                "10s",
                "gst-launch-1.0",
                "-q",
                "videotestsrc",
                "pattern=red",
                "num-buffers=8",
                "!",
                "video/x-raw,format=RGBA,width=4,height=4,framerate=4/1",
                "!",
                "videoconvert",
                "!",
                "avenc_ffv1",
                "!",
                "matroskamux",
                "!",
                "filesink",
                &format!("location={}", media.display()),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let config: crate::config::video::VideoConfig = Config::builder()
            .add_source(File::from_str(
                r##"canvas: { width: 4, height: 4, frame_rate: 4, background: "#000000" }
tracks: [{ name: screen, clips: [{ id: clip, start: 0, duration: 2, file: clip.mkv }] }]"##,
                FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        let actor = spawn_preview_actor(None);
        actor
            .commands
            .send(ActorCommand::Start {
                request_id: serde_json::json!("one"),
                config,
                base_path: directory.path().to_path_buf(),
                position_ms: 250,
                autoplay: false,
            })
            .unwrap();

        let frame = actor.frames.recv_timeout(Duration::from_secs(5)).unwrap();
        let header = decode_frame_message(&frame).unwrap();
        assert_eq!(header.epoch, 1);
        assert_eq!(header.position_ms, 250);
        let paused: serde_json::Value =
            serde_json::from_str(&actor.controls.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(paused["type"], "state");
        assert_eq!(paused["state"], "paused");
        assert_eq!(paused["epoch"], 1);
        assert_eq!(paused["request_id"], "one");

        actor
            .commands
            .send(ActorCommand::Play {
                request_id: serde_json::json!("play"),
            })
            .unwrap();
        let playing: serde_json::Value =
            serde_json::from_str(&actor.controls.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(playing["state"], "playing");
        assert_eq!(playing["epoch"], 1);
        assert_eq!(playing["request_id"], "play");
        actor
            .commands
            .send(ActorCommand::Pause {
                request_id: serde_json::json!("pause"),
            })
            .unwrap();
        let paused_again: serde_json::Value =
            serde_json::from_str(&actor.controls.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(paused_again["state"], "paused");
        assert_eq!(paused_again["epoch"], 1);
        assert_eq!(paused_again["request_id"], "pause");

        actor
            .commands
            .send(ActorCommand::Seek {
                request_id: serde_json::json!("seek"),
                position_ms: 500,
            })
            .unwrap();
        let frame = actor.frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(decode_frame_message(&frame).unwrap().epoch, 2);
        let seek_paused: serde_json::Value =
            serde_json::from_str(&actor.controls.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(seek_paused["request_id"], "seek");
        actor
            .commands
            .send(ActorCommand::Stop {
                request_id: serde_json::json!("stop"),
            })
            .unwrap();
        let mut stop = None;
        for _ in 0..3 {
            let value: serde_json::Value =
                serde_json::from_str(&actor.controls.recv_timeout(Duration::from_secs(5)).unwrap())
                    .unwrap();
            if value["state"] == "stopped" {
                stop = Some(value);
                break;
            }
        }
        let stop = stop.unwrap();
        assert_eq!(stop["epoch"], 3);
        assert_eq!(stop["request_id"], "stop");
        actor.shutdown();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    async fn active_player_excludes_start_play_and_seek_but_not_pause_or_stop() {
        use crate::player::{Player, PlayerDevices};
        use std::{collections::HashMap, path::Path, sync::Arc};

        let songs = crate::songs::get_all_songs(Path::new("assets/songs")).unwrap();
        let mut playlists = HashMap::new();
        playlists.insert(
            "all_songs".to_string(),
            crate::playlist::from_songs(songs).unwrap(),
        );
        let player = Arc::new(
            Player::new_with_devices(
                PlayerDevices {
                    audio: Some(Arc::new(crate::audio::mock::Device::get(
                        "preview-exclusion",
                    ))),
                    mappings: None,
                    midi: None,
                    dmx_engine: None,
                    sample_engine: None,
                    trigger_engine: None,
                },
                playlists,
                "all_songs".to_string(),
                None,
            )
            .unwrap(),
        );
        player.play().await.unwrap();
        assert!(player.is_playing().await);

        let commands = [
            (
                ClientCommand::Start {
                    request_id: serde_json::json!("start"),
                    config: valid_empty_config(),
                    position_ms: 10,
                    autoplay: false,
                },
                true,
            ),
            (
                ClientCommand::Play {
                    request_id: serde_json::json!("play"),
                },
                true,
            ),
            (
                ClientCommand::Seek {
                    request_id: serde_json::json!("seek"),
                    position_ms: 20,
                },
                true,
            ),
            (
                ClientCommand::Pause {
                    request_id: serde_json::json!("pause"),
                },
                false,
            ),
            (
                ClientCommand::Stop {
                    request_id: serde_json::json!("stop"),
                },
                false,
            ),
        ];
        for (command, excluded) in commands {
            assert_eq!(
                command.is_excluded_during_player_playback(),
                excluded,
                "{}",
                command.request_id()
            );
            if excluded {
                let request_id = command.request_id().clone();
                let error: serde_json::Value =
                    serde_json::from_str(&command.player_exclusion_error(7, "paused", 33)).unwrap();
                assert_eq!(error["type"], "error");
                assert_eq!(error["request_id"], request_id);
                assert_eq!(error["epoch"], 7);
                assert_eq!(error["state"], "paused");
                assert_eq!(
                    error["position_ms"],
                    if request_id == "start" { 10 } else { 33 }
                );
            }
        }
        assert!(
            player.is_playing().await,
            "admission checks stopped the player"
        );
        player.stop().await.unwrap();
    }

    #[cfg(feature = "video")]
    #[test]
    fn json_commands_deserialize_the_editor_protocol() {
        let start: ClientCommand = serde_json::from_str(
            r##"{"type":"start","request_id":"load-1","config":{"canvas":{"width":4,"height":4,"frame_rate":2,"background":"#000000"},"tracks":[]},"position_ms":125,"autoplay":false}"##,
        )
        .unwrap();
        assert!(matches!(
            start,
            ClientCommand::Start {
                request_id,
                position_ms: 125,
                autoplay: false,
                ..
            } if request_id == "load-1"
        ));
        assert!(matches!(
            serde_json::from_str::<ClientCommand>(r#"{"type":"play","request_id":"play-1"}"#).unwrap(),
            ClientCommand::Play { request_id } if request_id == "play-1"
        ));
        assert!(matches!(
            serde_json::from_str::<ClientCommand>(r#"{"type":"pause","request_id":"pause-1"}"#).unwrap(),
            ClientCommand::Pause { request_id } if request_id == "pause-1"
        ));
        assert!(matches!(
            serde_json::from_str::<ClientCommand>(r#"{"type":"seek","request_id":"seek-1","position_ms":900}"#).unwrap(),
            ClientCommand::Seek { request_id, position_ms: 900 } if request_id == "seek-1"
        ));
        assert!(matches!(
            serde_json::from_str::<ClientCommand>(r#"{"type":"stop","request_id":"stop-1"}"#).unwrap(),
            ClientCommand::Stop { request_id } if request_id == "stop-1"
        ));
        for missing_id in [
            r#"{"type":"start","config":{},"position_ms":0,"autoplay":false}"#,
            r#"{"type":"play"}"#,
            r#"{"type":"pause"}"#,
            r#"{"type":"seek","position_ms":0}"#,
            r#"{"type":"stop"}"#,
        ] {
            assert!(
                serde_json::from_str::<ClientCommand>(missing_id).is_err(),
                "accepted {missing_id}"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn independently_created_web_ui_states_share_one_preview_session() {
        let (first_state, _first_directory) = crate::webui::api::test_helpers::test_state();
        let (second_state, _second_directory) = crate::webui::api::test_helpers::test_state();
        let first = first_state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .unwrap();
        assert!(second_state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_err());
        drop(first);
        assert!(second_state
            .video_preview_permit
            .clone()
            .try_acquire_owned()
            .is_ok());
    }

    #[cfg(all(feature = "video", unix))]
    #[test]
    fn clip_path_validation_rejects_symlink_escape() {
        use config::{Config, File, FileFormat};
        use std::os::unix::fs::symlink;

        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("inside.mkv"), b"x").unwrap();
        std::fs::write(outside.path().join("secret.mkv"), b"x").unwrap();
        symlink(outside.path(), base.path().join("escape")).unwrap();
        let parse = |file: &str| -> crate::config::video::VideoConfig {
            Config::builder()
                .add_source(File::from_str(
                    &format!(
                        "tracks: [{{ name: screen, clips: [{{ id: clip, start: 0, duration: 1, file: {file} }}] }}]"
                    ),
                    FileFormat::Yaml,
                ))
                .build()
                .unwrap()
                .try_deserialize()
                .unwrap()
        };

        let mut inside = parse("inside.mkv");
        let bound_inside = sanitize_clip_paths(base.path(), &mut inside).unwrap();
        assert_eq!(
            inside.tracks()[0].clips()[0].file(),
            format!(
                "/proc/self/fd/{}",
                std::os::fd::AsRawFd::as_raw_fd(&bound_inside.files[0])
            )
        );
        assert_eq!(bound_inside.base_path, base.path().canonicalize().unwrap());
        let mut escaping = parse("escape/secret.mkv");
        let error = sanitize_clip_paths(base.path(), &mut escaping).unwrap_err();
        assert!(error.contains("outside the song directory"), "{error}");
        let mut missing = parse("missing.mkv");
        assert!(sanitize_clip_paths(base.path(), &mut missing)
            .unwrap_err()
            .contains("does not exist"));

        let original = base.path().join("original.mkv");
        std::fs::write(&original, b"original").unwrap();
        symlink("original.mkv", base.path().join("selected.mkv")).unwrap();
        let mut selected = parse("selected.mkv");
        let _bound_selected = sanitize_clip_paths(base.path(), &mut selected).unwrap();
        std::fs::remove_file(&original).unwrap();
        symlink(outside.path().join("secret.mkv"), &original).unwrap();
        assert_eq!(
            std::fs::read(selected.tracks()[0].clips()[0].file()).unwrap(),
            b"original",
            "engine-visible sanitized path followed the replaced symlink"
        );
    }

    #[cfg(all(feature = "video", target_os = "linux"))]
    #[test]
    fn preview_binding_deduplicates_files_and_caps_unique_media() {
        use config::{Config, File, FileFormat};

        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("shared.mkv"), b"shared").unwrap();
        let mut duplicated: crate::config::video::VideoConfig = Config::builder()
            .add_source(File::from_str(
                "tracks: [{ name: a, clips: [{ id: a, start: 0, duration: 1, file: shared.mkv }] }, { name: b, clips: [{ id: b, start: 0, duration: 1, file: shared.mkv }] }]",
                FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        let bound = sanitize_clip_paths(base.path(), &mut duplicated).unwrap();
        assert_eq!(bound.files.len(), 1);
        assert_eq!(
            duplicated.tracks()[0].clips()[0].file(),
            duplicated.tracks()[1].clips()[0].file()
        );

        let mut clips = Vec::new();
        for index in 0..=MAX_UNIQUE_MEDIA_FILES {
            let name = format!("file-{index}.mkv");
            std::fs::write(base.path().join(&name), b"x").unwrap();
            clips.push(format!(
                "{{ id: c{index}, start: {index}, duration: 1, file: {name} }}"
            ));
        }
        let mut excessive: crate::config::video::VideoConfig = Config::builder()
            .add_source(File::from_str(
                &format!("tracks: [{{ name: media, clips: [{}] }}]", clips.join(",")),
                FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert!(excessive.validate().is_ok());
        let error = sanitize_clip_paths(base.path(), &mut excessive).unwrap_err();
        assert!(
            error.contains("unique media files must not exceed 256"),
            "{error}"
        );
    }

    #[cfg(feature = "video")]
    #[test]
    fn jpeg_is_bounded_and_flattens_alpha_over_canvas_background() {
        use image::GenericImageView;

        let mut rgba = vec![0_u8; 2000 * 600 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[255, 0, 0, 128]);
        }
        let jpeg = encode_preview_jpeg(&rgba, 2000, 600, "#0000ff").unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();

        assert_eq!(decoded.dimensions(), (960, 288));
        let pixel = decoded.to_rgb8().get_pixel(400, 100).0;
        assert!(pixel[0].abs_diff(128) < 8, "pixel={pixel:?}");
        assert!(pixel[1] < 8, "pixel={pixel:?}");
        assert!(pixel[2].abs_diff(127) < 8, "pixel={pixel:?}");
    }

    #[test]
    fn binary_header_rejects_short_bad_magic_version_and_reserved_bytes() {
        assert!(decode_frame_message(&[0; 23]).is_err());

        let mut message = encode_frame_message(1, 2, &[3]);
        message[0] = b'X';
        assert!(decode_frame_message(&message).is_err());
        message[0] = b'M';
        message[4] = 2;
        assert!(decode_frame_message(&message).is_err());
        message[4] = 1;
        message[6] = 1;
        assert!(decode_frame_message(&message).is_err());
    }
}
