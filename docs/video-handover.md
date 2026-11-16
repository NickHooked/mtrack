# Video Timeline Feature Handover

This document describes the current state of the experimental Linux video timeline work and is intended to help Michael continue development without having to reconstruct the earlier decisions.

## Upstream refresh — 2026-11-16

This branch is integrated with upstream `mdwn/mtrack` main at
`5c6dc6dcda` (227 upstream commits since the original video baseline).
The previous video branch tip is preserved locally as
`backup/video-before-upstream-2026-11-16` (`43bd54e`).

A search of the 36 public upstream branches, commit messages, issues and
pull requests found no GStreamer/video timeline implementation or public
reference to this fork. This does not establish whether the maintainer has
unpublished work. Upstream issue #612 discusses audio/DMX/MIDI delay and
mentions video sync as a possible use case, but contains no video implementation.

Integration changes:

- The video configuration and safe media timeline resolver now live in
  `crates/mtrack-core`; the GStreamer engine stays optional in the player.
- Song loading, duration reporting and media inventory retain video support.
  MIDI EndOfTrack duration and SMPTE timing from this branch are retained
  alongside upstream's shared MIDI tempo model.
- The editor uses upstream's gRPC song APIs. Its separate preview WebSocket
  remains `/api/songs/{name}/video-preview/ws`; build capabilities are read
  from `/api/video/capabilities`, since upstream removed the old REST status API.
- Browser tests use the new gRPC mocks. The development proxy, video CI lane
  and release workflow include the dependencies introduced upstream.
- Live Player video integration, capture, independent program output and the
  other incomplete features below remain incomplete. This update is a port,
  not completion of the live-video feature set.

Build with `make build-video`. The current upstream frontend additionally
requires the pinned wasm-pack, wasm-bindgen and wasm-opt tools described in
`setup.sh` and `scripts/install-binaryen.sh`.

Local validation for the refresh: frontend type checking, production build,
targeted ESLint, all 33 video browser tests, 26 core video tests, 23 MIDI
playback tests, 140 song tests, 69 backend video tests (run serially) and all
24 GStreamer integration tests passed.
The updated app was also exercised with the two-clip demo: preview start,
play, pause and seek produced frames. Packaging metadata checks passed;
the full Debian package and the full repository test suite were not run.

## Media pool and direct timeline editing — 2026-11-16

