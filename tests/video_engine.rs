// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

#![cfg(feature = "video")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use config::{Config, File, FileFormat};
use mtrack::{
    config::video::VideoConfig,
    video::{GstVideoEngine, VideoFrame, VideoOutput},
};

fn video_config(yaml: &str) -> VideoConfig {
    Config::builder()
        .add_source(File::from_str(yaml, FileFormat::Yaml))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap()
}

fn make_video(directory: &Path, name: &str, pattern: &str) -> PathBuf {
    let path = directory.join(name);
    let status = Command::new("timeout")
        .args([
            "10s",
            "gst-launch-1.0",
            "-q",
            "videotestsrc",
            &format!("pattern={pattern}"),
            "num-buffers=6",
            "!",
            "video/x-raw,format=RGBA,width=4,height=4,framerate=2/1",
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
    path
}

fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    frame[offset..offset + 4].try_into().unwrap()
}

fn assert_opaque_color(actual: [u8; 4], expected: [u8; 3]) {
    for (actual, expected) in actual[..3].iter().zip(expected) {
        assert!(
            actual.abs_diff(expected) <= 10,
            "actual={actual:?}, expected={expected:?}"
        );
    }
    assert_eq!(actual[3], 255, "actual={actual:?}");
}

fn make_sequence_video(directory: &Path, name: &str, colors: &[[u8; 4]]) -> PathBuf {
    let raw = directory.join(format!("{name}.rgba"));
    let bytes: Vec<u8> = colors
        .iter()
        .flat_map(|color| std::iter::repeat_n(*color, 16).flatten())
        .collect();
    fs::write(&raw, bytes).unwrap();
    let path = directory.join(name);
    let status = Command::new("timeout")
        .args([
            "10s",
            "gst-launch-1.0",
            "-q",
            "filesrc",
            &format!("location={}", raw.display()),
            "!",
            "rawvideoparse",
            "format=rgba",
            "width=4",
            "height=4",
            "framerate=2/1",
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
        .expect("failed to run bounded GStreamer sequence fixture generator");
    assert!(
        status.success(),
        "sequence fixture generation failed: {status}"
    );
    path
}

#[test]
fn cloned_video_frame_shares_rgba_allocation() {
    let frame = VideoFrame {
        generation: 1,
        song_position: Duration::ZERO,
        pts: Duration::ZERO,
        width: 2,
        height: 2,
        rgba: Arc::from(vec![0_u8; 16]),
    };

    let cloned = frame.clone();

    assert!(Arc::ptr_eq(&frame.rgba, &cloned.rgba));
}

#[test]
#[should_panic(expected = "exactly one frame receiver")]
fn frame_stream_has_exactly_one_consumer() {
    let directory = tempfile::tempdir().unwrap();
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks: []
"##,
    );
    let engine = GstVideoEngine::headless(config, directory.path()).unwrap();

    let _first = engine.frame_receiver();
    let _second = engine.frame_receiver();
}

#[test]
fn missing_media_has_contextual_error() {
    let config = video_config(
        r##"
canvas: { width: 16, height: 16, frame_rate: 5, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: missing, start: 0, duration: 1, file: missing.mp4 }
"##,
    );

    let error = GstVideoEngine::headless(config, Path::new("/definitely/not/here")).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("missing"), "{message}");
    assert!(message.contains("missing.mp4"), "{message}");

    // Keep the object-safe transport contract exercised by this test crate.
    fn accepts_output(_output: &mut dyn VideoOutput) {}
    let _ = accepts_output;
    let _ = Duration::ZERO;
}

#[cfg(unix)]
#[test]
fn public_engine_rejects_media_symlink_escaping_song_directory() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.mkv"), b"secret").unwrap();
    symlink(
        outside.path().join("secret.mkv"),
        directory.path().join("escape.mkv"),
    )
    .unwrap();
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: escape, start: 0, duration: 1, file: escape.mkv }
"##,
    );

    let error = GstVideoEngine::headless(config, directory.path()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("outside"), "{message}");
    assert!(message.contains("song directory"), "{message}");
}

