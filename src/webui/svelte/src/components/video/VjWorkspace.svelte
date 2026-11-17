<script lang="ts">
  function copyData<T>(value: T): T {
    return JSON.parse(JSON.stringify(value));
  }
  import { onMount, onDestroy } from "svelte";
  import type { VideoConfig, VideoClip } from "../../lib/video/types";
  import {
    deckConfig,
    defaultClip,
    newOutput,
    targetLabel,
    type VjConfig,
    type ControlTarget,
    type MidiInput,
    type LayerSettings,
  } from "../../lib/video/vj";
  import { VjClient, type VjStatus } from "../../lib/video/vj-client";
  import {
    VIDEO_MEDIA_TYPE,
    parseLiveMedia,
    liveMedia,
    videoSourceLabel,
  } from "../../lib/video/media";
  import {
    uniqueClipId,
    videoConfigError,
  } from "../../lib/video/timeline-state";
  import VideoMediaPool from "./VideoMediaPool.svelte";
  import VideoLiveSources from "./VideoLiveSources.svelte";
  import VjOutputEditor from "./VjOutputEditor.svelte";
  import VjMidiPanel from "./VjMidiPanel.svelte";
  let {
    video,
    songName,
    files,
    locked = false,
    importing = false,
    onimport,
    onchange,
  }: {
    video: VideoConfig;
    songName: string;
    files: string[];
    locked?: boolean;
    importing?: boolean;
    onimport: (files: File[]) => void;
    onchange: (video: VideoConfig) => void;
  } = $props();
  let deck = $derived(deckConfig(video));
  let client: VjClient | null = null;
  let connected = $state(false);
  let status = $state<VjStatus>({ running: false, starting: false });
  let frame = $state<string | null>(null);
  let error = $state("");
  let selected = $state({ layer: 0, column: 0 });
  let tab = $state<"outputs" | "clip" | "layer" | "midi">("outputs");
  let library = $state<"media" | "live" | "compounds">("media");
  let learn = $state(false);
  let target = $state<ControlTarget | null>(null);
  let lastInput = $state<{ device: string; input: MidiInput } | null>(null);
  let settingsTimer: ReturnType<typeof setTimeout> | undefined;
  let busy = $derived(status.running || status.starting);
  let editLocked = $derived(locked || busy);
  let activeState = $derived(status.state);
  let selectedId = $derived(deck.grid[selected.layer]?.[selected.column]);
  let clip = $derived(
    video.tracks[selected.layer]?.clips.find((c) => c.id === selectedId),
  );
  let clipSettings = $derived(
    clip ? (deck.clips[clip.id] ?? defaultClip()) : defaultClip(),
  );
  let layer = $derived(
    activeState?.layers[selected.layer]?.settings ??
      deck.layers[selected.layer],
  );
  function connect() {
    client?.destroy();
    connected = false;
    status = { running: false, starting: false };
    client = new VjClient(songName, {
      connection: (v) => (connected = v),
      status: (v) => {
        if (v.error && v.error !== status.error) error = v.error;
        status = v;
      },
      frame: (v) => (frame = v),
      error: (v) => (error = v),
      midi: receiveMidi,
    });
  }
  onMount(() => {
    connect();
    return () => client?.destroy();
  });
  onDestroy(() => clearTimeout(settingsTimer));
  function save(next: VjConfig) {
    if (locked) return;
    if (next.bpm === deck.bpm && activeState?.bpm)
      next = { ...next, bpm: activeState.bpm };
    const problem = videoConfigError({ ...video, vj: next });
    if (problem) {
      error = problem;
      return;
    }
    onchange({ ...video, vj: next });
    if (status.running) {
      clearTimeout(settingsTimer);
      settingsTimer = setTimeout(
        () => client?.send({ type: "settings", settings: next }),
        80,
      );
    }
  }
  function changeDeck(patch: Partial<VjConfig>) {
    save({ ...deck, ...patch });
  }
  function start() {
    error = "";
    const settings = copyData(deck);
    if (!settings.outputs.length)
      settings.outputs = [
        newOutput(1, video.canvas.width, video.canvas.height),
      ];
    const config = { ...video, vj: settings };
    const problem = videoConfigError(config);
    if (problem) {
      error = problem;
      return;
    }
    if (!locked && JSON.stringify(config.vj) !== JSON.stringify(video.vj))
      onchange(config);
    if (client?.send({ type: "start", config }))
      status = { running: false, starting: true };
  }
  function control(controlTarget: ControlTarget, value = 1) {
    if (learn) {
      target = controlTarget;
      tab = "midi";
      return;
    }
    if (!status.running) return;
    client?.send({ type: "control", event: { target: controlTarget, value } });
  }
  function release(layer: number, column: number) {
    if (
      !learn &&
      deck.clips[deck.grid[layer]?.[column] ?? ""]?.trigger === "piano"
    )
      control({ type: "launch", layer, column }, 0);
  }
  function receiveMidi(device: string, input: MidiInput) {
    lastInput = { device, input };
    if (!target || locked || input.value === 0) return;
    const continuous = [
      "layer_opacity",
      "master_opacity",
      "crossfader",
    ].includes(target.type);
    const mode = continuous
      ? "absolute"
      : target.type === "launch" ||
          target.type === "column" ||
          target.type === "stop_layer" ||
          target.type === "stop_all" ||
          target.type === "tap_tempo"
        ? "momentary"
        : "toggle";
    changeDeck({
      midi: [
        ...deck.midi,
        {
          id: crypto.randomUUID(),
          device,
          channel: input.channel,
          kind: input.kind,
          number: input.number,
          target,
          mode,
          min: 0,
          max: 1,
        },
      ],
    });
    target = null;
    learn = false;
  }
  function browserMidi(device: string, data: number[]) {
    const type = data[0] & 0xf0;
    if (data.length !== 3 || ![0x80, 0x90, 0xb0].includes(type)) return;
    const wasLearning = !!target;
    receiveMidi(device, {
      kind: type === 0xb0 ? "cc" : "note",
      channel: data[0] & 15,
      number: data[1],
      value: type === 0x80 ? 0 : data[2],
    });
    if (!wasLearning) client?.send({ type: "midi", device, data });
  }
  function addMedia(
    media: string,
    layerIndex = selected.layer,
    column = selected.column,
  ) {
    if (editLocked) return;
    const source = parseLiveMedia(media);
    const next = copyData(video);
    if (layerIndex >= next.tracks.length)
      next.tracks.push({ name: `Layer ${next.tracks.length + 1}`, clips: [] });
    const track = next.tracks[layerIndex];
    const start = Math.max(0, ...track.clips.map((c) => c.start + c.duration));
    const id = uniqueClipId(next, source ? "live" : media);
    const newClip: VideoClip = {
      id,
      start,
      duration: 10,
      source_in: 0,
      file: source ? "" : media,
      ...(source ? { source } : {}),
    };
    track.clips.push(newClip);
    const settings = deckConfig(next);
    while (settings.grid.length < next.tracks.length)
      settings.grid.push(Array(settings.columns).fill(null));
    settings.grid[layerIndex][column] = id;
    next.vj = settings;
    onchange(next);
    selected = { layer: layerIndex, column };
    tab = "clip";
  }
  function drop(event: DragEvent, layerIndex: number, column: number) {
    event.preventDefault();
    if (editLocked) return;
    const media = event.dataTransfer?.getData(VIDEO_MEDIA_TYPE);
    if (media) {
      addMedia(media, layerIndex, column);
      return;
    }
    const raw = event.dataTransfer?.getData("application/x-mtrack-vj-cell");
    if (!raw) return;
    try {
      const from = JSON.parse(raw) as { layer: number; column: number };
      if (from.layer === layerIndex) {
        const grid = copyData(deck.grid);
        [grid[layerIndex][column], grid[from.layer][from.column]] = [
          grid[from.layer][from.column],
          grid[layerIndex][column],
        ];
        changeDeck({ grid });
      } else {
        error =
          "Move cells within a layer; drag sources from the library to another layer.";
      }
    } catch {
      error = "Invalid clip drag.";
    }
  }
  function patchClip(patch: Partial<VideoClip>) {
    if (!clip || editLocked) return;
    const next = copyData(video);
    const current = next.tracks[selected.layer].clips.find(
      (c) => c.id === clip!.id,
    )!;
    Object.assign(current, patch);
    const problem = videoConfigError(next);
    if (problem) {
      error = problem;
      return;
    }
    onchange(next);
  }
  function patchLayer(patch: Partial<LayerSettings>) {
    const layers = deck.layers.map((l, i) =>
      i === selected.layer ? { ...l, ...patch } : l,
    );
    changeDeck({ layers });
  }
  function keyboard(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (
      event.target instanceof HTMLElement &&
      event.target.closest("input,select,textarea,[contenteditable=true]")
    )
      return;
    if (event.repeat) return;
    const column = "1234567890".indexOf(event.key);
    if (column >= 0 && column < deck.columns) {
      event.preventDefault();
      control({ type: "column", column });
    } else if (event.key.toLowerCase() === "b") {
      control({ type: "blackout" }, activeState?.blackout ? 0 : 1);
    } else if (event.key.toLowerCase() === "f") {
      control({ type: "freeze" }, activeState?.frozen ? 0 : 1);
    } else if (event.key === "Escape") {
      control({ type: "stop_all" });
    }
  }
