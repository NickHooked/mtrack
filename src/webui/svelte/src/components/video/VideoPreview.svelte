<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import { onDestroy } from "svelte";
  import type { VideoConfig } from "../../lib/video/types";
  import {
    VideoPreviewClient,
    type PreviewConnectionLevelState,
    type PreviewConnectionState,
    type PreviewPlaybackState,
  } from "../../lib/video/preview";

  interface Props {
    songName: string;
    config: VideoConfig;
    durationMs?: number;
    stale?: boolean;
    available?: boolean | null;
    onreloaded?: () => void;
  }

  let {
    songName,
    config,
    durationMs = 0,
    stale = false,
    available = null,
    onreloaded,
  }: Props = $props();

  let client: VideoPreviewClient | null = null;
  let connection = $state<PreviewConnectionState>("disconnected");
  let playback = $state<PreviewPlaybackState>("stopped");
  let connectionLevelState = $state<PreviewConnectionLevelState | null>(null);
  let positionMs = $state(0);
  let frameUrl = $state<string | null>(null);
  let error = $state("");
  let busy = $state(false);
  let hasStarted = $state(false);
  let fullscreen = $state(false);
  type ReloadRequest = {
    requestId: string;
    baselineEpoch: bigint | null;
    acknowledgedEpoch: bigint | null;
    frameBeforeAcknowledgement: bigint | null;
  };
  let pendingReload: ReloadRequest | null = null;
  let acceptedEpoch: bigint | null = null;
  let previewElement: HTMLElement | null = $state(null);
  let connectedSong = "";
  let clientGeneration = 0;

  let effectiveDuration = $derived(
    Math.max(1, Math.round(durationMs), positionMs),
  );
  let unavailable = $derived(
    available === false || connectionLevelState === "disabled",
  );
  let controlsDisabled = $derived(
    unavailable ||
      connection !== "connected" ||
      busy ||
      playback === "shutting_down",
  );

  function completeReload() {
    pendingReload = null;
    onreloaded?.();
  }

  function createClient() {
    client?.destroy();
    const generation = ++clientGeneration;
    connectedSong = songName;
    client = new VideoPreviewClient(songName, {
      onConnection: (value) => {
        if (generation !== clientGeneration) return;
        connection = value;
        if (value !== "connecting") busy = false;
      },
      onState: (message) => {
        if (generation !== clientGeneration) return;
        playback = message.state;
        positionMs = message.position_ms;
        acceptedEpoch = BigInt(message.epoch);
        connectionLevelState = null;
        busy = false;
        error = "";
        if (pendingReload?.requestId === message.request_id) {
          pendingReload.acknowledgedEpoch = acceptedEpoch;
          if (
            pendingReload.frameBeforeAcknowledgement === acceptedEpoch &&
            (pendingReload.baselineEpoch === null ||
              acceptedEpoch > pendingReload.baselineEpoch)
          ) {
            completeReload();
          }
        }
      },
      onError: (message) => {
        if (generation !== clientGeneration) return;
        if (message.state === "busy" || message.state === "disabled") {
          connectionLevelState = message.state;
        } else {
          playback = message.state;
          positionMs = message.position_ms;
          connectionLevelState = null;
        }
        busy = false;
        pendingReload = null;
        error = message.error;
      },
      onFrame: (frame) => {
        if (generation !== clientGeneration) return;
        frameUrl = frame.url;
        positionMs = Number(
          frame.positionMs > BigInt(Number.MAX_SAFE_INTEGER)
            ? 0n
            : frame.positionMs,
        );
        if (pendingReload) {
          if (pendingReload.acknowledgedEpoch === frame.epoch) {
            completeReload();
          } else if (
            pendingReload.acknowledgedEpoch === null &&
            (pendingReload.baselineEpoch === null ||
              frame.epoch > pendingReload.baselineEpoch)
          ) {
            pendingReload.frameBeforeAcknowledgement = frame.epoch;
          }
        }
      },
    });
    client.connect();
  }

  $effect(() => {
    if (available !== true || connectedSong === songName) return;
    createClient();
  });

  function retryConnection() {
    if (unavailable) return;
    error = "";
    connectionLevelState = null;
    busy = false;
    pendingReload = null;
    createClient();
  }

  function run(command: () => void) {
    error = "";
    connectionLevelState = null;
    busy = true;
    try {
      command();
    } catch (reason) {
      busy = false;
      error =
        reason instanceof Error
          ? reason.message
          : "Video preview command failed.";
    }
  }

  function reload() {
    run(() => {
      const requestId = client?.start(config, positionMs, false);
      if (!requestId) throw new Error("Video preview is not connected.");
      hasStarted = true;
      pendingReload = {
        requestId,
        baselineEpoch: acceptedEpoch,
        acknowledgedEpoch: null,
        frameBeforeAcknowledgement: null,
      };
    });
  }

  function seek() {
    run(() => client?.seek(positionMs));
  }

  function formatTime(milliseconds: number): string {
    const value = Math.max(0, Math.round(milliseconds));
    const minutes = Math.floor(value / 60_000);
    const seconds = Math.floor((value % 60_000) / 1000);
    const millis = value % 1000;
    return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(millis).padStart(3, "0")}`;
  }

  async function toggleFullscreen() {
    if (!previewElement || !document.fullscreenEnabled) {
      error = "Fullscreen is not supported by this browser.";
      return;
    }
    try {
      if (document.fullscreenElement === previewElement)
        await document.exitFullscreen();
      else await previewElement.requestFullscreen();
    } catch (reason) {
      error =
        reason instanceof Error
          ? reason.message
          : "Unable to enter fullscreen.";
    }
  }

  function fullscreenChanged() {
    fullscreen = document.fullscreenElement === previewElement;
  }

  $effect(() => {
    document.addEventListener("fullscreenchange", fullscreenChanged);
    return () =>
      document.removeEventListener("fullscreenchange", fullscreenChanged);
  });

  onDestroy(() => {
    clientGeneration += 1;
    client?.destroy();
    client = null;
    if (document.fullscreenElement === previewElement)
      void document.exitFullscreen().catch(() => {});
  });
</script>

<section
  class="preview"
  data-testid="video-preview"
  aria-label="Editor video preview"
>
  <div class="heading">
    <div>
      <strong>Editor preview</strong>
      <span class="separate">Separate from program output</span>
    </div>
    <div class="badges" aria-live="polite">
      {#if stale}<span class="badge stale" data-testid="video-preview-stale"
          >Stale</span
        >{/if}
      <span class="badge" data-testid="video-preview-status">
        {connection === "connecting"
          ? "Connecting"
          : busy
            ? "Loading"
            : (connectionLevelState ?? playback)}
      </span>
    </div>
  </div>

  <div
    class="viewport"
    class:fullscreen
    bind:this={previewElement}
    data-testid="video-preview-viewport"
  >
    {#if frameUrl}
      <img
        src={frameUrl}
        alt="Composed editor video preview"
        data-testid="video-preview-image"
      />
    {:else}
      <div class="placeholder">
        {connection === "connecting"
          ? "Connecting preview…"
          : "Reload to render a frame"}
      </div>
    {/if}
    <button
      type="button"
      class="btn btn-sm fullscreen-button"
      aria-label={fullscreen
        ? "Exit fullscreen video preview"
        : "Fullscreen video preview"}
      disabled={!document.fullscreenEnabled}
      onclick={toggleFullscreen}
      >{fullscreen ? "Exit fullscreen" : "Fullscreen"}</button
    >
  </div>

  {#if unavailable}
    <div class="notice" data-testid="video-preview-unavailable">
      Video preview is unavailable in this server build.
    </div>
  {:else if available === null}
    <div class="notice">Checking video preview capability…</div>
  {/if}
  {#if error}
    <div class="error" role="alert" data-testid="video-preview-error">
      {error}
    </div>
  {/if}
  {#if connection === "disconnected" && !unavailable}
    <button
      type="button"
      class="btn btn-sm retry-button"
      data-testid="video-preview-retry"
      onclick={retryConnection}>Retry preview connection</button
    >
  {/if}

  <div class="transport">
    <div class="buttons">
      <button
        type="button"
        class="btn btn-sm btn-primary"
        data-testid="video-preview-start"
        disabled={controlsDisabled}
        onclick={reload}>{hasStarted ? "Reload" : "Start"}</button
      >
      <button
        type="button"
        class="btn btn-sm"
        data-testid="video-preview-play"
        disabled={controlsDisabled || !hasStarted || playback === "playing"}
        onclick={() => run(() => client?.play())}>Play</button
      >
      <button
        type="button"
        class="btn btn-sm"
        data-testid="video-preview-pause"
        disabled={controlsDisabled || !hasStarted || playback !== "playing"}
        onclick={() => run(() => client?.pause())}>Pause</button
      >
      <button
        type="button"
        class="btn btn-sm"
        data-testid="video-preview-stop"
        disabled={controlsDisabled || !hasStarted || playback === "stopped"}
        onclick={() => run(() => client?.stop())}>Stop</button
      >
    </div>
    <div class="scrub">
      <label for="video-preview-position">Video preview position</label>
      <input
        id="video-preview-position"
        data-testid="video-preview-seek"
        type="range"
        min="0"
        max={effectiveDuration}
        step="1"
        bind:value={positionMs}
        disabled={controlsDisabled || !hasStarted}
        onchange={seek}
      />
      <output data-testid="video-preview-time" for="video-preview-position">
        {formatTime(positionMs)} / {formatTime(durationMs)}
      </output>
    </div>
  </div>
</section>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--card-border);
    border-radius: var(--nc-radius-md);
    background: var(--nc-bg-2);
  }
  .heading,
  .transport,
  .buttons,
  .badges,
  .scrub {
    display: flex;
    align-items: center;
  }
  .heading,
  .transport {
    justify-content: space-between;
    gap: 12px;
  }
  .heading strong {
    color: var(--nc-fg-1);
    font: 600 14px var(--nc-font-display);
  }
  .separate {
    margin-left: 8px;
    color: var(--nc-fg-3);
    font-size: 11px;
  }
  .badges,
  .buttons {
    gap: 6px;
  }
  .badge {
    padding: 3px 7px;
    border: 1px solid var(--card-border);
    border-radius: 999px;
    color: var(--nc-fg-3);
    font: 10px var(--nc-font-mono);
    text-transform: uppercase;
  }
  .badge.stale {
    border-color: var(--nc-warn);
    color: var(--nc-warn);
  }
  .viewport {
    position: relative;
    display: grid;
    width: 100%;
    aspect-ratio: 16 / 9;
    place-items: center;
    overflow: hidden;
    background-color: #15171a;
    background-image:
      linear-gradient(45deg, #222 25%, transparent 25%),
      linear-gradient(-45deg, #222 25%, transparent 25%),
      linear-gradient(45deg, transparent 75%, #222 75%),
      linear-gradient(-45deg, transparent 75%, #222 75%);
    background-position:
      0 0,
      0 8px,
      8px -8px,
      -8px 0;
    background-size: 16px 16px;
  }
  .viewport:fullscreen {
    width: 100vw;
    height: 100vh;
    aspect-ratio: auto;
  }
  .fullscreen-button {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 1;
    opacity: 0.85;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .placeholder {
    color: #aaa;
    font-size: 13px;
  }
  .notice,
  .error {
    padding: 8px 10px;
    border-radius: 5px;
    font-size: 12px;
  }
  .notice {
    background: var(--nc-bg-3);
    color: var(--nc-fg-2);
  }
  .error {
    border: 1px solid color-mix(in srgb, var(--nc-error) 45%, transparent);
    background: color-mix(in srgb, var(--nc-error) 10%, transparent);
    color: var(--nc-error);
  }
  .scrub {
    flex: 1;
    justify-content: flex-end;
    gap: 8px;
    min-width: 240px;
  }
  .scrub label {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .scrub input {
    flex: 1;
    min-width: 100px;
  }
  output {
    min-width: 132px;
    color: var(--nc-fg-2);
    font: 11px var(--nc-font-mono);
    text-align: right;
  }
  @media (max-width: 720px) {
    .separate {
      display: block;
      margin: 2px 0 0;
    }
    .transport {
      align-items: stretch;
      flex-direction: column;
    }
    .buttons {
      display: grid;
      grid-template-columns: repeat(3, 1fr);
    }
    .buttons button {
      min-height: 44px;
    }
    .fullscreen-button {
      min-width: 44px;
      min-height: 44px;
    }
    .scrub {
      min-width: 0;
      width: 100%;
    }
  }
</style>
