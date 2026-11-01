<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import type { SongFile } from "../../lib/api/songs";
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
  import VideoPropertiesPanel from "./VideoPropertiesPanel.svelte";
  import VideoPreview from "./VideoPreview.svelte";
  import VideoTrackLane from "./VideoTrackLane.svelte";

  interface Props {
    video: VideoConfig | null;
    songName: string;
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
    songDurationSeconds = 0,
    files = [],
    previewAvailable = null,
    selectedClipId = $bindable(null),
    onchange,
    onvaliditychange,
  }: Props = $props();

  let error = $state("");
  let pixelsPerSecond = $state(10);
  let addTrackIndex = $state(0);
  let addFile = $state("");
  let addStart = $state(0);
  let addDuration = $state(10);
  let addSourceIn = $state(0);
  let previewStale = $state(false);

  let config = $derived(video ?? createVideoConfig());
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
    error = "";
    previewStale = true;
    onchange?.(next);
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
    if (!candidate.file.trim()) {
      error = "Clip file must not be empty.";
      return;
    }
    commitClip(selected.trackIndex, candidate);
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

<div class="video-editor" data-testid="video-timeline-editor">
  <VideoPreview
    {songName}
    {config}
    durationMs={songDurationSeconds * 1000}
    stale={previewStale}
    available={previewAvailable}
    onreloaded={() => (previewStale = false)}
  />

  <div class="toolbar">
    <div>
      <strong>Video timeline</strong>
      <span class="canvas-summary"
        >{config.canvas.width}×{config.canvas.height} · {config.canvas
          .frame_rate} fps</span
      >
    </div>
    <div class="toolbar-actions">
      <button
        type="button"
        class="btn btn-sm"
        aria-label="Zoom out video timeline"
        onclick={() => (pixelsPerSecond = Math.max(2, pixelsPerSecond / 1.25))}
        >−</button
      >
      <button
        type="button"
        class="btn btn-sm"
        aria-label="Zoom in video timeline"
        onclick={() => (pixelsPerSecond = Math.min(80, pixelsPerSecond * 1.25))}
        >+</button
      >
      <button
        type="button"
        class="btn btn-sm"
        data-testid="video-add-track"
        onclick={addTrack}>Add track</button
      >
    </div>
  </div>

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

  {#if displayedError}
    <div class="video-error" data-testid="video-error" role="alert">
      {displayedError}
    </div>
  {/if}

  {#if config.tracks.length === 0}
    <div class="empty">Add a track, then place file-backed clips on it.</div>
  {:else}
    <div class="timeline-scroll">
      <div class="ruler-row">
        <div class="ruler-label">Time</div>
        <div class="ruler" style:width="{timelineWidth}px">
          {#each rulerTicks as second (second)}
            <span style:left="{second * pixelsPerSecond}px">{second}s</span>
          {/each}
        </div>
      </div>
      {#each config.tracks as track, trackIndex (trackIndex)}
        <VideoTrackLane
          {track}
          {trackIndex}
          {pixelsPerSecond}
          {timelineWidth}
          timelineDuration={clipTimelineDuration}
          {selectedClipId}
          onrename={(name) => renameTrack(trackIndex, name)}
          onremove={() => removeTrack(trackIndex)}
          onselectclip={(id) => {
            error = "";
            selectedClipId = id;
          }}
          oncommitclip={(clip) => commitClip(trackIndex, clip)}
          ondeleteclip={(id) => deleteClip(trackIndex, id)}
        />
      {/each}
    </div>
  {/if}

  {#if selected}
    <VideoPropertiesPanel
      clip={selected.clip}
      onpatch={patchSelected}
      ondelete={deleteSelected}
    />
  {/if}
</div>

<style>
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
  .ruler {
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