The video editor now has a song-scoped media pool alongside a compact preview.
The layout follows the media pool/viewer/timeline pattern described in the
[DaVinci Resolve Edit overview](https://www.blackmagicdesign.com/products/davinciresolve/edit/).

- Import multiple videos by file picker or dropping files into the pool. This
  reuses the song upload/claim API and the existing filename/path validation.
  The client skips duplicate names, rather than replacing existing media.
- Drag a pool item onto a lane at a chosen time, or onto the new-track area to
  create a track automatically. Clicking a pool item appends to the selected
  clip's track, or the first track. Manual placement remains in a disclosure.
- Move clips between tracks, trim their edges, snap to clip boundaries, and
  bypass snapping with Shift. Overlaps on one track are rejected.
- Undo/redo keeps the last 50 local timeline edits; Ctrl/Cmd+Z and Shift+Z
  work outside text fields. File uploads are not undone, and history resets
  when leaving the editor. Changes still use the song's Save button.
- Click the timeline ruler to seek the preview; the playhead follows playback.
  Import and editing honor the player's lock.
- Composition settings (position, size, opacity and fades) now survive parsing,
  moving and saving. The earlier property-dropping limitation is fixed.

The pool lists videos stored with this song, including referenced nested files.
It is not a project-wide asset database. Browser-readable imports supply their
source duration for this editing session; otherwise new clips use the manual
placement duration (10 seconds initially). Source metadata for existing files,
thumbnail generation, ripple/roll edits and transitions remain future work.

Validation: 39 video browser tests, frontend type checking and targeted lint,
production frontend and video backend builds, and backend tests covering video
upload filename confinement and unchanged song config after import.

## Repository and branch

The work is published in Nick Haakman's fork:

- Fork: <https://github.com/NickHooked/mtrack>
- Branch: [`feature/video-timeline-files`](https://github.com/NickHooked/mtrack/tree/feature/video-timeline-files)
- Current handover baseline before this document: `39dd6df`
- Upstream baseline used when the original implementation plan was written: `c59ce4031c07269808eda49fa99033d1c06a5846`

To check it out from an existing upstream clone:

```bash
git remote add nick https://github.com/NickHooked/mtrack.git
git fetch nick
git switch --track nick/feature/video-timeline-files
```

## Goal

The intended end state is a Linux-native video timeline inside mTrack, covering the most important parts of the current Millumin workflow:

- multiple ordered video tracks on a song timeline;
- file clips placed at exact song times;
- live UVC/V4L2 USB capture sources;
- opacity and fades;
- crop, rotation, fit/fill and direct-manipulation picture-in-picture;
- a low-rate editor preview of the current unsaved draft;
- a separate fullscreen program output;
- synchronized play, pause, stop, seek and loop behavior alongside audio, MIDI and lighting.

For V1, mTrack's existing audio-derived playback clock remains authoritative. Video follows that transport; video must not delay or resample audio. External LTC/MTC following is deliberately deferred.

The full intended V1 scope and architecture are documented in:

- [`docs/plans/2026-09-22-video-timeline.md`](plans/2026-09-22-video-timeline.md)
- [`docs/plans/2026-09-23-video-editor-preview.md`](plans/2026-09-23-video-editor-preview.md)

## What is implemented

### Song configuration and timeline model

The branch adds a separate optional `video:` section to the song configuration rather than overloading the existing audio track model.

Implemented behavior includes:

- fixed canvas dimensions and frame rate;
- ordered video tracks and clips;
- stable clip IDs;
- file sources with source-in offsets;
- start time and duration;
- clip layout data;
- opacity and linear fade-in/fade-out;
- validation of ranges, overlaps and resource limits;
- safe media resolution beneath the song directory;
- video clip ends contributing to the resolved song duration.

Important files:

- `src/config/video.rs`
- `src/video/timeline.rs`
- `src/songs.rs`

### Timeline editor

The Svelte song editor contains a multitrack video timeline with:

- adding and removing tracks and clips;
- pointer-based clip movement and trimming;
- keyboard movement, trimming and deletion;
- overlap validation;
- stable normalization of missing or duplicate clip IDs;
- unified song save/reload behavior;
- dirty-state preservation when switching tabs;
- clip property editing.

Important files:

- `src/webui/svelte/src/components/video/VideoTimelineEditor.svelte`
- `src/webui/svelte/src/components/video/VideoTrackLane.svelte`
- `src/webui/svelte/src/components/video/VideoClipBlock.svelte`
- `src/webui/svelte/src/components/video/VideoPropertiesPanel.svelte`
- `src/webui/svelte/src/lib/video/timeline-state.ts`

### Optional GStreamer video engine

Video support is behind the Cargo feature `video`, so the normal audio-only build does not require GStreamer.

The current engine work includes:

- GStreamer startup and plugin diagnostics;
- file-video decoding;
- transparent RGBA canvas composition;
- ordered-track z-order;
- source timing and source-in handling;
- per-frame opacity and fade envelopes;
- positioned and scaled clips;
- bounded startup, cancellation and teardown behavior;
- playback generations to prevent stale workers or frames;
- synchronized preview and program-output modes;
- latest-frame semantics for preview;
- complete ordered-frame delivery for program output;
- media descriptor rebinding to reduce path-race exposure.

Important files:

- `src/video/engine.rs`
- `src/video/gstreamer.rs`
- `src/video/media_binding.rs`
- `tests/video_engine.rs`

### Editor preview

The Video tab can preview the real GStreamer-composited result rather than reconstructing the composition in the browser.

The preview implementation includes:

- a dedicated binary WebSocket at `/api/songs/{name}/video-preview/ws`;
- the current unsaved draft `VideoConfig` as preview input;
- start/reload, play, pause, seek and stop controls;
- one editor preview session at a time;
- epoch and request correlation to reject stale responses and frames;
- a binary `MTRK` frame header;
- JPEG frames at a bounded size and rate;
- latest-only delivery so a slow browser does not back-pressure the engine;
- explicit busy and disabled responses;
- cleanup when leaving the Video tab or switching songs;
- fullscreen preview and responsive/mobile controls;
- capability reporting for builds without the video feature.

Important files:

- `src/webui/video_preview.rs`
- `src/webui/svelte/src/components/video/VideoPreview.svelte`
- `src/webui/svelte/src/lib/video/preview.ts`

A UI screenshot is stored at:

- `artifacts/mtrack-video-preview.png`

## Build and packaging work

The branch also contains initial build and packaging support:

- `./setup.sh --video` selects the required GStreamer development/runtime packages;
- `cargo build --features video` builds the optional backend;
- the normal package remains free of GStreamer dependencies;
- a separate `mtrack-video` Debian variant declares its GStreamer dependencies and conflicts/replaces/provides `mtrack`;
- CI has a separate video build, Clippy and test lane;
- release workflow changes add separate Linux video assets;
- `tests/packaging/metadata.sh` validates setup selection, CI lane separation and base/video package metadata without installing packages.

This packaging/release work should be reviewed carefully before merging. It was the final area being worked on when development paused.

## Verification performed before handover

The focused feature checks were green at handover:

- 14 video configuration tests;
- 6 runtime timeline tests;
- 50 video-feature unit tests;
- 24 GStreamer engine integration tests;
- 33 focused Playwright timeline/preview tests;
- Svelte type checking, ESLint and Prettier checks;
- non-destructive base/video packaging metadata checks;
- `git diff --check`.

Useful focused commands:

```bash
cargo test --no-default-features config::video
cargo test --no-default-features video::timeline
cargo test --features video video::
cargo test --features video --test video_engine
make test-packaging-metadata

cd src/webui/svelte
npm run check
npm run lint
npm run format:check
npx playwright test --project=mock \
  e2e/tests/video-timeline.spec.ts \
  e2e/tests/video-preview-client.spec.ts \
  --workers=1
```

The repository-wide test and Clippy suites were **not** fully green in the handover environment. The observed failures were outside the focused video tests and included existing root-permission-sensitive tests, MCP integration flakiness, an example-song fixture mismatch, and warnings raised by the newer local Clippy version in unrelated code. These should not be interpreted as a claim that the complete branch is CI-clean; GitHub CI remains authoritative.

The local environment also warned that the system OpenSSL development package was unavailable, although this did not block the focused video checks.

## Important architectural invariants

Please preserve these unless there is a deliberate redesign:

1. **Audio remains the master clock.** Video follows the existing mTrack transport.
2. **GStreamer stays out of the real-time audio callback.** Media work belongs on dedicated workers/threads.
3. **Video remains optional.** Audio-only builds and packages should not acquire mandatory GStreamer dependencies.
4. **The browser preview is not the program output.** A slow or disconnected browser must never affect fullscreen output.
5. **Preview is bounded and lossy; program output is not.** Preview may drop old frames, while program output preserves ordered frames.
6. **The unsaved editor draft drives preview.** Users should not have to save merely to inspect an edit.
7. **All media paths remain constrained to the song directory.** Avoid reopening user-controlled paths after validation where possible.
8. **Worker teardown must be bounded.** Start, seek, stop, disconnect and drop must not leave blocked GStreamer workers behind.
9. **Playback generations/epochs reject stale work.** Old frames and responses must never appear after restart, seek or reload.
10. **Do not silently expand V1 into all of Millumin.** Projection mapping, shaders, keyframes, NDI/DeckLink, multi-output and external timecode are later work.

## Known incomplete areas

The branch is a substantial prototype, not a finished V1. Major remaining work includes:

- V4L2/UVC discovery and stable hardware-profile bindings;
- capture-source warm-up and lifecycle management;
- full integration with `Player`, its readiness barrier and the shared playback clock;
- coordinated seek, song change, full-song loop and section-loop behavior;
- production fullscreen output and display selection;
- safe media import and probing APIs;
- complete crop, rotation, fit/fill and direct-manipulation PiP behavior;
- video health, status and operator preflight;
- hardware validation with real USB capture devices and a program display;
- lifecycle stress testing and production documentation;
- final review of CI, Debian packaging and release workflow changes.

The existing preview engine should not be mistaken for complete live Player integration. It proves a large part of the file-video composition and editor-preview path, but the synchronized live-show path still needs to be completed.

## Suggested continuation order

1. Review the complete branch diff, especially workflow and packaging changes.
2. Re-run focused checks in the maintainer's normal development/CI environment.
3. Decide whether to split the branch into smaller reviewable PRs.
4. Finish V4L2 discovery and machine-specific capture bindings.
5. Finish compositor source lifecycle for both file and capture sources.
6. Integrate video with `Player`, `PlaybackSync`, seek and loop semantics.
7. Implement and validate the independent fullscreen program output.
8. Complete media APIs and direct-manipulation PiP.
9. Add health/preflight and run the practical acceptance show from the implementation plan.

## Branch commits before this handover document

```text
2271768 docs: plan Linux video timeline
42eb7c3 [verified] feat: add song video timeline model
1e8d0ef [verified] feat: add multitrack video timeline editor
39dd6df [verified] feat(video): add GStreamer preview and packaging
```

The branch is intentionally published from Nick's fork without an upstream pull request, leaving Michael free to continue directly, reorganize the commits, or open one or more draft PRs when appropriate.
