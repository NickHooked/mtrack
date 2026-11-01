# M-Track Linux Video Timeline Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Add a Linux-native video timeline to M-Track that can place video files and live USB capture sources at exact song times, composite ordered tracks, provide draggable picture-in-picture, and send the result to one fullscreen program output while remaining synchronized with audio, MIDI, and DMX.

**Architecture:** Keep M-Track's audio-derived `PlaybackClock` as the only transport authority. Add an optional GStreamer-backed video subsystem that participates in the existing readiness barrier in `Player::play_files`, receives the same `PlaybackSync`, and renders on its own dedicated GLib/GStreamer thread. Store show intent in each song's YAML (`video:` timeline) and machine-specific capture/output bindings in the active hardware profile. Extend the existing Svelte song timeline rather than creating a second editor. Feed a reduced-rate, non-blocking composite preview to the browser; the program output must never depend on the browser.

**Tech Stack:** Rust 2021, GStreamer 1.x (`gstreamer`, `gstreamer-video`, `gstreamer-app` Rust crates), Linux V4L2 initially, Axum/WebSocket, Svelte 5/TypeScript, Playwright, existing M-Track YAML/Serde configuration and playback synchronization primitives.

**Baseline inspected:** `c59ce4031c07269808eda49fa99033d1c06a5846` on 2026-09-22.

---

## 1. Product scope

### V1 must support

1. One song-bound, linear video timeline.
2. Multiple ordered video tracks; upper tracks render above lower tracks.
3. File clips placed at arbitrary absolute times with source in/out points.
4. Live inputs from UVC/V4L2 USB capture cards placed as bounded timeline clips.
5. Capture warm-up before the clip in-point; no stale frame after seek or stop.
6. Per-clip position, size, crop, rotation, opacity, aspect-ratio lock, fit and fill.
7. Direct drag/resize manipulation in the browser preview for PiP.
8. Hard cuts and a linear opacity fade-in/fade-out.
9. One fixed-resolution program canvas and one fullscreen Linux output.
10. Low-rate editor preview, video health/status, and clear missing/busy-device errors.
11. Play, pause, stop, seek, song change, full-song loop, and section-loop behavior that remains coherent with audio/MIDI/DMX.
12. Save/reopen without changing the source media.

### Down-the-line requirement — roadmap point 10: external timecode follow

M-Track must eventually be able to run the video timeline as a **timecode follower**, so its own audio playback is not required when show audio comes from another DAW, playback machine or console.

Support both:

- **LTC (Linear Timecode)** decoded from a selected audio input/channel;
- **MTC (MIDI Time Code)** received from a selected MIDI input. (The standard name is MTC, not M2C.)

This is deliberately not part of the first video release, but the V1 architecture must not make it difficult later. Keep transport position behind an extensible clock/transport-source abstraction rather than baking the audio sample counter directly into the video engine.

Future timecode-follow semantics must explicitly define and test:

- selectable frame rates: 24, 25, 29.97 drop-frame/non-drop-frame where applicable, and 30 fps;
- timecode offset between incoming show time and M-Track song time;
- acquire/lock indication and measured drift;
- start/chase behavior when timecode appears mid-song;
- forward jumps, backward jumps and discontinuities as synchronized seeks;
- stop timeout and configurable freewheel during brief signal loss;
- clean relock without stale video, duplicated MIDI events or incorrect DMX state;
- whether M-Track transport controls are disabled, ignored or act as an offset while externally slaved;
- status/API feedback showing source (`internal`, `ltc`, or `mtc`), frame rate, current timecode, lock state and signal age;
- operation with M-Track audio playback disabled while video, MIDI and DMX continue to follow the external clock.

Recommended later architecture:

- extend `PlaybackClock`/transport timing with an external, discontinuity-aware source rather than feeding decoded LTC/MTC directly into the video subsystem;
- implement LTC ingest as its own audio-input path, independent of normal M-Track output playback;
- implement MTC quarter-frame assembly plus full-frame locate handling;
- turn large timecode discontinuities into the same coordinated restart-based seek used by the current Player, so every active subsystem lands on one position;
- add holdover/freewheel only after acquire, loss and relock behavior is covered by deterministic tests.

