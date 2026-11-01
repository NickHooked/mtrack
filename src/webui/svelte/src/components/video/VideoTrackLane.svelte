<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import type { VideoClip, VideoTrack } from "../../lib/video/types";
  import VideoClipBlock from "./VideoClipBlock.svelte";

  interface Props {
    track: VideoTrack;
    trackIndex: number;
    selectedClipId?: string | null;
    pixelsPerSecond: number;
    timelineWidth: number;
    timelineDuration: number;
    onrename?: (name: string) => void;
    onremove?: () => void;
    onselectclip?: (id: string) => void;
    oncommitclip?: (clip: VideoClip) => void;
    ondeleteclip?: (id: string) => void;
  }

  let {
    track,
    trackIndex,
    selectedClipId = null,
    pixelsPerSecond,
    timelineWidth,
    timelineDuration,
    onrename,
    onremove,
    onselectclip,
    oncommitclip,
    ondeleteclip,
  }: Props = $props();

  function commitName(target: HTMLInputElement) {
    const value = target.value.trim();
    if (value && value !== track.name) onrename?.(value);
    else target.value = track.name;
  }
</script>

<div class="video-track" data-testid="video-track">
  <div class="track-header">
    <input
      class="track-name"
      data-testid="video-track-name"
      aria-label={`Video track ${trackIndex + 1} name`}
      value={track.name}
      onblur={(event) => commitName(event.currentTarget)}
      onkeydown={(event) => {
        if (event.key === "Enter") {
          commitName(event.currentTarget);
          event.currentTarget.blur();
        }
      }}
    />
    <button
      type="button"
      class="remove-track"
      data-testid="video-remove-track"
      aria-label={`Remove video track ${track.name}`}
      onclick={onremove}>×</button
    >
  </div>
  <div class="lane" style:width="{timelineWidth}px">
    {#each track.clips as clip (clip.id)}
      <VideoClipBlock
        {clip}
        {pixelsPerSecond}
        {timelineDuration}
        selected={selectedClipId === clip.id}
        onselect={() => onselectclip?.(clip.id)}
        oncommit={oncommitclip}
        ondelete={() => ondeleteclip?.(clip.id)}
      />
    {/each}
  </div>
</div>

<style>
  .video-track {
    display: grid;
    grid-template-columns: 150px 1fr;
    min-width: max-content;
    border-bottom: 1px solid var(--card-border);
  }
  .track-header {
    position: sticky;
    left: 0;
    z-index: 8;
    width: 150px;
    min-height: 56px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px;
    box-sizing: border-box;
    background: var(--nc-bg-2);
    border-right: 1px solid var(--card-border);
  }
  .track-name {
    min-width: 0;
    width: 100%;
    padding: 6px 8px;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--nc-fg-1);
    font: 600 12px var(--nc-font-sans);
  }
  .track-name:focus {
    outline: none;
    border-color: var(--nc-cyan-400);
    background: var(--nc-bg-1);
  }
  .remove-track {
    width: 26px;
    height: 26px;
    flex: 0 0 26px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--nc-fg-3);
    cursor: pointer;
    font-size: 17px;
  }
  .remove-track:hover {
    background: color-mix(in srgb, var(--nc-error) 15%, transparent);
    color: var(--nc-error);
  }
  .lane {
    position: relative;
    height: 56px;
    background-image: linear-gradient(
      to right,
      color-mix(in srgb, var(--card-border) 55%, transparent) 1px,
      transparent 1px
    );
    background-size: 50px 100%;
  }
</style>