#[cfg(target_os = "linux")]
#[test]
fn public_engine_rejects_user_supplied_proc_fd_paths() {
    use std::os::fd::AsRawFd;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inside.mkv");
    fs::write(&path, b"inside").unwrap();
    let file = fs::File::open(path).unwrap();
    let config = video_config(&format!(
        r##"
canvas: {{ width: 4, height: 4, frame_rate: 2, background: "#000000" }}
tracks:
  - name: screen
    clips:
      - {{ id: injected, start: 0, duration: 1, file: /proc/self/fd/{} }}
"##,
        file.as_raw_fd()
    ));

    let error = GstVideoEngine::headless(config, directory.path()).unwrap_err();
    assert!(error.to_string().contains("safe relative path"), "{error}");
}

#[test]
fn invalid_clip_layout_is_rejected_before_engine_pipeline_construction() {
    let directory = tempfile::tempdir().unwrap();
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: oversized, start: 0, duration: 1, file: missing.mkv, width: 4097, height: 1 }
"##,
    );

    let error = GstVideoEngine::headless(config, directory.path()).unwrap_err();
    assert!(
        matches!(error, mtrack::video::VideoError::InvalidConfig(_)),
        "invalid layout reached engine construction: {error}"
    );
}

#[test]
fn corrupt_existing_media_reports_contextual_runtime_error() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("corrupt.mkv"), b"not a media file").unwrap();
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: corrupt, start: 0, duration: 1, file: corrupt.mkv }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let errors = engine.runtime_error_receiver();

    engine.start(Duration::ZERO).unwrap();

    let message = errors
        .recv_timeout(Duration::from_secs(5))
        .expect("corrupt media error was not reported")
        .to_string();
    assert!(message.contains("corrupt"), "{message}");
    assert!(message.contains("corrupt.mkv"), "{message}");
    assert!(
        message.contains("preroll") || message.contains("decode"),
        "{message}"
    );
}

#[test]
fn uncovered_canvas_pixels_are_transparent() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#123456" }
tracks:
  - name: later
    clips:
      - { id: red, start: 0.5, duration: 0.5, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    let first = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.song_position, Duration::ZERO);
    assert!(first.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0));
    engine.cancel().unwrap();
}

#[test]
fn positions_and_scales_a_clip_over_transparent_pixels() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "green.mkv", "green");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: overlay
    clips:
      - { id: green, start: 0, duration: 0.5, file: green.mkv, x: 1, y: 1, width: 2, height: 2 }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(pixel(&frame.rgba, 4, 0, 0)[3], 0);
    assert_eq!(pixel(&frame.rgba, 4, 3, 3)[3], 0);
    for y in 1..3 {
        for x in 1..3 {
            assert_opaque_color(pixel(&frame.rgba, 4, x, y), [0, 255, 0]);
        }
    }
    engine.cancel().unwrap();
}

#[test]
fn later_track_wins_where_positioned_clips_overlap() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    make_video(directory.path(), "green.mkv", "green");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: back
    clips:
      - { id: red, start: 0, duration: 0.5, file: red.mkv, x: 0, y: 0, width: 3, height: 3 }
  - name: front
    clips:
      - { id: green, start: 0, duration: 0.5, file: green.mkv, x: 1, y: 1, width: 2, height: 2 }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_opaque_color(pixel(&frame.rgba, 4, 0, 0), [255, 0, 0]);
    assert_opaque_color(pixel(&frame.rgba, 4, 1, 1), [0, 255, 0]);
    assert_eq!(pixel(&frame.rgba, 4, 3, 3)[3], 0);
    engine.cancel().unwrap();
}