### Explicitly not V1

- Millumin dashboard/cue-column mode.
- Nested timelines, timeline proxies, or multi-canvas projects.
- Keyframes or animated transforms.
- General shader/effect system, chroma key, blend modes, masks, perspective warp, projection mapping, soft edge.
- NDI, RTMP, Syphon, SDI/DeckLink output, HDR/10-bit, alpha output, or multi-output routing.
- External LTC/MTC follow or generation. Follow mode is retained as roadmap point 10 above; M-Track's internal transport remains master in V1.
- Live-safe "Silent Edition" editing.
- Video audio. Embedded audio is muted; M-Track's existing audio tracks remain authoritative.

### Practical acceptance show

- Track 1 plays a background H.264 file from song time `00:00.000`.
- Track 2 opens a USB HDMI capture source before `00:30.000`, fades it in, and displays it as a draggable PiP until `01:00.000`.
- Seeking to `00:20`, `00:45`, and `01:10` produces respectively background-only, live PiP, and background-only output without stale frames or a locked capture device.
- Pause freezes the visible output and resumes at the preserved position; Stop returns to the configured stopped frame/black and releases time-bound media resources.
- Saving and reopening reproduces timing, source assignment, z-order, crop, and geometry.
- Fullscreen output continues if the editor browser disconnects.

---

## 2. Product decisions

### 2.1 Use "Millumin", not "Lumen"

The reference product is **Millumin**: its timeline, ordered layers, live capture inputs, workspace transforms, effects and outputs match the requested workflow. Lumen is a different macOS video synthesizer.

### 2.2 Extend the song timeline, not the audio `tracks` model

Do not overload `src/config/track.rs::Track`. Existing audio tracks are full-song mono/channel sources and determine audio routing. Video needs multiple clips per track, arbitrary start/end times, source offsets, transforms, fades and live-source references. Add a separate optional `video:` block to `song.yaml`.

### 2.3 Separate show intent from machine bindings

- Song YAML says **when and how** `camera_main` appears.
- The active hardware profile says **which physical V4L2 device** `camera_main` means and which display/output is used.

This follows M-Track's existing portability model for audio, MIDI and DMX and prevents `/dev/video0` from leaking into every song.

### 2.4 Audio remains the master clock

When audio exists, `PlaybackClock` derives from the output device's sample counter (`src/clock.rs`). Video must follow that clock and may drop/duplicate frames to catch up; audio must never be resampled or delayed to chase video. Without audio, the existing monotonic wall clock remains master.

### 2.5 Dedicated media thread, same process for V1

Run GStreamer and its GLib main loop on a dedicated thread and expose a small Rust trait to Player. This fits the existing synchronized-subsystem model and lets video share `PlaybackClock` and `CancelHandle` exactly. Keep all GStreamer callbacks out of CPAL's real-time audio callback.

A crash-isolated `mtrack video-worker` process is a post-V1 hardening option. Do not begin there: cross-process clock transport would add complexity before the timeline semantics are proven.

### 2.6 Feature-gate video

Build video behind Cargo feature `video` so existing headless/audio-only packages do not gain mandatory GStreamer runtime dependencies. A build without the feature must still parse projects without video only if no active song/profile uses video; otherwise report a direct actionable error.

### 2.7 Browser preview is not the program output

The program compositor branches through `tee`:

- branch A: full-rate GPU-backed program sink;
- branch B: leaky queue, downscale, low-rate JPEG frames to `appsink` for editor preview.

If preview is slow or no browser is connected, branch B drops frames. It must never back-pressure branch A.

---

## 3. Proposed configuration contract

### 3.1 Song YAML

```yaml
kind: song
name: Example
tracks:
  - name: backing-l
    file: backing.wav
    file_channel: 1
  - name: backing-r
    file: backing.wav
    file_channel: 2

video:
  canvas:
    width: 1920
    height: 1080
    frame_rate: 50
    background: "#000000"
  tracks:
    - name: background
      clips:
        - id: intro-film
          start: 0s
          duration: 90s
          source:
            type: file
            file: visuals/intro.mp4
            in: 0s
          transform:
            fit: fill
          fade_out: 500ms

    - name: camera-pip
      clips:
        - id: guitar-camera
          start: 30s
          duration: 30s
          source:
            type: capture
            input: camera_main
          warmup: 1500ms
          transform:
            x: 0.68
            y: 0.64
            width: 0.28
            height: 0.28
            crop:
              left: 0.0
              top: 0.0
              right: 0.0
              bottom: 0.0
            rotation_degrees: 0.0
            lock_aspect: true
          opacity: 1.0
          fade_in: 300ms
          fade_out: 300ms
```

