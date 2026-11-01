# Video Editor Preview Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Show the real GStreamer-composited output inside mTrack's Video tab with responsive start, play, pause, seek, reload and stop controls.

**Architecture:** Add a synchronized preview mode to `GstVideoEngine`, then expose one editor-scoped preview actor through a dedicated binary WebSocket at `/api/songs/{name}/video-preview/ws`. The browser sends the current draft `VideoConfig`; the server streams latest-only JPEG frames with epoch and position metadata. A Svelte canvas presents those JPEGs above the existing timeline. The global JSON `/ws` and live Player remain untouched.

**Tech Stack:** Rust 2021, GStreamer/appsink, Axum WebSocket, optional `image` JPEG encoder, Svelte 5/TypeScript, Playwright.

---

### Task 1: Add a bounded synchronized preview mode

**Files:** `Cargo.toml`, `src/video.rs`, `tests/video_engine.rs`

1. Write failing tests proving preview frames are clock-synchronized, latest-only/bounded, and cheap to clone.
2. Add `GstVideoEngine::preview`, `OutputMode::Preview`, bounded latest-frame delivery, and `Arc<[u8]>` frame storage.
3. Preserve deterministic headless tests and complete program output.
4. Run feature-video and no-feature checks.

### Task 2: Add the dedicated preview WebSocket service

**Files:** create `src/webui/video_preview.rs`; modify `src/webui.rs`, `src/webui/api.rs`, `src/webui/server.rs`, `src/webui/api/status.rs`, `Cargo.toml`

1. Write protocol/unit tests first for JSON commands, binary `MTRK` frame headers, epoch filtering and JPEG output.
2. Add a single-session permit in `WebUiState` and a blocking actor owning `GstVideoEngine`.
3. Resolve song name through the loaded song registry and canonicalize every media path beneath the song root.
4. Reject preview while Player playback is active.
5. Implement start/play/pause/seek/stop; paused start/seek must emit one frame and remain paused.
6. Encode composed frames to bounded-size JPEG at max 12 fps; use latest-only channels and clean disconnect teardown.
7. Report `capabilities.video_preview`; a non-video build returns an explicit disabled response.

### Task 3: Add the Svelte preview canvas and controls

**Files:** create `src/webui/svelte/src/lib/video/preview.ts`, `src/webui/svelte/src/components/video/VideoPreview.svelte`; modify `VideoTimelineEditor.svelte`, `SongDetail.svelte`, status types, mock server and timeline Playwright tests.

1. Write failing protocol/client tests and Playwright tests.
2. Render a 16:9 checkerboard canvas above the timeline.
3. Add reload, play, pause, seek, stop and fullscreen controls with loading/error/disabled states.
4. Start/reload sends the unsaved draft config; timeline edits mark preview stale.
5. Discard old epochs, revoke object URLs and close/stop on tab navigation/destruction.
6. Verify responsive/mobile layout and keyboard-accessible controls.

### Task 4: End-to-end verification and review

1. Run Rust feature/no-feature suites, Svelte check/lint and Playwright video tests.
2. Start a real local mTrack preview with deterministic layered media.
3. Capture a screenshot/video showing the preview in the actual UI.
4. Run independent spec and code-quality reviews; fix all critical/important findings.
5. Commit only after all gates pass.
