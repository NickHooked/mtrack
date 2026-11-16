<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import {
    fetchSongFiles,
    uploadTrack,
    type SongFile,
  } from "../../lib/api/songs";
  import VideoLiveSources from "./VideoLiveSources.svelte";
  import { parseLiveMedia } from "../../lib/video/media";
  import VideoMediaPool from "./VideoMediaPool.svelte";
  import {
    isVideoFile,
    localVideoDuration,
    VIDEO_MEDIA_TYPE,
  } from "../../lib/video/media";
  import type {
    VideoClip,
    VideoConfig,
    VideoTrack,
  } from "../../lib/video/types";
  import {
    createVideoConfig,
    normalizeClipPatch,
    overlapError,
    uniqueClipId,
    videoConfigError,
    videoEnd,
  } from "../../lib/video/timeline-state";
  import VideoTrackAutomation from "./VideoTrackAutomation.svelte";
  import {
    evaluatedClip,
    insertPoint,
    propertyValue,
  } from "../../lib/video/automation.svelte";
  import VideoCompositionPanel from "./VideoCompositionPanel.svelte";
  import VideoPropertiesPanel from "./VideoPropertiesPanel.svelte";
  import VideoPreview from "./VideoPreview.svelte";
  import VideoTrackLane from "./VideoTrackLane.svelte";

  interface Props {
    video: VideoConfig | null;
    songName: string;
    locked?: boolean;
    songDurationSeconds?: number;
    files?: SongFile[];
    previewAvailable?: boolean | null;
    selectedClipId?: string | null;
    onchange?: (video: VideoConfig) => void;
    onvaliditychange?: (error: string | null) => void;
  }

  let {
    video,
    songName,
    locked = false,
    songDurationSeconds = 0,
    files = [],
    previewAvailable = null,
    selectedClipId = $bindable(null),
    onchange,
    onvaliditychange,
  }: Props = $props();

  let editingCompound = $state<string | null>(null);
  let compoundName = $state("");
  let rootConfig = $derived(video ?? createVideoConfig());
  let config = $derived(
    editingCompound && rootConfig.compounds?.[editingCompound]
      ? {
          ...rootConfig.compounds[editingCompound],
          compounds: rootConfig.compounds,
        }
      : rootConfig,
  );
  function publish(next: VideoConfig) {
    if (editingCompound) {
      const { compounds: library, ...definition } = next;
      onchange?.({
        ...rootConfig,
        compounds: { ...library, [editingCompound]: definition },
      });
    } else onchange?.(next);
  }
  function openCompound(id: string | null) {
    editingCompound = id;
    selectedClipId = null;
    playhead = 0;
    undoStack = [];
    redoStack = [];
    previewStale = true;
  }
  function createCompound() {
    const id =
      compoundName.trim() ||
      `Compound ${Object.keys(rootConfig.compounds ?? {}).length + 1}`;
    if (rootConfig.compounds?.[id]) {
      error = "Choose a unique compound name.";
      return;
    }
    onchange?.({
      ...rootConfig,
      compounds: {
        ...rootConfig.compounds,
        [id]: { canvas: { ...config.canvas }, tracks: [] },
      },
    });
    compoundName = "";
    openCompound(id);
  }

  let error = $state("");
  let playhead = $state(0);
  let selectedMask = $state(-1);
  let drawingMask = $state(false);
  $effect(() => {
    void selectedClipId;
    selectedMask = -1;
    drawingMask = false;
  });
  let libraryTab = $state<"media" | "live" | "compounds">("media");
  let previewControl = $state<VideoPreview>();
  onMount(() => {
    let active = true;
    fetchSongFiles(songName)
      .then((result) => {
        if (active)
          mediaFiles = result
            .filter((file) => isVideoFile(file.name))
            .map((file) => file.name);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  });
  let mediaFiles = $state<string[]>([]);
  let mediaDurations = $state<Record<string, number>>({});
  let importing = $state(false);
  let importMessage = $state("");
  let snapping = $state(true);
  let undoStack = $state<VideoConfig[]>([]);
  let redoStack = $state<VideoConfig[]>([]);
  let media = $derived(
    [
      ...new Set([
        ...files
          .filter((file) => isVideoFile(file.name))
          .map((file) => file.name),
        ...mediaFiles,
        ...config.tracks.flatMap((track) =>
          track.clips.filter((clip) => !clip.source).map((clip) => clip.file),
        ),
      ]),
    ].sort((a, b) => a.localeCompare(b)),
  );

  function copy(value: VideoConfig): VideoConfig {
    return JSON.parse(JSON.stringify(value));
  }
  function undo() {
    const previous = undoStack.at(-1);
    if (!previous || locked) return;
    redoStack = [...redoStack, copy(config)];
    undoStack = undoStack.slice(0, -1);
    error = "";
    previewStale = true;
    publish(previous);
  }
  function redo() {
    const next = redoStack.at(-1);
    if (!next || locked) return;
    undoStack = [...undoStack, copy(config)];
    redoStack = redoStack.slice(0, -1);
    error = "";
    previewStale = true;
    publish(next);
  }
  function historyKey(event: KeyboardEvent) {
    if (
      !(event.ctrlKey || event.metaKey) ||
      !(event.target instanceof HTMLElement)
    )
      return;
    if (event.target.closest("input, textarea, select, [contenteditable=true]"))
      return;
    if (event.key.toLowerCase() === "z") {
      event.preventDefault();
      if (event.shiftKey) redo();
      else undo();
    } else if (event.key.toLowerCase() === "y") {
      event.preventDefault();
      redo();
    }
  }

  async function importMedia(incoming: File[]) {
    if (importing || locked) return;
    importing = true;
    importMessage = "";
    const messages: string[] = [];
    try {
      const refreshed = await fetchSongFiles(songName);
      const existing = new SvelteSet([
        ...refreshed.map((file) => file.name),
        ...media,
      ]);
      let count = 0;
      for (const file of incoming) {
        if (!isVideoFile(file.name)) {
          messages.push(`${file.name}: choose a video file.`);
          continue;
        }
        if (existing.has(file.name)) {
          messages.push(
            `${file.name} is already in this song; kept the existing file.`,
          );
          continue;
        }
        try {
          await uploadTrack(songName, file);
          existing.add(file.name);
          mediaFiles = [...mediaFiles, file.name];
          const duration = await localVideoDuration(file);
          if (duration !== null)
            mediaDurations = { ...mediaDurations, [file.name]: duration };
          count++;
        } catch (cause) {
          messages.push(
            `${file.name}: ${cause instanceof Error ? cause.message : String(cause)}`,
          );
        }
      }
      importMessage = [
        `${count} video${count === 1 ? "" : "s"} imported.`,
        ...messages,
      ].join(" ");
    } catch (cause) {
      importMessage = cause instanceof Error ? cause.message : String(cause);
    } finally {
      importing = false;
    }
  }

  function nextTrackName(): string {
    let number = config.tracks.length + 1;
    while (config.tracks.some((track) => track.name === `Video ${number}`))
      number++;
    return `Video ${number}`;
  }
  function placeMedia(file: string, trackIndex: number, start?: number) {
    const source = parseLiveMedia(file);
    if (locked || (!source && !media.includes(file))) return;
    if (
      source?.type === "compound" &&
      (!rootConfig.compounds?.[source.id] || source.id === editingCompound)
    )
      return;
    const track = config.tracks[trackIndex] ?? {
      name: nextTrackName(),
      clips: [],
    };
    const position =
      start ??
      track.clips.reduce(
        (end, clip) => Math.max(end, clip.start + clip.duration),
        0,
      );
    const clip: VideoClip = {
      id: uniqueClipId(
        config,
        source?.type === "compound"
          ? source.id
          : source?.type === "v4l2"
            ? source.device
            : source
              ? "live-test"
              : file,
      ),
      file: source ? "" : file,
      ...(source ? { source } : {}),
      start: position,
      duration:
        (source?.type === "compound"
          ? Math.max(0.01, videoEnd(rootConfig.compounds![source.id]))
          : mediaDurations[file]) ?? Math.max(0.01, Number(addDuration) || 10),
      source_in: 0,
    };
    const overlap = overlapError(track, clip);
    if (overlap) {
      error = overlap;
      return;
    }
    const next = { ...track, clips: [...track.clips, clip] };
    emit(
      config.tracks[trackIndex]
        ? replaceTrack(trackIndex, next)
        : { ...config, tracks: [...config.tracks, next] },
    );
    selectedClipId = clip.id;
  }
  function moveAcrossTracks(
    sourceIndex: number,
    targetIndex: number,
    clip: VideoClip,
  ) {
    if (sourceIndex === targetIndex) {
      commitClip(sourceIndex, clip);
      return;
    }
    const target = config.tracks[targetIndex];
    if (!target) return;
    const overlap = overlapError(target, clip);
    if (overlap) {
      error = overlap;
      return;
    }
    // Remove the now-empty source, since persisted video tracks must have clips.
    emit({
      ...config,
      tracks: config.tracks
        .map((track, index) => {
          if (index === sourceIndex)
            return {
              ...track,
              clips: track.clips.filter((item) => item.id !== clip.id),
            };
          if (index === targetIndex)
            return { ...track, clips: [...track.clips, clip] };
          return track;
        })
        .filter(
          (track, index) => index !== sourceIndex || track.clips.length > 0,
        ),
    });
  }

  let pixelsPerSecond = $state(10);
  let addTrackIndex = $state(0);
  let addFile = $state("");
  let addStart = $state(0);
  let addDuration = $state(10);
  let addSourceIn = $state(0);
  let previewStale = $state(false);

  let validationError = $derived(videoConfigError(config));
  let displayedError = $derived(error || validationError || "");

  $effect(() => {
    onvaliditychange?.(validationError);
  });
  let selected = $derived.by(() => {
    if (!selectedClipId) return null;
    for (let trackIndex = 0; trackIndex < config.tracks.length; trackIndex++) {
      const clipIndex = config.tracks[trackIndex].clips.findIndex(
        (clip) => clip.id === selectedClipId,
      );
      if (clipIndex >= 0) {
        return {
          trackIndex,
          clipIndex,
          clip: config.tracks[trackIndex].clips[clipIndex],
        };
      }
    }
    return null;
  });
  let timelineSeconds = $derived(
    Math.max(60, songDurationSeconds, videoEnd(config) + 5),
  );
  let clipTimelineDuration = $derived(Math.max(60, videoEnd(config) + 5));
  let timelineWidth = $derived(timelineSeconds * pixelsPerSecond);
  let rulerTicks = $derived(
    Array.from(
      { length: Math.floor(timelineSeconds / 10) + 1 },
      (_, index) => index * 10,
    ),
  );

  function emit(next: VideoConfig) {
    if (locked || JSON.stringify(config) === JSON.stringify(next)) return;
    undoStack = [...undoStack.slice(-49), copy(config)];
    redoStack = [];
    error = "";
    previewStale = true;
    publish(next);
  }

  function replaceTrack(trackIndex: number, track: VideoTrack): VideoConfig {
    return {
      ...config,
      canvas: { ...config.canvas },
      tracks: config.tracks.map((current, index) =>
        index === trackIndex ? track : current,
      ),
    };
  }

  function addTrack() {
    let number = config.tracks.length + 1;
    let name = `Video ${number}`;
    const names = new Set(config.tracks.map((track) => track.name));
    while (names.has(name)) name = `Video ${++number}`;
    emit({
      ...config,
      canvas: { ...config.canvas },
      tracks: [...config.tracks, { name, clips: [] }],
    });
    addTrackIndex = config.tracks.length;
  }

  function renameTrack(trackIndex: number, name: string) {
    emit(replaceTrack(trackIndex, { ...config.tracks[trackIndex], name }));
  }

  function removeTrack(trackIndex: number) {
    const removed = config.tracks[trackIndex];
    if (removed.clips.some((clip) => clip.id === selectedClipId)) {
      selectedClipId = null;
    }
    emit({
      ...config,
      canvas: { ...config.canvas },
      tracks: config.tracks.filter((_, index) => index !== trackIndex),
    });
    addTrackIndex = Math.max(
      0,
      Math.min(addTrackIndex, config.tracks.length - 2),
    );
  }

  function addClip() {
    if (config.tracks.length === 0) {
      error = "Add a video track before adding a clip.";
      return;
    }
    const file = addFile.trim();
    if (!file) {
      error = "Choose or enter a video file.";
      return;
    }
    const trackIndex = Math.min(addTrackIndex, config.tracks.length - 1);
    const clip: VideoClip = {
      id: uniqueClipId(config, file),
      start: Math.max(0, Number(addStart) || 0),
      duration: Math.max(0.01, Number(addDuration) || 0.01),
      file,
      source_in: Math.max(0, Number(addSourceIn) || 0),
    };
    const overlap = overlapError(config.tracks[trackIndex], clip);
    if (overlap) {
      error = overlap;
      return;
    }
    emit(
      replaceTrack(trackIndex, {
        ...config.tracks[trackIndex],
        clips: [...config.tracks[trackIndex].clips, clip],
      }),
    );
    selectedClipId = clip.id;
    addFile = "";
  }

  function commitClip(trackIndex: number, candidate: VideoClip) {
    const track = config.tracks[trackIndex];
    const overlap = overlapError(track, candidate, candidate.id);
    if (overlap) {
      error = overlap;
      return;
    }
    emit(
      replaceTrack(trackIndex, {
        ...track,
        clips: track.clips.map((clip) =>
          clip.id === candidate.id ? candidate : clip,
        ),
      }),
    );
  }

  function patchSelected(patch: Partial<VideoClip>) {
    if (!selected) return;
    const candidate = normalizeClipPatch(selected.clip, patch);
    if (!candidate.source && !candidate.file.trim()) {
      error = "Clip file must not be empty.";
      return;
    }
    commitClip(selected.trackIndex, candidate);
  }

  function patchComposition(patch: Partial<VideoClip>) {
    if (selected && !patch.automation) {
      const clip = selected.clip;
      const automation = { ...clip.automation };
      // Topology changes invalidate mask-index paths; never redirect a curve to another point.
      if (
        patch.masks &&
        JSON.stringify(patch.masks.map((m) => [m.shape, m.points?.length])) !==
          JSON.stringify(
            (clip.masks ?? []).map((m) => [m.shape, m.points?.length]),
          )
      ) {
        for (const path of Object.keys(automation))
          if (path.startsWith("/masks/")) delete automation[path];
      }
      const candidate = { ...clip, ...patch };
      for (const [path, lane] of Object.entries(automation)) {
        if (lane.enabled && path.slice(1).split("/")[0] in patch) {
          const value = propertyValue(candidate, path);
          const current = propertyValue(
            evaluatedClip(clip, config.canvas, playhead - clip.start),
            path,
          );
          if (value !== current)
            automation[path] = insertPoint(
              lane,
              Math.max(0, Math.min(clip.duration, playhead - clip.start)),
              value,
            );
        }
      }
      patch = { ...patch, automation };
    }
    patchSelected(patch);
    previewControl?.refreshComposition();
  }

  function deleteClip(trackIndex: number, clipId: string) {
    const track = config.tracks[trackIndex];
    if (!track) return;
    const clips = track.clips.filter((clip) => clip.id !== clipId);
    if (clips.length === track.clips.length) return;
    if (clips.length === 0) {
      removeTrack(trackIndex);
    } else {
      emit(replaceTrack(trackIndex, { ...track, clips }));
    }
    if (selectedClipId === clipId) selectedClipId = null;
  }

  function deleteSelected() {
    if (!selected) return;
    deleteClip(selected.trackIndex, selected.clip.id);
  }
</script>

<svelte:window onkeydown={historyKey} />
<div class="video-editor" data-testid="video-timeline-editor">
  {#if editingCompound}<div class="compound-navigation">
      <button class="btn btn-sm" onclick={() => openCompound(null)}
        >← Main timeline</button
      ><strong>Compound · {editingCompound}</strong>
    </div>{/if}
  <div class="workspace">
    <div class="source-library">
      <div class="library-tabs" role="tablist" aria-label="Video sources">
        <button
          role="tab"
          aria-selected={libraryTab === "media"}
          onclick={() => (libraryTab = "media")}>Media files</button
        >
        <button
          role="tab"
          aria-selected={libraryTab === "live"}
          onclick={() => (libraryTab = "live")}>Live sources</button
        >
        <button
          role="tab"
          aria-selected={libraryTab === "compounds"}
          onclick={() => (libraryTab = "compounds")}>Compounds</button
        >
      </div>
      <div hidden={libraryTab !== "compounds"} class="compound-library">
        <input
          aria-label="New compound name"
          placeholder="Compound name"
          bind:value={compoundName}
          disabled={locked}
        />
        <button class="btn btn-sm" disabled={locked} onclick={createCompound}
          >＋ New compound</button
        >
        {#each Object.entries(rootConfig.compounds ?? {}) as [id, compound] (id)}
          <div class="compound-source">
            <button
              class="btn btn-sm"
              disabled={locked || id === editingCompound}
              draggable={!locked && id !== editingCompound}
              title="Drag onto a video track or click to append."
              ondragstart={(e) => {
                e.dataTransfer?.setData(
                  VIDEO_MEDIA_TYPE,
                  `mtrack-live:${JSON.stringify({ type: "compound", id })}`,
                );
                if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
              }}
              onclick={() =>
                placeMedia(
                  `mtrack-live:${JSON.stringify({ type: "compound", id })}`,
                  selected?.trackIndex ?? 0,
                )}>{id} · {videoEnd(compound).toFixed(1)}s</button
            ><button
              class="btn btn-sm"
              disabled={locked}
              aria-label={`Edit compound ${id}`}
              onclick={() => openCompound(id)}>Edit</button
            >
          </div>
        {/each}
      </div>
      <div hidden={libraryTab !== "media"}>
        <VideoMediaPool
          files={media}
          busy={importing}
          {locked}
          onimport={importMedia}
          onadd={(file) => placeMedia(file, selected?.trackIndex ?? 0)}
        />
      </div>
      <div hidden={libraryTab !== "live"}>
        <VideoLiveSources
          {locked}
          onadd={(file) => placeMedia(file, selected?.trackIndex ?? 0)}
        />
      </div>
    </div>
    <div class="viewer">
      {#key editingCompound}<VideoPreview
          bind:this={previewControl}
          editingClip={locked || !selected
            ? null
            : evaluatedClip(
                selected.clip,
                config.canvas,
                Math.max(0, playhead - selected.clip.start),
              )}
          bind:selectedMask
          bind:drawingMask
          onpatch={patchComposition}
          onposition={(ms) => (playhead = ms / 1000)}
          {songName}
          {config}
          durationMs={Math.max(
            editingCompound ? 0 : songDurationSeconds,
            videoEnd(config),
          ) * 1000}
          stale={previewStale}
          available={previewAvailable}
          onreloaded={() => (previewStale = false)}
        />{/key}
    </div>
    <aside class="inspector" aria-label="Video properties" inert={locked}>
      {#if selected}
        <VideoCompositionPanel
          clip={evaluatedClip(
            selected.clip,
            config.canvas,
            Math.max(0, playhead - selected.clip.start),
          )}
          canvas={config.canvas}
          bind:selectedMask
          bind:drawingMask
          onpatch={patchComposition}
        />
        <VideoPropertiesPanel
          clip={selected.clip}
          onpatch={patchSelected}
          ondelete={deleteSelected}
        />
      {/if}
      {#if !selected}<p>Select a clip to edit its position and masks.</p>{/if}
    </aside>
  </div>
  {#if importMessage}<div
      class="import-message"
      role="status"
      data-testid="video-import-status"
    >
      {importMessage}
    </div>{/if}
  {#if locked}<div class="import-message">
      Unlock the player to edit the timeline.
    </div>{/if}
  <div class="edit-controls" inert={locked}>
    <div class="toolbar">
      <div>
        <strong
          title="Drag media onto a track. Drag clips to move them or their edges to trim. Hold Shift to bypass snapping. New clips use the imported duration when available; otherwise the manual duration (default 10s)."
          >Video timeline</strong
        >
        <span class="canvas-summary"
          >{config.canvas.width}×{config.canvas.height} · {config.canvas
            .frame_rate} fps</span
        >
      </div>
      <div class="toolbar-actions">
        <button
          class="btn btn-sm"
          type="button"
          aria-label="Undo video edit"
          disabled={!undoStack.length}
          onclick={undo}>↶ Undo</button
        >
        <button
          class="btn btn-sm"
          type="button"
          aria-label="Redo video edit"
          disabled={!redoStack.length}
          onclick={redo}>↷ Redo</button
        >
        <button
          class="btn btn-sm"
          type="button"
          aria-pressed={snapping}
          onclick={() => (snapping = !snapping)}>Snap</button
        >
        <button
          type="button"
          class="btn btn-sm"
          aria-label="Zoom out video timeline"
          onclick={() =>
            (pixelsPerSecond = Math.max(2, pixelsPerSecond / 1.25))}>−</button
        >
        <button
          type="button"
          class="btn btn-sm"
          aria-label="Zoom in video timeline"
          onclick={() =>
            (pixelsPerSecond = Math.min(80, pixelsPerSecond * 1.25))}>+</button
        >
        <button
          type="button"
          class="btn btn-sm"
          data-testid="video-add-track"
          onclick={addTrack}>Add track</button
        >
      </div>
    </div>

    <details class="manual-placement">
      <summary>Manual placement</summary>
      <div class="add-panel">
        <label class="file-control">
          <span>File</span>
          <input
            type="text"
            list="video-song-files"
            placeholder="video.mp4"
            data-testid="video-add-file"
            bind:value={addFile}
          />
          <datalist id="video-song-files">
            {#each files as file (file.name)}
              <option value={file.name}></option>
            {/each}
          </datalist>
        </label>
        <label>
          <span>Track</span>
          <select
            data-testid="video-add-track-select"
            value={addTrackIndex}
            disabled={config.tracks.length === 0}
            onchange={(event) =>
              (addTrackIndex = Number(event.currentTarget.value))}
          >
            {#each config.tracks as track, index (index)}
              <option value={index}>{track.name}</option>
            {/each}
          </select>
        </label>
        <label>
          <span>Start (s)</span>
          <input
            type="number"
            min="0"
            step="0.01"
            data-testid="video-add-start"
            bind:value={addStart}
          />
        </label>
        <label>
          <span>Duration (s)</span>
          <input
            type="number"
            min="0.01"
            step="0.01"
            data-testid="video-add-duration"
            bind:value={addDuration}
          />
        </label>
        <label>
          <span>Source in (s)</span>
          <input
            type="number"
            min="0"
            step="0.01"
            data-testid="video-add-source-in"
            bind:value={addSourceIn}
          />
        </label>
        <button
          type="button"
          class="btn btn-sm btn-primary"
          data-testid="video-add-clip"
          onclick={addClip}>Add clip</button
        >
      </div>
    </details>

    {#if displayedError}
      <div class="video-error" data-testid="video-error" role="alert">
        {displayedError}
      </div>
    {/if}

    {#if config.tracks.length === 0}
      <p class="empty">Drag media below to create your first video track.</p>
    {:else}
      <div class="timeline-scroll">
        <div class="ruler-row">
          <div class="ruler-label">Time</div>
          <button
            type="button"
            class="ruler"
            style:width="{timelineWidth}px"
            aria-label="Seek video timeline"
            onclick={(event) =>
              previewControl?.seekTo(
                ((event.clientX -
                  event.currentTarget.getBoundingClientRect().left) /
                  pixelsPerSecond) *
                  1000,
              )}
          >
            <i
              class="ruler-playhead"
              style:left="{playhead * pixelsPerSecond}px"
            ></i>
            {#each rulerTicks as second (second)}
              <span style:left="{second * pixelsPerSecond}px">{second}s</span>
            {/each}
          </button>
        </div>
        {#each config.tracks as track, trackIndex (trackIndex)}
          <VideoTrackLane
            {track}
            {trackIndex}
            {pixelsPerSecond}
            {timelineWidth}
            timelineDuration={clipTimelineDuration}
            {selectedClipId}
            {snapping}
            {playhead}
            snapPoints={config.tracks.flatMap((item) =>
              item.clips.flatMap((clip) => [
                { time: clip.start, id: clip.id },
                { time: clip.start + clip.duration, id: clip.id },
              ]),
            )}
            ondropmedia={(file, start) => placeMedia(file, trackIndex, start)}
            onmoveclip={(clip, targetIndex) =>
              moveAcrossTracks(trackIndex, targetIndex, clip)}
            onrename={(name) => renameTrack(trackIndex, name)}
            onremove={() => removeTrack(trackIndex)}
            onselectclip={(id) => {
              error = "";
              selectedClipId = id;
            }}
            oncommitclip={(clip) => commitClip(trackIndex, clip)}
            ondeleteclip={(id) => deleteClip(trackIndex, id)}
          />
          <VideoTrackAutomation
            {track}
            canvas={config.canvas}
            {selectedClipId}
            {pixelsPerSecond}
            {timelineWidth}
            position={playhead}
            onselectclip={(id) => (selectedClipId = id)}
            onpatch={(clip, patch) => {
              commitClip(trackIndex, normalizeClipPatch(clip, patch));
              previewControl?.refreshComposition();
            }}
            onseek={(time) => previewControl?.seekTo(time * 1000)}
          />
        {/each}
      </div>
    {/if}

    <button
      type="button"
      class="new-track-drop"
      data-testid="video-new-track-drop"
      ondragover={(event) => {
        if (event.dataTransfer?.types.includes(VIDEO_MEDIA_TYPE))
          event.preventDefault();
      }}
      ondrop={(event) => {
        event.preventDefault();
        const file = event.dataTransfer?.getData(VIDEO_MEDIA_TYPE);
        if (file) placeMedia(file, config.tracks.length, 0);
      }}
      onclick={addTrack}
      >＋ Drop media here to create a track · or click to add an empty track</button
    >
  </div>
</div>

<style>
  .compound-navigation,
  .compound-source {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .compound-library {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .compound-library[hidden] {
    display: none;
  }
  .compound-library input {
    width: 100%;
    box-sizing: border-box;
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    border: 1px solid var(--card-border);
    padding: 6px;
  }

  .source-library {
    min-width: 0;
  }
  .library-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 6px;
  }
  .library-tabs button {
    padding: 6px 10px;
    color: var(--nc-fg-3);
    background: var(--nc-bg-2);
    border: 1px solid var(--card-border);
    border-radius: 4px;
    cursor: pointer;
  }
  .library-tabs button[aria-selected="true"] {
    color: var(--nc-cyan-400);
    border-color: var(--nc-cyan-400);
  }

  .workspace {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr) 300px;
    gap: 12px;
    align-items: stretch;
  }
  .inspector {
    height: 430px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }
  .viewer {
    min-width: 0;
  }
  .edit-controls {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .import-message {
    margin: 0;
    color: var(--nc-fg-3);
    font-size: 12px;
    line-height: 1.5;
  }
  .manual-placement summary {
    cursor: pointer;
    color: var(--nc-fg-3);
    font-size: 12px;
    padding: 4px 0;
  }
  .new-track-drop {
    padding: 16px;
    border: 1px dashed var(--card-border);
    border-radius: 6px;
    background: transparent;
    color: var(--nc-fg-3);
    cursor: pointer;
  }
  .new-track-drop:hover {
    border-color: var(--nc-cyan-400);
  }
  @media (max-width: 760px) {
    .workspace {
      grid-template-columns: 1fr;
    }
    .toolbar {
      flex-wrap: wrap;
    }
  }
  .video-editor {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 12px;
  }
  .toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }
  .toolbar strong {
    color: var(--nc-fg-1);
    font: 600 14px var(--nc-font-display);
  }
  .canvas-summary {
    margin-left: 10px;
    color: var(--nc-fg-3);
    font: 11px var(--nc-font-mono);
  }
  .toolbar-actions {
    display: flex;
    gap: 6px;
  }
  .add-panel {
    display: grid;
    grid-template-columns:
      minmax(180px, 2fr) minmax(130px, 1fr) repeat(3, 105px)
      auto;
    align-items: end;
    gap: 8px;
    padding: 12px;
    border: 1px solid var(--card-border);
    border-radius: var(--nc-radius-md);
    background: var(--nc-bg-2);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--nc-fg-3);
    font-size: 11px;
    font-weight: 600;
  }
  input,
  select {
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
    padding: 7px 8px;
    border: 1px solid var(--card-border);
    border-radius: 5px;
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    font: 12px var(--nc-font-mono);
  }
  input:focus,
  select:focus {
    outline: none;
    border-color: var(--nc-cyan-400);
  }
  .video-error {
    padding: 9px 12px;
    border: 1px solid color-mix(in srgb, var(--nc-error) 45%, transparent);
    border-radius: 6px;
    background: color-mix(in srgb, var(--nc-error) 10%, transparent);
    color: var(--nc-error);
    font-size: 13px;
  }
  .empty {
    padding: 36px;
    border: 1px dashed var(--card-border);
    border-radius: var(--nc-radius-md);
    text-align: center;
    color: var(--nc-fg-3);
    font-size: 13px;
  }
  .timeline-scroll {
    overflow-x: auto;
    border: 1px solid var(--card-border);
    border-radius: var(--nc-radius-md);
    background: var(--nc-bg-1);
  }
  .ruler-row {
    display: grid;
    grid-template-columns: 150px 1fr;
    min-width: max-content;
    border-bottom: 1px solid var(--card-border);
  }
  .ruler-label {
    position: sticky;
    left: 0;
    z-index: 9;
    width: 150px;
    box-sizing: border-box;
    padding: 7px 10px;
    border-right: 1px solid var(--card-border);
    background: var(--nc-bg-2);
    color: var(--nc-fg-3);
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
  }
  .ruler-playhead {
    position: absolute;
    z-index: 2;
    top: 0;
    bottom: 0;
    border-left: 2px solid var(--nc-cyan-400);
    pointer-events: none;
  }
  .ruler {
    padding: 0;
    border: 0;
    text-align: left;
    cursor: crosshair;
    position: relative;
    height: 28px;
    background: var(--nc-bg-2);
  }
  .ruler span {
    position: absolute;
    top: 7px;
    transform: translateX(3px);
    color: var(--nc-fg-4);
    font: 9px var(--nc-font-mono);
  }
  @media (max-width: 950px) {
    .add-panel {
      grid-template-columns: repeat(3, minmax(120px, 1fr));
    }
    .file-control {
      grid-column: span 2;
    }
  }
  @media (max-width: 600px) {
    .add-panel {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .file-control,
    .add-panel button {
      grid-column: span 2;
    }
  }
</style>
