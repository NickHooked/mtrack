# Live video performance

Build the web UI with `npm run build` in `src/webui/svelte`, then build with
`cargo build --release --features video` and start Mtrack with the web interface.
Release binaries embed the built web UI, so this order matters.
Use a release build for performance: unoptimized pixel transforms and JPEG
encoding are much slower. `examples/songs/vj-demo/song.yaml` is a complete,
self-contained composition using simulated camera sources.

Open a song's **Video → Live / VJ** workspace. The VJ transport runs independently
of the song transport. Stop normal song playback before starting VJ output.
Switching back to the timeline, leaving the page or losing the controller's
connection stops the session and clears its outputs. A reconnect does not
silently restart a show.

## Deck

Rows use the song's video tracks; columns are simultaneous scene triggers.
Existing timeline clips become cells. Drag media, a live source or a compound
onto an empty cell. Drop on **New layer** to create a track. Move cells within a
row by dragging. Right-click a cell for its trigger and playback settings.
Deck ordering is separate from timeline time: launching a clip starts at its
source-in point, irrespective of its start on the timeline.

- Click a cell to launch it, or a column header to launch that column's clips.
  Empty cells leave their layer playing.
- Restart, toggle and piano/hold modes; looping, one-shot and auto-next.
- Immediate, beat or bar quantization, BPM and tap tempo.
- Layer opacity, mute, solo, normal/add/screen/multiply blend modes, brightness,
  contrast, saturation and invert.
- Assign layers to A or B for the crossfader. Both are fully visible at centre;
  unassigned layers are unaffected.
- Master opacity, global freeze and blackout apply to all mapped outputs.
- Keys 1–9 and 0 launch the first ten columns; B toggles blackout, F toggles
  freeze, Escape stops the layers. Shortcuts do not intercept text inputs.
- Clip masks, transforms and existing clip automation are evaluated during
  performance. Repeated camera uses share one capture pipeline per device.

**Save** stores the deck, output mappings and MIDI bindings in the song's YAML.
Live trigger positions and temporary fader moves are not saved as a performance.
Media/track changes require stopping output; output mapping, effects and MIDI
bindings can be edited live. Timeline edits reconcile deck cells and layer
routing so removing a track cannot redirect a projector to an unrelated layer.

## Output canvases and mapping

Create up to eight independently sized output canvases. Each has up to sixteen
slices, which can read either the complete composition or a single layer after
its fader/effects. An input crop selects a normalized source rectangle. Four
corner handles apply perspective-correct projection mapping; arrow keys nudge
one pixel and Shift+arrows ten. Folded and degenerate quads are rejected.

Slice opacity and independent left/top/right/bottom feather widths create soft
edges. Output calibration includes opacity, brightness, gamma, X/Y flips and a
test card. Prepare mappings offline; the preview updates once output is running.

**Open output** creates a clean browser output window. Move it onto the desired
monitor/projector, then click **Fullscreen output**. Match the output canvas
resolution to the display; fullscreen fills that display. The composition has
its own unmapped output window as well. Output viewers cannot control the deck.
If a viewer disconnects it reconnects automatically; loss of the controlling
VJ session clears the output to black.

The current transport streams full-resolution JPEG frames to browser outputs.
This is useful for monitor/projector operation but is not a frame-locked,
lossless output system. It does not implement DeckLink playback, NDI, Spout,
Syphon, projector discovery or automatic display assignment. Physical outputs
and capture hardware still require testing on the intended rig. CPU rendering
and encoding cost scale with resolution, layers and slices; use the live FPS
indicator to size the show. Compositions support 1–60 fps, 4096-pixel output
edges, 16 megapixels across output canvases and 64 megapixels of layer frames.
The development machine reached roughly 20–21 fps with two mapped 640×360
canvases, a two-camera test-pattern compound and a masked live layer. This was
below the configured 25 fps; higher resolutions and file decoding can reduce
throughput further. This is a functional first version, not a guaranteed
real-time multi-projector performance target.

## MIDI

Enable **MIDI learn**, select a supported clip, column, fader or button, and move
its MIDI control. Bindings store input, channel, note/CC, target, range and
absolute/toggle/momentary behavior. Continuous CC values normalize 0–127 to the
configured range. Note-off and note-on with zero velocity release momentary and
piano controls. More than one binding may target a control.

Host input uses the MIDI device/controller already configured in Mtrack, sharing
its existing event watcher. Browser input uses **Connect browser MIDI** and
requires browser Web MIDI support and permission. The input monitor shows which
side received the event. Avoid overlapping bindings with the player's transport
notes; starting normal song playback stops the VJ session. MIDI clock sync,
feedback LEDs, relative encoders and recording a live performance are not yet
implemented.

## Runtime checks

The backend validates configuration and resolves/binds file media inside the
song directory before rendering, including nested compounds. One process-wide
permit excludes competing editor/VJ capture sessions and remains held until
the native worker actually exits. Socket queues and output buffering are bounded;
slow viewers receive the latest frame rather than a backlog. Output frames are
shared across viewers rather than opening additional cameras.