Rules:

- `start`, `duration`, `in`, `warmup`, `fade_in`, and `fade_out` use `duration-string`, already present in M-Track.
- Normalized `x`, `y`, `width`, `height`, and crop values make projects resolution-independent. Coordinates use a top-left canvas origin.
- Track list order is back-to-front; later tracks have greater z-order.
- Clip IDs are unique within the song and remain stable across editor operations.
- File paths resolve relative to the song directory and go through existing safe-path rules.
- Capture clips have no source seek; on seek into an active capture clip, open/warm immediately and reveal only after the first valid frame.
- Embedded file audio is disabled.
- V1 rejects overlapping clips on the same track. Simultaneous material belongs on separate tracks. This avoids ambiguous same-track transition semantics; fades are per clip.
- Song duration becomes the maximum of audio duration, MIDI/lighting end, and video clip end.

### 3.2 Hardware profile YAML

```yaml
kind: hardware_profile
hostname: mtrack-video

video:
  output:
    backend: auto
    display: HDMI-A-1
    fullscreen: true
  inputs:
    camera_main:
      backend: v4l2
      match:
        serial: "CAPTURE-SERIAL-HERE"
        vendor_id: "534d"
        product_id: "2109"
      format:
        width: 1920
        height: 1080
        frame_rate: 50
        pixel_format: YUY2
  preview:
    width: 960
    height: 540
    frame_rate: 12
    jpeg_quality: 75
```

Rules:

- Prefer stable serial/vendor/product metadata; `/dev/videoN` may be an explicit fallback but is not the durable identity.
- Reject ambiguous matches and show every matching node.
- Output backend `auto` chooses Wayland/X11 in desktop sessions; a later phase may add DRM/KMS.
- Missing optional video hardware does not prevent audio playback, but status is red and the program canvas shows black. Add `required: true` only after production semantics are agreed with the maintainer.

---

## 4. Runtime model

### 4.1 New trait

Create `src/video.rs` and `src/video/engine.rs` with an object-safe interface parallel to existing subsystems:

```rust
pub trait Device: Send + Sync {
    fn play_from(
        &self,
        song: Arc<crate::songs::Song>,
        sync: crate::playsync::PlaybackSync,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn status(&self) -> VideoStatus;
    fn latest_preview(&self) -> Option<PreviewFrame>;
}
```

`play_from` must:

1. Build/prepare all clips active at `start_time` plus near-future clips inside the preload horizon.
2. Signal `ReadyGuard` only after the program sink and every required currently-active source can produce.
3. Wait for `PlaybackClock::start()` like audio/MIDI/DMX.
4. Schedule visibility from `start_time + clock.elapsed()`.
5. Observe cancellation quickly.
6. Clear compositor pads and release capture devices during teardown.

### 4.2 Seek semantics

Current M-Track seek is restart-based (`src/player/seek.rs`): cancel all subsystems, wait for wind-down, then call `play_from_locked(position)`. Reuse this. Do not invent an independent video seek API.

For a file clip at song position `P`:

```text
source_position = clip.in + (P - clip.start)
```

For a capture clip active at `P`:

- open immediately;
- hold its compositor alpha at zero until the first valid frame;
- reveal it at the correct fade value for `P`;
- never display the final frame from the previous playback generation.

Tag every playback generation and reject late bus/appsink messages from older generations.

### 4.3 Pause/stop semantics

M-Track pause currently captures elapsed, performs Stop, then stores a pending start. In V1:

- preserve the last composed preview/program frame while paused;
- release file decoders and capture devices after retaining that frame;
- Play rebuilds at the stored position;
- Stop clears the retained frame and shows black or configured background.