</script>

<svelte:window
  onkeydown={keyboard}
  onkeyup={(e) => {
    if (
      e.target instanceof HTMLElement &&
      e.target.closest("input,select,textarea,[contenteditable=true]")
    )
      return;
    const column = "1234567890".indexOf(e.key);
    if (column >= 0 && column < deck.columns)
      control({ type: "column", column }, 0);
  }}
/>
<section class="vj-workspace" data-testid="vj-workspace">
  <div class="toolbar">
    <div class="live-label">
      <span class:live={status.running}>●</span><strong>PERFORM</strong><small
        >{status.running
          ? `${(status.fps ?? 0).toFixed(1)} fps`
          : status.starting
            ? "Starting…"
            : "Standby"}</small
      >
    </div>
    {#if !connected}<button
        class="btn"
        onclick={connect}
        title="Reconnect without losing the draft. Output stays stopped until started explicitly."
        >Reconnect</button
      >{/if}
    {#if busy}<button
        class="btn stop"
        onclick={() => client?.send({ type: "stop" })}>■ Stop output</button
      >{:else}<button class="btn start" disabled={!connected} onclick={start}
        >▶ Start VJ output</button
      >{/if}
    <label class="tempo"
      >BPM<input
        aria-label="VJ tempo"
        type="number"
        min="20"
        max="300"
        value={activeState?.bpm ?? deck.bpm}
        disabled={locked}
        onchange={(e) =>
          changeDeck({
            bpm: Math.max(20, Math.min(300, +e.currentTarget.value)),
          })}
      /></label
    ><button
      class="btn"
      title="Tap quarter notes to set tempo"
      onclick={() => control({ type: "tap_tempo" })}>Tap</button
    >
    <select
      aria-label="Launch quantization"
      value={deck.quantize}
      disabled={locked}
      onchange={(e) => changeDeck({ quantize: +e.currentTarget.value })}
      ><option value={0}>Immediate</option><option value={0.25}>¼ beat</option
      ><option value={0.5}>½ beat</option><option value={1}>Beat</option><option
        value={2}>2 beats</option
      ><option value={4}>Bar</option><option value={8}>2 bars</option></select
    >
    <button
      class="btn"
      class:armed={learn}
      disabled={locked}
      title="Select a clip, fader or button, then move your MIDI controller"
      onclick={() => {
        learn = !learn;
        target = null;
        tab = "midi";
      }}>MIDI learn</button
    >
    <button
      class="btn"
      class:armed={activeState?.frozen}
      title="Freeze the live composition and playheads · F"
      onclick={() => control({ type: "freeze" }, activeState?.frozen ? 0 : 1)}
      >❚❚ Freeze</button
    >
    <button
      class="btn blackout"
      class:armed={activeState?.blackout}
      title="Black out every output · B"
      onclick={() =>
        control({ type: "blackout" }, activeState?.blackout ? 0 : 1)}
      >Blackout</button
    >
  </div>
  {#if error}<div class="error" role="alert">
      {error}<button title="Dismiss error" onclick={() => (error = "")}
        >×</button
      >
    </div>{/if}
  {#if learn}<div class="learn-strip">
      {target
        ? `Learning: ${targetLabel(target)}`
        : "Select a clip, fader or button to learn"}<button
        onclick={() => {
          learn = false;
          target = null;
        }}>Cancel</button
      >
    </div>{/if}
  <div class="performance-layout">
    <aside class="library">
      <div class="tabs">
        {#each ["media", "live", "compounds"] as item (item)}<button
            class:active={library === item}
            onclick={() => (library = item as typeof library)}>{item}</button
          >{/each}
      </div>
      {#if library === "media"}<VideoMediaPool
          {files}
          busy={importing}
          locked={editLocked}
          placementHint="Drag to a grid cell or click to add to the selected layer"
          disabledMessage={locked
            ? "Unlock to import media"
            : "Stop output to import media"}
          {onimport}
          onadd={(media) => addMedia(media)}
        />{:else if library === "live"}<VideoLiveSources
          locked={editLocked}
          onadd={(media) => addMedia(media)}
        />{:else}<div class="compound-list">
          {#each Object.keys(video.compounds ?? {}) as id (id)}<button
              class="source"
              disabled={editLocked}
              draggable={!editLocked}
              ondragstart={(e) =>
                e.dataTransfer?.setData(
                  VIDEO_MEDIA_TYPE,
                  liveMedia({ type: "compound", id }),
                )}
              onclick={() => addMedia(liveMedia({ type: "compound", id }))}
              >◈ {id}</button
            >{:else}<span>Create compound sources in the timeline.</span>{/each}
        </div>{/if}
    </aside>
    <main class="performance-center">
      <div class="program">
        <header>
          <strong>COMPOSITION</strong><span
            >{video.canvas.width} × {video.canvas.height}<span
              class="beat"
              style:opacity={(activeState?.beat ?? 0) % 1 < 0.2 ? 1 : 0.25}
            >
              ●</span
            ></span
          ><a
            href="/video-output/composition"
            target="_blank"
            rel="noopener"
            title="Open the unmapped composition in a separate output window"
            >↗</a
          >
        </header>
        <div
          class="screen"
          style:aspect-ratio={`${video.canvas.width}/${video.canvas.height}`}
        >
          {#if frame}<img src={frame} alt="Live VJ composition" />{:else}<span
              >Live composition preview</span
            >{/if}
        </div>
      </div>
      <div class="master-controls">
        <label
          title="Master level for composition and every mapped output"
          onpointerdown={() => {
            if (learn) control({ type: "master_opacity" });
          }}
          >Master <input
            aria-label="VJ master opacity"
            type="range"
            min="0"
            max="1"
            step=".01"
            value={activeState?.master ?? 1}
            oninput={(e) =>
              control({ type: "master_opacity" }, +e.currentTarget.value)}
          /><output>{Math.round((activeState?.master ?? 1) * 100)}%</output
          ></label
        ><label
          title="Assign layers to A or B. The centre keeps both sides fully visible."
          onpointerdown={() => {
            if (learn) control({ type: "crossfader" });
          }}
          >A <input
            aria-label="VJ crossfader"
            type="range"
            min="0"
            max="1"
            step=".01"
            value={activeState?.crossfader ?? 0.5}
            oninput={(e) =>
              control({ type: "crossfader" }, +e.currentTarget.value)}
          /> B</label
        >
      </div>
      <div class="deck-scroll" class:learning={learn}>
        <div
          class="deck"
          style:grid-template-columns={`180px repeat(${deck.columns}, minmax(105px, 1fr))`}
        >
          <div class="deck-corner">
            <button
              title="Stop every layer · Escape"
              onclick={() => control({ type: "stop_all" })}>■ All</button
            ><button
              disabled={editLocked || deck.columns >= 64}
              title="Add column"
              onclick={() =>
                changeDeck({
                  columns: deck.columns + 1,
                  grid: deck.grid.map((row) => [...row, null]),
                })}>＋</button
            >
          </div>
          {#each Array.from({ length: deck.columns }, (_, i) => i) as column (column)}<button
              class="column"
              data-testid={`vj-column-${column}`}
              title={`Launch column ${column + 1}${column < 10 ? ` · ${(column + 1) % 10}` : ""}`}
              onpointerdown={(e) => {
                if (e.button === 0) {
                  e.currentTarget.setPointerCapture(e.pointerId);
                  control({ type: "column", column });
                }
              }}
              onpointerup={() => control({ type: "column", column }, 0)}
              onpointercancel={() => control({ type: "column", column }, 0)}
              onkeydown={(e) => {
                if (e.key === " " || e.key === "Enter") {
                  e.preventDefault();
                  if (!e.repeat) control({ type: "column", column });
                }
              }}
              onkeyup={(e) => {
                if (e.key === " " || e.key === "Enter") {
                  e.preventDefault();
                  control({ type: "column", column }, 0);
                }
              }}
              onclick={(e) => {
                if (e.detail === 0) control({ type: "column", column });
              }}>▶ {column + 1}</button
            >{/each}
          {#each video.tracks as track, layerIndex (layerIndex)}
            {@const state = activeState?.layers[layerIndex]}
            {@const setting = state?.settings ?? deck.layers[layerIndex]}
            <div
              class="layer-controls"
              class:selected-layer={selected.layer === layerIndex}
            >
              <div>
                <button
                  class="layer-name"
                  title="Layer effects and blend mode"
                  onclick={() => {
                    selected = { layer: layerIndex, column: 0 };
                    tab = "layer";
                  }}>{track.name}</button
                ><button
                  title={`Stop ${track.name}`}
                  onclick={() =>
                    control({ type: "stop_layer", layer: layerIndex })}
                  >■</button
                >
              </div>
              <div>
                <button
                  class:on={setting.mute}
                  title="Mute layer"
                  onclick={() =>
                    control(
                      { type: "layer_mute", layer: layerIndex },
                      setting.mute ? 0 : 1,
                    )}>M</button
                ><button
                  class:on={setting.solo}
                  title="Solo layer"
                  onclick={() =>
                    control(
                      { type: "layer_solo", layer: layerIndex },
                      setting.solo ? 0 : 1,
                    )}>S</button
                ><select
                  aria-label={`Crossfader assignment ${layerIndex + 1}`}
                  value={deck.layers[layerIndex].crossfader}
                  disabled={locked}
                  onchange={(e) =>
                    changeDeck({
                      layers: deck.layers.map((l, i) =>
                        i === layerIndex
                          ? {
                              ...l,
                              crossfader: e.currentTarget
                                .value as LayerSettings["crossfader"],
                            }
                          : l,
                      ),
                    })}
                  ><option value="none">—</option><option value="a">A</option
                  ><option value="b">B</option></select
                ><span>{Math.round(setting.opacity * 100)}%</span>
              </div>
              <input
                aria-label={`Layer ${layerIndex + 1} opacity`}
                title="Layer opacity · MIDI learn supported"
                type="range"
                min="0"
                max="1"
                step=".01"
                value={setting.opacity}
                onpointerdown={() => {
                  if (learn)
                    control({ type: "layer_opacity", layer: layerIndex });
                }}
                oninput={(e) =>
                  control(
                    { type: "layer_opacity", layer: layerIndex },
                    +e.currentTarget.value,
                  )}
              />
            </div>
            {#each deck.grid[layerIndex] as id, column (column)}
              {@const cell = track.clips.find((c) => c.id === id)}
              <button
                class="cell"
                class:playing={state?.active?.id === id && !!id}
                class:queued={state?.queued?.column === column}
                class:chosen={selected.layer === layerIndex &&
                  selected.column === column}
                data-testid={`vj-cell-${layerIndex}-${column}`}
                draggable={!editLocked && !!cell}
                title={cell
                  ? `${videoSourceLabel(cell)} · Click to launch · Right click for properties`
                  : "Drop a source here"}
                ondragstart={(e) =>
                  e.dataTransfer?.setData(
                    "application/x-mtrack-vj-cell",
                    JSON.stringify({ layer: layerIndex, column }),
                  )}
                ondragover={(e) => {
                  if (!editLocked) e.preventDefault();
                }}
                ondrop={(e) => drop(e, layerIndex, column)}
                onpointerdown={(e) => {
                  if (e.button !== 0) return;
                  selected = { layer: layerIndex, column };
                  if (cell) {
                    (e.currentTarget as HTMLElement).setPointerCapture(
                      e.pointerId,
                    );
                    control({ type: "launch", layer: layerIndex, column });
                  }
                }}
                onpointerup={() => release(layerIndex, column)}
                onpointercancel={() => release(layerIndex, column)}
                onclick={(e) => {
                  if (e.detail === 0) {
                    selected = { layer: layerIndex, column };
                    if (cell)
                      control({ type: "launch", layer: layerIndex, column });
                  }
                }}
                onkeydown={(e) => {
                  if (e.key === " " || e.key === "Enter") {
                    e.preventDefault();
                    if (!e.repeat) {
                      selected = { layer: layerIndex, column };
                      if (cell)
                        control({ type: "launch", layer: layerIndex, column });
                    }
                  }
                }}
                onkeyup={(e) => {
                  if (e.key === " " || e.key === "Enter") {
                    e.preventDefault();
                    release(layerIndex, column);
                  }
                }}
                oncontextmenu={(e) => {
                  e.preventDefault();
                  selected = { layer: layerIndex, column };
                  tab = "clip";
                }}
              >
                {#if cell}<span class="cell-type"
                    >{cell.source?.type === "compound"
                      ? "◈ COMPOUND"
                      : cell.source
                        ? "● LIVE"
                        : "▶ VIDEO"}</span
                  ><strong
                    >{cell.source?.type === "compound"
                      ? cell.source.id
                      : cell.source?.type === "v4l2"
                        ? cell.source.device
                        : cell.source
                          ? "Test pattern"
                          : cell.file}</strong
                  ><small
                    >{state?.queued?.column === column
                      ? "Queued"
                      : state?.active?.id === id
                        ? "Playing"
                        : `${cell.duration}s`}</small
                  >{#if state?.active?.id === id}<span
                      class="progress"
                      style:width={`${(((activeState!.clock - state.active.started) % cell.duration) / cell.duration) * 100}%`}
                    ></span>{/if}{:else}<span class="empty-cell">＋</span>{/if}
              </button>
            {/each}
          {/each}
        </div>
      </div>
      <button
        class="new-layer"
        disabled={editLocked}
        ondragover={(e) => {
          if (!editLocked && e.dataTransfer?.types.includes(VIDEO_MEDIA_TYPE))
            e.preventDefault();
        }}
        ondrop={(e) => drop(e, video.tracks.length, 0)}
        onclick={() =>
          addMedia(liveMedia({ type: "test_pattern" }), video.tracks.length, 0)}
        title="Drop a media, camera or compound source to create a layer. Click to add a test-pattern layer."
        >＋ New layer · drop a source</button
      >
    </main>
    <aside class="inspector">
      <div class="tabs">
        {#each ["outputs", "clip", "layer", "midi"] as item (item)}<button
            class:active={tab === item}
            onclick={() => (tab = item as typeof tab)}>{item}</button
          >{/each}
      </div>
      <div hidden={tab !== "outputs"}>
        <VjOutputEditor
          outputs={deck.outputs}
          tracks={video.tracks.map((t) => t.name)}
          running={status.running}
          {locked}
          onchange={(outputs) => changeDeck({ outputs })}
        />
      </div>
      <div hidden={tab !== "midi"}>
        <VjMidiPanel
          mappings={deck.midi}
          learning={target}
          {lastInput}
          {locked}
          onchange={(midi) => changeDeck({ midi })}
          oninput={browserMidi}
          oncancel={() => {
            target = null;
            learn = false;
          }}
        />
      </div>
      {#if tab === "clip"}<div class="properties">
          <strong>Clip · {selected.layer + 1}:{selected.column + 1}</strong
          >{#if clip}<span>{videoSourceLabel(clip)}</span><label
              >Trigger<select
                aria-label="Clip trigger mode"
                value={clipSettings.trigger}
                disabled={locked}
                onchange={(e) =>
                  changeDeck({
                    clips: {
                      ...deck.clips,
                      [clip!.id]: {
                        ...clipSettings,
                        trigger: e.currentTarget
                          .value as typeof clipSettings.trigger,
                      },
                    },
                  })}
                ><option value="restart">Restart</option><option value="toggle"
                  >Toggle</option
                ><option value="piano">Piano / hold</option></select
              ></label
            ><label class="check"
              ><input
                type="checkbox"
                checked={clipSettings.looping}
                disabled={locked}
                onchange={(e) =>
                  changeDeck({
                    clips: {
                      ...deck.clips,
                      [clip!.id]: {
                        ...clipSettings,
                        looping: e.currentTarget.checked,
                      },
                    },
                  })}
              />Loop</label
            ><label class="check"
              ><input
                type="checkbox"
                checked={clipSettings.auto_next}
                disabled={locked || clipSettings.looping}
                onchange={(e) =>
                  changeDeck({
                    clips: {
                      ...deck.clips,
                      [clip!.id]: {
                        ...clipSettings,
                        auto_next: e.currentTarget.checked,
                      },
                    },
                  })}
              />Auto next after one-shot</label
            ><label
              >Duration (s)<input
                type="number"
                min=".01"
                step=".1"
                value={clip.duration}
                disabled={editLocked}
                onchange={(e) =>
                  patchClip({ duration: +e.currentTarget.value })}
              /></label
            ><label
              >Source in (s)<input
                type="number"
                min="0"
                step=".1"
                value={clip.source_in}
                disabled={editLocked ||
                  (!!clip.source && clip.source.type !== "compound")}
                onchange={(e) =>
                  patchClip({ source_in: +e.currentTarget.value })}
              /></label
            ><button
              class="btn"
              disabled={editLocked}
              title="Remove from the deck; the timeline clip remains available"
              onclick={() => {
                const grid = copyData(deck.grid);
                grid[selected.layer][selected.column] = null;
                changeDeck({ grid });
              }}>Clear cell</button
            >{:else}<span>Select a clip or drop a source into this cell.</span
            >{/if}
        </div>{/if}
      {#if tab === "layer"}<div class="properties">
          <strong
            >{video.tracks[selected.layer]?.name ?? "Layer properties"}</strong
          >{#if layer}<label
              >Blend mode<select
                value={deck.layers[selected.layer].blend}
                disabled={locked}
                onchange={(e) =>
                  patchLayer({
                    blend: e.currentTarget.value as LayerSettings["blend"],
                  })}
                ><option value="normal">Normal</option><option value="add"
                  >Add</option
                ><option value="screen">Screen</option><option value="multiply"
                  >Multiply</option
                ></select
              ></label
            >{#each [{ key: "brightness", label: "Brightness", min: -1, max: 1 }, { key: "contrast", label: "Contrast", min: 0, max: 2 }, { key: "saturation", label: "Saturation", min: 0, max: 2 }, { key: "invert", label: "Invert", min: 0, max: 1 }] as property (property.key)}<label
                >{property.label}<input
                  type="range"
                  min={property.min}
                  max={property.max}
                  step=".01"
                  disabled={locked}
                  value={deck.layers[selected.layer][
                    property.key as "brightness"
                  ]}
                  oninput={(e) =>
                    patchLayer({ [property.key]: +e.currentTarget.value })}
                /></label
              >{/each}<button
              class="btn"
              disabled={locked}
              onclick={() =>
                patchLayer({
                  brightness: 0,
                  contrast: 1,
                  saturation: 1,
                  invert: 0,
                })}>Reset effects</button
            >{/if}
        </div>{/if}
    </aside>
  </div>
</section>

<style>
  .vj-workspace {
    --accent: #6ee7cc;
    color: #dce5f2;
    background: #0d121b;
    border: 1px solid #2e3b4d;
    border-radius: 9px;
    overflow: hidden;
  }
  .toolbar {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    padding: 12px;
    background: #161e2b;
    border-bottom: 1px solid #303d51;
  }
  .toolbar select {
    width: auto;
  }
  .live-label {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-right: auto;
  }
  .live-label strong {
    font-size: 12px;
    letter-spacing: 1.5px;
  }
  .live-label small {
    color: #9babbe;
    font-size: 11px;
  }
  .live {
    color: var(--accent);
  }
  .btn {
    font-size: 11px;
    white-space: nowrap;
  }
  .start {
    background: #1c4b43;
    color: #aff5e3;
    border-color: #398877;
  }
  .stop {
    color: #ffb1aa;
  }
  .armed {
    background: #653553 !important;
    border-color: #d19cf6 !important;
    color: #f2cfff !important;
  }
  .blackout.armed {
    background: #642b31 !important;
    color: #ffbdc4 !important;
  }
  .tempo {
    display: flex;
    gap: 5px;
    align-items: center;
    font-size: 11px;
  }
  .tempo input {
    width: 65px;
  }
  input,
  select {
    background: #101722;
    color: #dce5f2;
    border: 1px solid #344157;
    border-radius: 4px;
    padding: 5px;
    min-width: 0;
    font-size: 12px;
  }
  input[type="range"] {
    padding: 0;
    accent-color: var(--accent);
    min-width: 30px;
  }
  .performance-layout {
    display: grid;
    grid-template-columns: 205px minmax(320px, 1fr) 315px;
    align-items: start;
  }
  .library,
  .inspector {
    padding: 10px;
    min-width: 0;
  }
  .library {
    border-right: 1px solid #2c394c;
  }
  .inspector {
    border-left: 1px solid #2c394c;
    background: #141b27;
    align-self: stretch;
  }
  .tabs {
    display: flex;
    gap: 3px;
    margin-bottom: 12px;
  }
  .tabs button {
    flex: 1;
    text-transform: capitalize;
    font-size: 11px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: #8899b1;
    padding: 8px 2px;
    cursor: pointer;
  }
  .tabs button.active {
    color: #9eebdb;
    border-color: #6ee7cc;
  }
  .performance-center {
    padding: 12px;
    min-width: 0;
  }
  .program {
    max-width: 620px;
    margin: 0 auto;
    border: 1px solid #324158;
    border-radius: 5px;
    overflow: hidden;
  }
  .program header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
    background: #1a2433;
    font-size: 10px;
    letter-spacing: 1px;
  }
  .program header span {
    color: #9daec6;
  }
  .program a {
    color: #a6e6db;
  }
  .screen {
    background: #000;
    display: flex;
    align-items: center;
    justify-content: center;
    max-height: 310px;
  }
  .screen > span {
    color: #5f7089;
    font-size: 12px;
  }
  .screen img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  .master-controls {
    display: flex;
    gap: 22px;
    padding: 14px 0;
  }
  .master-controls label {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 11px;
    flex: 1;
  }
  .master-controls input {
    width: 100%;
  }
  .master-controls output {
    width: 35px;
    color: #9edbcd;
  }
  .deck-scroll {
    overflow: auto;
    border: 1px solid #334057;
    border-radius: 5px;
  }
  .deck {
    display: grid;
    gap: 4px;
    background: #0a0f17;
    padding: 4px;
    min-width: max-content;
  }
  .column,
  .deck-corner {
    height: 30px;
  }
  .deck-corner {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .deck button {
    cursor: pointer;
    background: #1a2434;
    border: 1px solid #33445e;
    border-radius: 4px;
    color: #b7c7dc;
  }
  .column {
    font-size: 11px;
  }
  .cell {
    display: flex;
    position: relative;
    flex-direction: column;
    justify-content: center;
    gap: 8px;
    height: 105px;
    padding: 9px;
    text-align: left;
    overflow: hidden;
  }
  .cell strong {
    font-size: 11px;
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cell-type {
    font-size: 9px;
    color: #8398b9;
    letter-spacing: 0.7px;
  }
  .cell small {
    font-size: 9px;
    color: #8194b1;
  }
  .cell.chosen {
    border-color: #a9bcd9;
  }
  .cell.playing {
    border-color: #6ee7cc;
    background: #14372f;
  }
  .cell.playing .cell-type,
  .cell.playing small {
    color: #8bddc8;
  }
  .cell.queued {
    border-color: #e4bc65;
    background: #41351e;
  }
  .progress {
    position: absolute;
    bottom: 0;
    left: 0;
    height: 3px;
    background: #6ee7cc;
    max-width: 100%;
  }
  .empty-cell {
    color: #33475e;
    align-self: center;
    font-size: 24px;
  }
  .layer-controls {
    padding: 7px;
    background: #182232;
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .layer-controls > div {
    display: flex;
    gap: 5px;
    align-items: center;
    justify-content: space-between;
  }
  .layer-controls .layer-name {
    max-width: 135px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    background: none;
    border: 0;
    font-size: 11px;
    text-align: left;
  }
  .layer-controls span {
    font-size: 10px;
    color: #8ea3bd;
  }
  .layer-controls button {
    font-size: 10px;
  }
  .layer-controls .on {
    background: #725b35;
    color: #ffe2a9;
  }
  .layer-controls select {
    font-size: 10px;
    padding: 2px;
  }
  .selected-layer {
    box-shadow: inset 2px 0 #81a9dd;
  }
  .new-layer {
    width: 100%;
    border: 1px dashed #334b66;
    border-radius: 5px;
    background: #101925;
    color: #89a8c6;
    padding: 10px;
    font-size: 11px;
    margin-top: 10px;
    cursor: pointer;
  }
  .learning .cell,
  .learning input {
    outline: 1px solid #a975d0;
  }
  .error,
  .learn-strip {
    display: flex;
    justify-content: space-between;
    padding: 10px 14px;
    font-size: 12px;
    background: #44242c;
    color: #ffb8b8;
  }
  .learn-strip {
    background: #3d284d;
    color: #dfb6fc;
  }
  .error button,
  .learn-strip button {
    background: none;
    border: 0;
    color: inherit;
    cursor: pointer;
  }
  .properties {
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 12px;
  }
  .properties label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: #9badc5;
  }
  .properties .check {
    flex-direction: row;
    align-items: center;
  }
  .properties > span {
    font-size: 11px;
    color: #8fa3be;
    overflow-wrap: anywhere;
  }
  .compound-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .source {
    background: #1e2940;
    border: 1px solid #3b4b6b;
    color: #c2d1eb;
    padding: 12px;
    font-size: 12px;
    text-align: left;
  }
  .compound-list span {
    font-size: 12px;
    color: #8fa3be;
  }
  .beat {
    color: #6ee7cc !important;
  }
  [hidden] {
    display: none !important;
  }
  @media (max-width: 1150px) {
    .performance-layout {
      grid-template-columns: 165px minmax(260px, 1fr) 280px;
    }
    .toolbar {
      gap: 6px;
    }
    .inspector {
      padding: 8px;
    }
  }
  @media (max-width: 850px) {
    .performance-layout {
      grid-template-columns: 160px 1fr;
    }
    .inspector {
      grid-column: 1/-1;
      border-top: 1px solid #303d51;
    }
    .screen {
      max-height: 250px;
    }
  }
</style>