#[test]
fn source_in_selects_a_later_media_frame() {
    let directory = tempfile::tempdir().unwrap();
    make_sequence_video(
        directory.path(),
        "sequence.mkv",
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ],
    );
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: selected, start: 0, duration: 0.5, file: sequence.mkv, source_in: 1.0 }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(frame
        .rgba
        .chunks_exact(4)
        .all(|pixel| { pixel[2] >= 250 && pixel[0] <= 5 && pixel[1] <= 5 && pixel[3] == 255 }));
    engine.cancel().unwrap();
}

#[test]
fn compositor_applies_base_opacity_and_fade_envelopes_each_frame() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: faded, start: 0, duration: 2, file: red.mkv, opacity: 0.5, fade_in: 1, fade_out: 1 }
      - { id: tail, start: 2, duration: 0.5, file: red.mkv, opacity: 0 }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    for (position, expected_alpha) in [(0, 0), (500, 64), (1_000, 128), (1_500, 64), (2_000, 0)] {
        let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(frame.song_position, Duration::from_millis(position));
        let actual = pixel(&frame.rgba, 4, 0, 0);
        assert!(actual[0] >= 245, "position={position}, pixel={actual:?}");
        assert!(
            actual[3].abs_diff(expected_alpha) <= 2,
            "position={position}, pixel={actual:?}"
        );
    }
    engine.cancel().unwrap();
}

#[test]
fn compositor_keeps_opacity_bound_to_source_track_after_timeline_sorting() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "green.mkv", "green");
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: later-back-track
    clips:
      - { id: later-green, start: 0.5, duration: 1.5, file: green.mkv, x: 0, y: 0, width: 2, height: 4, opacity: 0.25 }
  - name: earlier-front-track
    clips:
      - { id: earlier-red, start: 0, duration: 2, file: red.mkv, x: 2, y: 0, width: 2, height: 4, opacity: 0.75 }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();

    let first = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.song_position, Duration::ZERO);
    assert_eq!(pixel(&first.rgba, 4, 0, 0)[3], 0);
    assert!(pixel(&first.rgba, 4, 2, 0)[3].abs_diff(191) <= 2);

    let second = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(second.song_position, Duration::from_millis(500));
    let green = pixel(&second.rgba, 4, 0, 0);
    let red = pixel(&second.rgba, 4, 2, 0);
    assert!(green[1] >= 245, "green pixel={green:?}");
    assert!(green[3].abs_diff(64) <= 2, "green pixel={green:?}");
    assert!(red[0] >= 245, "red pixel={red:?}");
    assert!(red[3].abs_diff(191) <= 2, "red pixel={red:?}");
    engine.cancel().unwrap();
}

#[test]
fn decoded_content_obeys_source_start_seek_and_end_offsets() {
    let directory = tempfile::tempdir().unwrap();
    make_sequence_video(
        directory.path(),
        "timeline.mkv",
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
            [255, 0, 255, 255],
            [0, 255, 255, 255],
            [255, 255, 0, 255],
        ],
    );
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: selected
    clips:
      - { id: offset, start: 1.0, duration: 1.5, file: timeline.mkv, source_in: 0.5 }
  - name: timeline-tail
    clips:
      - { id: tail, start: 3.0, duration: 0.5, file: timeline.mkv }
"##,
    );
    let mut engine = GstVideoEngine::headless(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();

    engine.start(Duration::ZERO).unwrap();
    let mut at_start = None;
    let mut after_start = None;
    let mut at_end = None;
    while at_end.is_none() {
        let frame = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("timeline frame watchdog expired");
        match frame.song_position {
            position if position == Duration::from_secs(1) => at_start = Some(frame),
            position if position == Duration::from_millis(1_500) => after_start = Some(frame),
            position if position == Duration::from_millis(2_500) => at_end = Some(frame),
            _ => {}
        }
    }
    let at_start = at_start.expect("missing start-inclusive frame");
    assert_opaque_color(pixel(&at_start.rgba, 4, 0, 0), [0, 255, 0]);
    let after_start = after_start.expect("missing frame after clip start");
    assert_opaque_color(pixel(&after_start.rgba, 4, 0, 0), [0, 0, 255]);
    assert!(at_end
        .unwrap()
        .rgba
        .chunks_exact(4)
        .all(|pixel| pixel[3] == 0));

    engine.seek(Duration::from_millis(1_500)).unwrap();
    engine.play().unwrap();
    let sought = frames
        .recv_timeout(Duration::from_secs(5))
        .expect("seeked frame watchdog expired");
    assert_eq!(sought.song_position, Duration::from_millis(1_500));
    assert_opaque_color(pixel(&sought.rgba, 4, 0, 0), [0, 0, 255]);
    engine.cancel().unwrap();
}