This matches the existing user-visible pause position while avoiding capture-card ownership while paused. If retaining the last program frame proves sink-specific, V1 may show black while paused only after the behavior is explicitly documented and accepted; do not silently differ between preview and program output.

### 4.4 Section and full-song loops

At loop boundaries, current audio/MIDI/DMX use `LoopControl` without restarting the whole Player. Video must observe the same active section and consumed-loop time. Add loop-transition tests before declaring V1 complete. A simpler initial implementation may rebuild the affected video graph at the loop boundary, provided it meets the frame/latency acceptance limit and cannot stale-lock a capture source.

---

## 5. Implementation tasks

### Task 0: Land an architecture RFC and prove GStreamer viability

**Objective:** Remove the two largest unknowns—dynamic seekable composition and live-source warm-up—before changing the persistent schema.

**Files:**
- Create: `docs/design/video-timeline.md`
- Create: `examples/video_spike.rs`
- Modify: `Cargo.toml`

**Steps:**

1. Add an opt-in `video-spike` example using feature-gated GStreamer dependencies.
2. Build one `compositor` pipeline with:
   - one local video file;
   - one `v4l2src` or `videotestsrc` fallback;
   - dynamic x/y/width/height/alpha;
   - one real output sink and one leaky preview `appsink` branch.
3. Demonstrate start from a non-zero file timestamp.
4. Demonstrate hiding/showing the live source without rebuilding the whole app.
5. Demonstrate teardown and immediate reopen of the same capture node 50 times.
6. Measure first-frame latency and PTS drift against a monotonic reference for 30 minutes.
7. Record whether the implementation uses a custom pipeline or GStreamer Editing Services. Prefer a custom pipeline unless GES handles live-source clips and restart-based seeks without a custom source subclass.
8. Delete the spike only after its tests/lessons have been transferred into production modules.

**Commands:**

```bash
cargo run --features video --example video_spike -- \
  --file /path/to/test.mp4 --capture /dev/video0 --duration 1800
cargo test --features video
```

**Gate:** Do not start the editor until the spike proves file seek, capture reopen, compositor transforms, and a non-blocking preview branch.

**Commit:** `spike: validate gstreamer video timeline primitives`

### Task 1: Add validated video configuration types

**Objective:** Parse, validate, serialize and round-trip the song timeline and profile bindings without changing playback.

**Files:**
- Create: `src/config/video.rs`
- Modify: `src/config.rs`
- Modify: `src/config/song.rs`
- Modify: `src/config/profile.rs`
- Modify: `src/config/kind.rs` only if a new standalone config kind is actually needed (not expected)
- Test: unit tests colocated in the files above
- Modify: `docs/src/configuration/song-config.md`
- Modify: `docs/src/configuration/hardware-profiles.md`

**Tests first:**

- Empty/missing `video` remains backward-compatible.
- Valid file and capture clips round-trip exactly.
- Duplicate IDs fail.
- Zero/negative duration fails.
- Non-finite/out-of-range normalized geometry, crop, opacity and rotation fail.
- Fade durations cannot exceed clip duration.
- File clips require a safe relative path.
- Capture clips require a non-empty logical input name.
- Same-track overlap fails in V1.
- Missing/zero canvas dimensions or frame rate fail.
- Ambiguous/malformed hardware selectors fail.

**Commands:**

```bash
cargo test config::video --all-features
cargo test config::song --all-features
cargo test config::profile --all-features
```

**Commit:** `feat(video): add timeline and hardware configuration`

### Task 2: Resolve config into runtime video models and song duration

**Objective:** Convert YAML into absolute, validated runtime clips and include video in total song length.

**Files:**
- Create: `src/video.rs`
- Create: `src/video/timeline.rs`
- Modify: `src/lib.rs`
- Modify: `src/songs.rs`
- Test: `src/video/timeline.rs` unit tests and `src/songs.rs` tests

**Tests first:**

- Runtime clips are sorted by time and stable track order.
- `active_clips_at(P)` returns the correct z-ordered set before, inside and after bounds.
- File source position after seek is `in + P - start`.
- Fade opacity is correct at start, middle and end.
- Video-only songs receive a non-zero duration.
- Existing audio-only songs retain identical duration and behavior.
- Missing media path is a load failure with song, track and clip ID in the message.

