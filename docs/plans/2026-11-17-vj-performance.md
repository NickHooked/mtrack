# VJ performance goal

Branch: `feature/vj-mapping-midi-grid`, based on video branch `7199b210`.

User approved the gauntlet cycle: implement, test, critically review, repair,
test again. No separate installed gauntlet skill is used.

## Delivery

- Persistent XY deck: tracks as layers, columns as simultaneous scenes;
  file, camera and compound sources reuse the timeline media bindings.
- Live controls: layer/master faders, mute/solo, A/B crossfader, blackout,
  freeze, BPM/quantized triggering, loop/one-shot and trigger modes.
- Multiple independently sized output canvases with composition/layer routing,
  slices, input crops, perspective corner mapping and edge blending.
- MIDI learn and saved note/CC mappings through host MIDI and browser MIDI.
- Actual program rendering, preview/output windows, bounded resource lifetime,
  saved configuration, useful failures, focused tests and demo screenshots.

## Research

Official references: [clips](https://resolume.com/support/en/clips),
[MIDI shortcuts](https://resolume.com/support/en/midi-shortcuts),
[screens](https://resolume.com/support/en/screens),
[input selection](https://resolume.com/support/en/input-selection),
[output transformation](https://www.resolume.com/support/en/output-transformation),
[edge blending](https://resolume.com/support/en/edge-blending).

## Gauntlet log

1. Architecture inspection: existing GStreamer decoders, shared capture cache,
   compound renderer, confined media binding and preview exclusion can be reused.
   Live performance will run independently of the song transport, with explicit
   session start/stop and shared outputs; the editor remains a separate mode.
2. In progress: validated data model, deterministic performance controls and
   projection rasterizer before wiring the live renderer and UI.
3. First implementation: typed/persisted deck, deterministic triggers/MIDI,
   CPU perspective compositor, shared native decoders, single-owner live session,
   multiple browser output windows and an inspector/grid UI.
4. Gauntlet pass 1: 46 core video tests, 78 backend video tests and 51 browser
   tests passed. Browser tests initially caught Svelte proxy cloning failures;
   replaced raw structuredClone with serialization of validated configuration.
   Read-through found a reactive image subscription that could reconnect every
   frame; excluded image URL bookkeeping from effect dependencies.
5. Gauntlet review: preserve MIDI faders across mapping updates; reconcile routes
   by track identity after timeline deletion/reordering; validate canvas budgets
   and reserved output IDs. Added tests for routing reconciliation. The actual
   debug demo rendered both mapped outputs but only ~1–2 fps; building release
   and adding fast paths for neutral compositing/unwarped outputs before final
   performance validation.
6. Gauntlet pass 2: 48 core video tests, 78 backend video tests and 31 native
   video integration tests passed; the build without the video feature also
   passed. Five frontend persistence tests cover route removal/remapping,
   cleared cells, minimal YAML defaults and undo restoration. Five VJ browser
   journeys cover output persistence, MIDI learn/faders, drag/drop, piano key
   releases and reconnect/song navigation. Existing video browser coverage
   passed in the earlier 51-test run. Type checking reports no errors/warnings;
   changed-file lint and formatting checks pass.
7. Visual/runtime review: fixed the numeric quantization dropdown's blank
   selection and moved source placement help to tooltips. Piano key releases
   and reconnect are now explicitly tested. Release rendering reached about
   13 fps for two warped 640×360 outputs plus a compound and masked live layer;
   independent output mapping/JPEG encoding now uses a bounded four-thread
   pool. Native worker tests passed again after that optimization.
8. File timing review found that checking only successive requested positions
   missed accumulated decoder drift when rendering was moderately slower than
   the requested frame rate. Track decoded media time separately and re-seek
   when it falls behind. A real lossless red/blue video fixture now verifies
   the correct frame after a sequence of small overruns; all 79 backend video
   tests pass with this regression included.
9. Final native integration run: 31/31 passed. Release build completed and the
   real browser demo rendered two 640×360 warped/feathered outputs with a dual
   test-camera compound plus a masked live layer at roughly 20–21 fps on this
   host (configured 25 fps). Confirmed recorded-video launching, isolated-layer
   output and blackout clearing both independent viewer windows. Hardware was
   not attached. During screenshot review, SVG frame replacement visibly
   flickered; use a separate HTML image under the mapping handles so the browser
   retains the previous decoded frame while loading the next.