#[test]
fn transport_pause_seek_play_and_stop_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 2, file: red.mkv }
"##,
    );
    let (result_tx, result_rx) = crossbeam_channel::bounded(1);

    std::thread::spawn(move || {
        let result = (|| {
            let mut engine = GstVideoEngine::headless(config, directory.path())?;
            let frames = engine.frame_receiver();

            engine.start(Duration::ZERO)?;
            frames.recv_timeout(Duration::from_secs(5)).unwrap();
            engine.pause()?;
            assert!(!engine.is_running());

            engine.seek(Duration::from_secs(1))?;
            assert!(!engine.is_running());
            engine.play()?;
            let sought = frames.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(sought.song_position, Duration::from_secs(1));

            let stopped = Instant::now();
            engine.stop()?;
            assert!(!engine.is_running());
            assert!(stopped.elapsed() < Duration::from_secs(10));
            Ok::<_, mtrack::video::VideoError>(())
        })();
        let _ = result_tx.send(result);
    });

    result_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("transport sequence exceeded its watchdog")
        .expect("transport sequence failed");
}

#[test]
fn preview_frames_advance_in_real_time() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 2, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();

    engine.start(Duration::ZERO).unwrap();
    let first = frames.recv_timeout(Duration::from_secs(2)).unwrap();
    let first_received = Instant::now();
    assert_eq!(first.song_position, Duration::ZERO);
    assert!(
        engine.is_running(),
        "preview raced to EOS after its first frame"
    );
    let second = frames.recv_timeout(Duration::from_secs(2)).unwrap();

    assert_eq!(second.song_position, Duration::from_millis(500));
    assert!(
        first_received.elapsed() >= Duration::from_millis(350),
        "preview frame interval was faster than the configured clock: {:?}",
        first_received.elapsed()
    );
    engine.cancel().unwrap();
}

#[test]
fn preview_empty_timeline_emits_one_transparent_frame_at_requested_position() {
    let directory = tempfile::tempdir().unwrap();
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#123456" }
tracks: []
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();

    engine.start(Duration::from_millis(375)).unwrap();

    let frame = frames.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(frame.song_position, Duration::from_millis(375));
    assert!(frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0));
    assert!(frames.recv_timeout(Duration::from_millis(250)).is_err());
    engine.cancel().unwrap();
}

#[test]
fn preview_at_timeline_end_emits_one_transparent_frame_at_requested_position() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#abcdef" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 1, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();

    engine.start(Duration::from_millis(1_250)).unwrap();

    let frame = frames.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(frame.song_position, Duration::from_millis(1_250));
    assert!(frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0));
    assert!(frames.recv_timeout(Duration::from_millis(250)).is_err());
    engine.cancel().unwrap();
}

#[test]
fn play_after_natural_eos_resumes_at_end_without_replaying_last_frame() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#abcdef" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 1, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!engine.is_running(), "preview did not reach natural EOS");
    while frames.try_recv().is_ok() {}

    engine.play().unwrap();
    let resumed = frames.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(resumed.song_position, Duration::from_secs(1));
    engine.cancel().unwrap();
}