**Commit:** `feat(video): resolve song timelines into runtime clips`

### Task 3: Add optional GStreamer build and startup diagnostics

**Objective:** Keep non-video builds clean while giving video builds actionable plugin diagnostics.

**Files:**
- Modify: `Cargo.toml`
- Modify: `setup.sh`
- Modify: `packaging/debian/*`
- Create: `src/video/gstreamer.rs`
- Modify: `src/main.rs` or CLI diagnostics path
- Test: compile checks with and without `--features video`

**Requirements:**

- `default` build behavior must be decided with the maintainer; recommended default is no video feature for backward-compatible headless binaries, plus a separate video-enabled release asset/package.
- At startup, enumerate required elements (`uridecodebin`/decode path, `v4l2src`, `compositor`, `videoconvert`, output sink, `appsink`, JPEG encoder) and list exactly which are missing.
- Do not initialize GStreamer on audio-only profiles.

**Commands:**

```bash
cargo check --no-default-features
cargo check --features video
cargo test --no-default-features
cargo test --features video
```

**Commit:** `build(video): add optional gstreamer backend`

### Task 4: Implement capture discovery and stable matching

**Objective:** Resolve logical input names to exactly one Linux capture endpoint and expose discovery to the UI.

**Files:**
- Create: `src/video/discovery.rs`
- Modify: `src/webui/api/devices.rs`
- Modify: `src/webui/api.rs`
- Modify: `src/webui/svelte/src/lib/api/config.ts`
- Modify: `src/webui/svelte/src/components/config/*`
- Test: Rust unit/API tests and Playwright mock data/tests

**Requirements:**

- Read V4L2/udev identity and advertised formats.
- Return stable serial, vendor/product, node, display name and formats.
- Detect zero, one and multiple matches.
- Never silently choose the first node when a selector matches several endpoints.
- Show busy/permission-denied separately from missing.

**Commit:** `feat(video): discover and bind Linux capture inputs`

### Task 5: Implement the compositor and source lifecycle

**Objective:** Render files and V4L2 sources into a fixed canvas with deterministic z-order and basic clip properties.

**Files:**
- Create: `src/video/engine.rs`
- Create: `src/video/pipeline.rs`
- Create: `src/video/source.rs`
- Create: `src/video/compositor.rs`
- Test: unit tests plus headless integration tests using `videotestsrc`

**Tests first:**

- Correct z-order for two sources.
- Geometry conversion from normalized canvas values.
- Crop and rotation produce expected test-frame hashes or sampled pixels.
- Fade alpha at known timestamps.
- A source error blackens only its clip/layer and updates status.
- Late callbacks from a previous playback generation are ignored.
- Repeated prepare/play/cancel leaves no capture device open.

**Important:** Keep GStreamer/GLib work on its media thread. Communication into it must use bounded channels; status and preview producers must drop data rather than block the media thread.

**Commit:** `feat(video): add realtime gstreamer compositor`

### Task 6: Integrate video into Player's synchronized barrier

**Objective:** Make video start, stop and seek with the same clock and cancellation generation as audio/MIDI/DMX.

**Files:**
- Modify: `src/player.rs`
- Modify: `src/player/hardware.rs`
- Modify: `src/player/playback.rs`
- Modify: `src/player/seek.rs` only if tests expose missing general subsystem behavior
- Modify: `src/playsync.rs` only for reusable readiness/error semantics
- Modify: `src/state.rs`
- Test: player unit tests and harness checks

**Implementation points:**

- Add `video_device` to `HardwareState`, `PlayerDevices`, and `PlaybackContext`.
- Count video in `has_*`, `expected_ready`, join and cancellation handling in `Player::play_files`.
- Pass the same `PlaybackSync { cancel_handle, ready_tx, clock, start_time, loop_control }`.
- Do not let preview readiness satisfy program-output readiness.
- Add a bounded video-ready timeout with an actionable error; never leave audio/MIDI/DMX spinning forever.
- Decide required/optional failure behavior explicitly; tests must cover both if both are supported.

**Tests first:**

