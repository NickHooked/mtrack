<script lang="ts">
  import { onMount } from "svelte";
  import { VIDEO_MEDIA_TYPE, liveMedia } from "../../lib/video/media";
  import type { VideoSource } from "../../lib/video/types";
  let {
    locked = false,
    onadd,
  }: { locked?: boolean; onadd: (media: string) => void } = $props();
  let devices = $state<{ name: string; device: string }[]>([]);
  let message = $state("");
  let busy = $state(false);
  let available = $state(false);
  let manualDevice = $state("");
  const testSource: VideoSource = { type: "test_pattern" };
  async function refresh() {
    busy = true;
    message = "";
    try {
      const response = await fetch("/api/video/sources");
      if (!response.ok) throw new Error("Cannot discover cameras.");
      const result = await response.json();
      devices = result.devices;
      available = result.live_capture === true;
      if (!available) message = "This build does not support live video.";
      else if (!devices.length)
        message =
          "No cameras detected on the Mtrack host. Connect a capture device and refresh.";
    } catch {
      message = "Camera discovery unavailable. Check the Mtrack connection.";
    } finally {
      busy = false;
    }
  }
  onMount(refresh);
  function drag(event: DragEvent, source: VideoSource) {
    if (locked || !available) {
      event.preventDefault();
      return;
    }
    event.dataTransfer?.setData(VIDEO_MEDIA_TYPE, liveMedia(source));
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "copy";
  }
</script>

<section aria-label="Live video sources">
  <header>
    <strong>Live sources</strong><button
      class="btn btn-sm"
      onclick={refresh}
      disabled={busy}>Refresh cameras</button
    >
  </header>
  <p>
    Drag a source onto the timeline, or click to append it. The clip duration
    controls when the live input is visible. Capture runs on the Mtrack host.
  </p>
  {#if message}<p role="status">{message}</p>{/if}
  {#each devices as camera (camera.device)}
    <button
      class="source"
      disabled={locked || !available}
      draggable={!locked && available}
      ondragstart={(event) =>
        drag(event, { type: "v4l2", device: camera.device })}
      onclick={() => onadd(liveMedia({ type: "v4l2", device: camera.device }))}
    >
      <span>● {camera.name}</span><small>{camera.device}</small>
    </button>
  {/each}
  <button
    class="source"
    disabled={locked || !available}
    draggable={!locked && available}
    ondragstart={(event) => drag(event, testSource)}
    onclick={() => onadd(liveMedia(testSource))}
  >
    <span>● Live test pattern</span><small>Test without capture hardware</small>
  </button>
  <details>
    <summary>Camera device path</summary>
    <label
      >Linux video device<input
        placeholder="/dev/video0"
        bind:value={manualDevice}
        disabled={locked || !available}
      /></label
    >
    <button
      class="btn btn-sm"
      disabled={locked || !available || !manualDevice.trim()}
      onclick={() =>
        onadd(liveMedia({ type: "v4l2", device: manualDevice.trim() }))}
      >Add camera</button
    >
  </details>
</section>

<style>
  section {
    padding: 12px;
    border: 1px solid var(--card-border);
    border-radius: 6px;
    background: var(--nc-bg-2);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  p,
  small,
  summary,
  label {
    font-size: 12px;
    color: var(--nc-fg-3);
  }
  p {
    line-height: 1.5;
  }
  .source {
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    width: 100%;
    padding: 10px;
    margin: 6px 0;
    border: 1px solid var(--card-border);
    border-radius: 5px;
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    cursor: grab;
  }
  .source:disabled {
    opacity: 0.5;
    cursor: default;
  }
  small {
    overflow-wrap: anywhere;
  }
  summary {
    cursor: pointer;
    margin-top: 10px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 8px 0;
  }
  input {
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    padding: 6px;
    border: 1px solid var(--card-border);
  }
</style>
