<script lang="ts">
  import type {
    VideoTrack,
    VideoClip,
    VideoCanvas,
  } from "../../lib/video/types";
  import VideoAutomation from "./VideoAutomation.svelte";
  let {
    track,
    canvas,
    selectedClipId,
    pixelsPerSecond,
    timelineWidth,
    position,
    onselectclip,
    onpatch,
    onseek,
  }: {
    track: VideoTrack;
    canvas: VideoCanvas;
    selectedClipId: string | null;
    pixelsPerSecond: number;
    timelineWidth: number;
    position: number;
    onselectclip: (id: string) => void;
    onpatch: (clip: VideoClip, patch: Partial<VideoClip>) => void;
    onseek: (time: number) => void;
  } = $props();
  let path = $state("/opacity");
  let activeId = $state<string | null>(null);
  let clip = $derived(
    track.clips.find((c) => c.id === selectedClipId) ??
      track.clips.find((c) => c.id === activeId) ??
      track.clips[0],
  );
</script>

{#if clip}
  <details
    class="track-automation"
    data-testid="video-track-automation"
    open={track.clips.some((c) => Object.keys(c.automation ?? {}).length > 0)}
  >
    <summary title="Show automation lanes for this track"
      >Automation · {track.name}</summary
    >
    {#key clip.id}<VideoAutomation
        {clip}
        bind:path
        {canvas}
        {pixelsPerSecond}
        {timelineWidth}
        {position}
        trackClips={track.clips}
        onselectclip={(id) => {
          activeId = id;
          onselectclip(id);
        }}
        onpatch={(patch) => onpatch(clip, patch)}
        {onseek}
      />{/key}
  </details>
{/if}

<style>
  .track-automation {
    min-width: max-content;
    border-bottom: 1px solid var(--card-border);
  }
  summary {
    position: sticky;
    left: 0;
    width: fit-content;
    max-width: 300px;
    padding: 5px 10px;
    font-size: 11px;
    color: var(--nc-fg-3);
    cursor: pointer;
  }
</style>