- All four subsystems see the same start barrier.
- Seek waits for old video teardown before reopening capture.
- Stop during startup cannot later reveal an old frame.
- Pause preserves position and chosen freeze/black behavior.
- Audio-only behavior and existing test counts remain unchanged.

**Commands:**

```bash
cargo test player::playback --all-features
cargo test player::seek --all-features
cargo test --workspace --all-features
```

**Commit:** `feat(video): synchronize video with player transport`

### Task 7: Implement program output and preview transport

**Objective:** Produce one reliable fullscreen output and a non-blocking browser preview.

**Files:**
- Create: `src/video/output.rs`
- Create: `src/video/preview.rs`
- Modify: `src/webui/server.rs`
- Modify: `src/webui/state.rs`
- Modify: `src/webui/api/status.rs`
- Test: API/WebSocket tests and back-pressure tests

**Preview protocol recommendation:**

- Add `/ws/video-preview` with binary JPEG frames and a small JSON metadata/status message.
- Only encode when at least one preview subscriber exists.
- Use a channel capacity of one latest frame; replace/drop old frames.
- Include canvas dimensions, playback generation and song position in metadata.

**Tests first:**

- Slow/disconnected preview clients do not reduce program frame rate.
- Two clients receive current frames without creating two encoders.
- Stale-generation frames are rejected.
- Program sink remains active after editor disconnect.

**Commit:** `feat(video): add fullscreen output and editor preview`

### Task 8: Extend song REST APIs and safe media handling

**Objective:** Let the existing song editor load/save video config and import supported video files safely.

**Files:**
- Modify: `src/webui/api/songs_api.rs`
- Modify: `src/webui/safe_path.rs` only if existing rules are insufficient
- Modify: `src/webui/svelte/src/lib/api/songs.ts`
- Test: `songs_api.rs` tests and mock server

**Requirements:**

- Add supported video extensions separately from audio extensions.
- Probe metadata/duration/size/frame-rate server-side through the chosen backend.
- Keep uploaded/imported media under the project root.
- Validate before atomic save and reload.
- Return clip-specific errors, not generic YAML failures.
- Preserve unknown compatible YAML fields according to the project's current editing strategy.

**Commit:** `feat(video): expose timeline media through song APIs`

### Task 9: Add timeline state and reusable video editor primitives

**Objective:** Build timeline interactions on the existing editor's zoom, ruler, snapping, dirty guard and preview transport.

**Files:**
- Create: `src/webui/svelte/src/lib/video/types.ts`
- Create: `src/webui/svelte/src/lib/video/timeline-state.ts`
- Create: `src/webui/svelte/src/components/video/VideoTrackLane.svelte`
- Create: `src/webui/svelte/src/components/video/VideoClipBlock.svelte`
- Create: `src/webui/svelte/src/components/video/VideoPropertiesPanel.svelte`
- Modify: `src/webui/svelte/src/components/songs/SectionTimelineEditor.svelte` or extract a shared timeline shell if that avoids duplication
- Modify: `src/webui/svelte/src/components/songs/SongDetail.svelte`
- Test: unit/Playwright tests

**UI behavior:**

- Add video lanes below musical markers/waveforms in the song Timeline tab.
- Drag empty media from a library panel onto a lane.
- Drag a clip body to move it; drag edges to trim timeline duration/source in-point.
- Reorder/add/remove tracks.
- Snap to playhead, clip edges, sections, measures and beats when available.
- Use keyboard delete and arrow nudging.
- Keep selected clip and unsaved state stable while changing song-detail tabs.

**Tests first:**

- Pixel/time conversion and zoom-anchor behavior.
- Drag, trim, snap, track reorder, selection and delete.
- Overlap prevention on one track.
- Save/reload round-trip.
- Lock mode disables authoring but not playback.

**Commit:** `feat(video): add multitrack clip timeline editor`

### Task 10: Add direct-manipulation PiP canvas

**Objective:** Let an operator drag and resize the selected active clip on top of the live composite preview.

**Files:**
- Create: `src/webui/svelte/src/components/video/VideoCanvas.svelte`
- Create: `src/webui/svelte/src/components/video/TransformHandles.svelte`
- Create: `src/webui/svelte/src/lib/video/transform.ts`
- Modify: video properties/editor components from Task 9
- Test: transform unit tests and Playwright pointer tests