#[test]
fn slow_preview_consumer_receives_only_the_latest_frame() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 10, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 2, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();

    engine.start(Duration::ZERO).unwrap();
    std::thread::sleep(Duration::from_millis(1_100));

    assert!(
        frames.len() <= 1,
        "preview retained {} historical frames",
        frames.len()
    );
    let frame = frames.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        frame.song_position >= Duration::from_millis(700),
        "slow consumer received stale frame at {:?}",
        frame.song_position
    );
    engine.cancel().unwrap();
}

#[test]
fn preview_cancellation_interrupts_synchronized_frame_wait() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 0.1, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 100, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    engine.start(Duration::ZERO).unwrap();
    frames.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let cancelled = Instant::now();
    engine.cancel().unwrap();

    assert!(
        cancelled.elapsed() < Duration::from_secs(1),
        "preview cancellation was blocked by its synchronized sink for {:?}",
        cancelled.elapsed()
    );
}

#[test]
fn slow_frame_rate_preview_stays_alive_between_frames() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 0.1, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 100, file: red.mkv }
"##,
    );
    let mut engine = GstVideoEngine::preview(config, directory.path()).unwrap();
    let frames = engine.frame_receiver();
    let errors = engine.runtime_error_receiver();
    engine.start(Duration::ZERO).unwrap();
    frames.recv_timeout(Duration::from_secs(2)).unwrap();

    std::thread::sleep(Duration::from_millis(250));

    assert!(
        engine.is_running(),
        "preview stopped between low-fps frames"
    );
    assert!(
        errors.try_recv().is_err(),
        "preview reported a false timeout"
    );
    engine.cancel().unwrap();
}

#[test]
fn program_output_presents_every_frame_in_order() {
    let directory = tempfile::tempdir().unwrap();
    let colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
        [255, 0, 255, 255],
        [0, 255, 255, 255],
    ];
    make_sequence_video(directory.path(), "program-sequence.mkv", &colors);
    let config = video_config(
        r##"
canvas: { width: 4, height: 4, frame_rate: 2, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: sequence, start: 0, duration: 3, file: program-sequence.mkv }
"##,
    );
    let mut engine = GstVideoEngine::program(config, directory.path(), Some("appsink")).unwrap();
    let frames = engine.frame_receiver();
    let errors = engine.runtime_error_receiver();

    let started = Instant::now();
    engine.start(Duration::ZERO).unwrap();
    let presented: Vec<_> = (0..colors.len())
        .map(|index| {
            let frame = frames
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| {
                    panic!(
                        "missing synchronized program frame {index}; runtime error: {:?}",
                        errors.try_recv()
                    )
                });
            (frame.song_position, pixel(&frame.rgba, 4, 0, 0))
        })
        .collect();

    for (index, ((position, actual), expected)) in presented.iter().zip(colors).enumerate() {
        assert_eq!(*position, Duration::from_millis(index as u64 * 500));
        assert_opaque_color(*actual, expected[..3].try_into().unwrap());
    }
    assert!(
        started.elapsed() >= Duration::from_millis(2_400),
        "synchronized program sink was bypassed: {:?}",
        started.elapsed()
    );

    let cancel_started = Instant::now();
    engine.cancel().unwrap();
    assert!(cancel_started.elapsed() < Duration::from_secs(2));
}

#[test]
fn program_output_backpressure_does_not_block_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    make_video(directory.path(), "red.mkv", "red");
    let config = video_config(
        r##"
canvas: { width: 1024, height: 1024, frame_rate: 0.1, background: "#000000" }
tracks:
  - name: screen
    clips:
      - { id: red, start: 0, duration: 100, file: red.mkv }
"##,
    );
    let (result_tx, result_rx) = crossbeam_channel::bounded(1);

    std::thread::spawn(move || {
        let result = (|| {
            let mut engine = GstVideoEngine::program(config, directory.path(), Some("fakesink"))?;
            engine.start(Duration::ZERO)?;
            std::thread::sleep(Duration::from_millis(250));
            engine.cancel()
        })();
        let _ = result_tx.send(result);
    });

    result_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("program-output cancellation exceeded its watchdog")
        .expect("program-output worker did not stop cleanly");
}