**Behavior:**

- Preview uses `object-fit: contain`; pointer coordinates convert through letterbox offsets into normalized canvas coordinates.
- Drag moves; corner handles resize; optional modifier preserves aspect ratio.
- Properties expose exact x/y/width/height/crop/rotation/opacity.
- Provide Fit, Fill, Center and Reset buttons.
- Optimistic local drag updates do not write YAML on every pointer event; commit on pointer release and save through normal song save.
- If current playhead is outside the selected clip, offer "Preview selected clip" in authoring mode without changing live program output. If live-safe preview is deferred, state this and require seeking into the clip.

**Commit:** `feat(video): add draggable picture-in-picture editor`

### Task 11: Add video health, logs and operator preflight

**Objective:** Make failures visible before and during a show.

**Files:**
- Modify: `src/player.rs` (`HardwareStatusSnapshot`)
- Modify: `src/player/hardware.rs`
- Modify: `src/webui/api/status.rs`
- Modify: `src/webui/svelte/src/pages/StatusPage.svelte`
- Modify: status/store types and translations
- Test: status API and Playwright tests

**Status data:**

- backend initialized;
- output connected/active;
- measured output FPS and dropped frames;
- active clips and source states;
- each capture input: matched node, negotiated format, first-frame latency, frames received, last error;
- preview subscribers/encoder state;
- decoder/GStreamer error.

**Preflight checks:**

- every referenced file exists and probes;
- every logical capture source resolves exactly once;
- requested formats negotiate;
- output/display exists;
- canvas/frame-rate supported;
- estimated decode load warning, not a fake guarantee.

**Commit:** `feat(video): expose health and preflight diagnostics`

### Task 12: Add full automated and hardware-shaped validation

**Objective:** Prove transport correctness, resource cleanup and operator workflow rather than only config parsing.

**Files:**
- Add: `harness/src/checks/video.rs`
- Modify: `harness/src/checks.rs`
- Add: `src/webui/svelte/e2e/video-timeline.spec.ts`
- Modify: mock server/test data
- Add: test assets under the project's established test-asset location
- Modify: CI workflow(s) for a GStreamer-enabled Linux job

**Automated matrix:**

1. File only.
2. `videotestsrc` standing in for capture.
3. Two simultaneous tracks and PiP.
4. Seek before/inside/after a live clip.
5. Pause/resume and Stop reset.
6. Full-song loop and section loop.
7. Missing/busy capture input.
8. Preview disconnect/back-pressure.
9. 100 play/stop cycles with no growing thread, fd or device-handle count.
10. Audio-only regression build without video feature.

**Real hardware test:**

- USB HDMI capture card at target resolution/frame rate.
- 60-minute run with repeated seeks and camera interval entries/exits.
- Record first-frame latency, drift against an embedded timecode/burn-in test pattern, dropped frames, CPU/GPU load and recovery after unplug/replug.
- Perform on the intended Linux graphics stack and physical output, not only CI.

**Acceptance thresholds to agree before implementation:**

- No A/V timeline error above one output frame after settling.
- No stale frame from a prior seek/play generation.
- Program output does not stall when preview is slow.
- Capture source can be reused after every seek/stop cycle.
- No audio underrun regression attributable to video under target show load.

**Commit:** `test(video): cover synchronized timeline playback end to end`

### Task 13: Package and document the feature

**Objective:** Make installation and operation reproducible on supported Linux systems.

**Files:**
- Modify: `README.md`
- Modify: `docs/src/SUMMARY.md`
- Create: `docs/src/video/overview.md`
- Create: `docs/src/video/timeline.md`
- Create: `docs/src/video/capture.md`
- Create: `docs/src/video/output.md`
- Modify: release/package workflows

**Document:**

- supported codecs are whatever the installed GStreamer plugin set provides, with tested baseline codecs listed explicitly;
- required Debian/Ubuntu packages;
- V4L2 permissions/udev guidance;
- capture identity and reassignment;
- output/display selection;
- preview vs program semantics;
- exact pause/stop/seek/loop behavior;
- troubleshooting commands and status fields;
- tested hardware matrix and known limitations.

**Commit:** `docs(video): document Linux timeline and capture workflow`

---

## 6. Delivery phases

### Phase A — technical proof

Tasks 0–3. Outcome: validated media stack, schema and build strategy; no promise of a polished editor yet.

### Phase B — first end-to-end vertical slice

Tasks 4–8 with a minimal YAML-authored show. Outcome: background file + timed capture PiP renders fullscreen and follows transport, status and seek correctly.

### Phase C — usable authoring

Tasks 9–11. Outcome: operators can build and manipulate the timeline in the existing browser UI without hand-editing YAML.

### Phase D — production hardening

Tasks 12–13. Outcome: hardware soak results, CI coverage, packaging and operating documentation. Only after this phase should the current Millumin show be considered migratable.

### Phase E — external timecode follower (later roadmap point 10)

Add LTC-from-audio-input and MTC-from-MIDI-input as alternative transport sources. Outcome: M-Track can chase an external show clock and run video/MIDI/DMX without using M-Track for audio playback. This phase starts only after internal-clock video transport, seek and loop correctness are stable.

---

## 7. Risks and mitigations

### GStreamer/system dependency weight

**Risk:** M-Track currently has lightweight audio dependencies; video pulls in a large native stack.

**Mitigation:** Cargo feature, separate video-enabled packages/assets, startup plugin diagnostics, audio-only CI lane.

### Video work disturbs real-time audio

**Risk:** Decode/composite work or preview encoding causes CPAL underruns.

**Mitigation:** dedicated media thread, GPU conversion/composition where available, bounded/leaky preview queues, health counters, stress tests with the production buffer size.

### `/dev/videoN` instability and capture exclusivity

**Risk:** devices reorder or remain busy after seek.

**Mitigation:** udev identity matching, ambiguity errors, generation-tagged teardown, repeated reopen test, explicit busy state.

### Browser editor and actual output diverge

**Risk:** CSS preview looks right but program compositor differs.

**Mitigation:** preview must be a branch of the actual composed frame, not a browser reimplementation. Browser overlays only show selection handles.

### Loop semantics are harder than linear playback

**Risk:** capture/file source state is wrong when a section loops without a full Player restart.

**Mitigation:** treat section-loop validation as a V1 gate; reuse `LoopControl`; test loop entry, repeated iterations and break-out.

### Program output selection differs between Wayland, X11 and headless rigs

**Risk:** "fullscreen on display X" is backend-specific.

**Mitigation:** start with one documented desktop/session path selected during Task 0; make sink construction an abstraction; add DRM/KMS only after the main path is stable.

### Millumin feature creep

**Risk:** effects, mapping, NDI and multi-output prevent the core timeline from shipping.

**Mitigation:** enforce the explicit V1 exclusions. PiP is transforms + alpha over; do not build a generic node/effect graph.

---

## 8. Pull-request strategy

Keep reviewable PRs aligned with tasks rather than one giant video branch:

1. RFC/spike (may be draft and later partly removed).
2. Config/runtime model with no GStreamer dependency in default build.
3. GStreamer backend and discovery.
4. Player synchronization and status.
5. Program output/preview and REST support.
6. Timeline UI.
7. PiP canvas.
8. Harness, hardware evidence, packaging and docs.

Every PR must preserve `cargo test --workspace` and the Svelte checks. Any PR touching transport must include stop, pause, seek and section-loop tests, not only happy-path Play.

---

## 9. Verification commands

```bash
# Rust formatting, lint and tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --no-default-features

# Web UI
cd src/webui/svelte
npm ci
npm run check
npm run lint
npm run test:mock
npm run build

# Project-level gates
cd ../../..
make test
make lint
make docs
```

The final release also requires the real capture-card/second-display soak described in Task 12; green unit tests alone are not production acceptance.

---

## 10. Recommended first implementation milestone

Do **not** start by recreating all of Millumin. The first milestone should be:

> One 1920×1080/50 canvas, one file track, one V4L2 capture track, bounded live clip timing, draggable PiP geometry, per-clip fades, one fullscreen output, one low-rate editor preview, and correct play/pause/stop/seek behavior under M-Track's existing master clock.

That is the smallest slice that proves the requested value and the hardest engineering assumptions at the same time.