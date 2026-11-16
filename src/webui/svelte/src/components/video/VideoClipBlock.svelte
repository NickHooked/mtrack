<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import type { VideoClip } from "../../lib/video/types";
  import {
    MIN_CLIP_DURATION,
    snapTime,
    moveClip,
    trimClipLeft,
    trimClipRight,
  } from "../../lib/video/timeline-state";

  interface Props {
    clip: VideoClip;
    pixelsPerSecond: number;
    timelineDuration: number;
    selected?: boolean;
    trackIndex: number;
    snapping?: boolean;
    snapPoints?: { time: number; id: string }[];
    onmove?: (clip: VideoClip, targetIndex: number) => void;
    onselect?: () => void;
    oncommit?: (clip: VideoClip) => void;
    ondelete?: () => void;
  }

  let {
    clip,
    pixelsPerSecond,
    timelineDuration,
    selected = false,
    trackIndex,
    snapping = true,
    snapPoints = [],
    onmove,
    onselect,
    oncommit,
    ondelete,
  }: Props = $props();

  type DragMode = "move" | "left" | "right";
  let dragMode = $state<DragMode | null>(null);
  let originX = 0;
  let originY = 0;
  let originScroll = 0;
  let scroller: HTMLElement | null = null;
  let targetIndex = $state<number | null>(null);
  let originClip: VideoClip;
  let preview = $state<VideoClip | null>(null);
  let shown = $derived(preview ?? clip);

  function rounded(value: number): number {
    return Math.round(value * 1000) / 1000;
  }

  function beginDrag(event: PointerEvent, mode: DragMode) {
    if (event.button !== 0) return;
    event.stopPropagation();
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    dragMode = mode;
    originX = event.clientX;
    originY = event.clientY;
    scroller = target.closest(".timeline-scroll");
    originScroll = scroller?.scrollLeft ?? 0;
    targetIndex = trackIndex;
    originClip = { ...clip };
    preview = { ...clip };
    onselect?.();
    event.preventDefault();
  }

  function drag(event: PointerEvent) {
    if (!dragMode || !preview) return;
    const delta =
      (event.clientX - originX + (scroller?.scrollLeft ?? 0) - originScroll) /
      pixelsPerSecond;
    const points = [
      0,
      ...snapPoints
        .filter((point) => point.id !== clip.id)
        .map((point) => point.time),
    ];
    const snap = (time: number) =>
      snapping && !event.shiftKey
        ? snapTime(time, points, pixelsPerSecond)
        : rounded(time);
    if (dragMode === "move") {
      const lane = document
        .elementFromPoint(event.clientX, event.clientY)
        ?.closest<HTMLElement>("[data-video-track-index]");
      targetIndex = lane ? Number(lane.dataset.videoTrackIndex) : null;
      const start = Math.max(0, originClip.start + delta);
      const snapped =
        snapping && !event.shiftKey
          ? snapTime(
              start,
              [
                ...points,
                ...points
                  .map((point) => point - originClip.duration)
                  .filter((point) => point >= 0),
              ],
              pixelsPerSecond,
            )
          : rounded(start);
      preview = moveClip(originClip, snapped);
    } else if (dragMode === "left") {
      preview = trimClipLeft(originClip, snap(originClip.start + delta));
    } else {
      preview = trimClipRight(
        originClip,
        snap(originClip.start + originClip.duration + delta),
      );
    }
  }

  function finishDrag(event: PointerEvent) {
    if (!dragMode || !preview) return;
    const result = preview;
    const mode = dragMode;
    dragMode = null;
    preview = null;
    if (
      Math.abs(event.clientX - originX) < 2 &&
      Math.abs(event.clientY - originY) < 2
    )
      return;
    if (mode === "move") {
      if (targetIndex !== null) onmove?.(result, targetIndex);
    } else oncommit?.(result);
  }

  function cancelDrag() {
    dragMode = null;
    preview = null;
  }

  function clipKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" || event.key === " ") {
      onselect?.();
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      const delta = event.key === "ArrowLeft" ? -0.1 : 0.1;
      onselect?.();
      oncommit?.(moveClip(clip, rounded(clip.start + delta)));
    } else if (event.key === "Delete" || event.key === "Backspace") {
      onselect?.();
      ondelete?.();
    } else {
      return;
    }
    event.preventDefault();
  }

  function trimKeydown(event: KeyboardEvent, side: "left" | "right") {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const delta = event.key === "ArrowLeft" ? -0.1 : 0.1;
    const candidate =
      side === "left"
        ? trimClipLeft(clip, rounded(clip.start + delta))
        : trimClipRight(clip, rounded(clip.start + clip.duration + delta));
    onselect?.();
    oncommit?.(candidate);
    event.stopPropagation();
    event.preventDefault();
  }
</script>

<div
  class="video-clip"
  class:selected
  class:dragging={dragMode !== null}
  data-testid="video-clip"
  data-clip-id={clip.id}
  style:left="{shown.start * pixelsPerSecond}px"
  style:width="{Math.max(14, shown.duration * pixelsPerSecond)}px"
  role="button"
  tabindex="0"
  aria-label={`Video clip ${clip.file}`}
  onfocus={onselect}
  onpointerdown={(event) => beginDrag(event, "move")}
  onpointermove={drag}
  onpointerup={finishDrag}
  onpointercancel={cancelDrag}
  onkeydown={clipKeydown}
>
  <span
    class="trim-handle trim-left"
    data-testid="video-trim-left"
    aria-label="Trim clip start"
    role="slider"
    aria-valuemin={rounded(Math.max(0, shown.start - shown.source_in))}
    aria-valuemax={rounded(shown.start + shown.duration - MIN_CLIP_DURATION)}
    aria-valuenow={rounded(shown.start)}
    tabindex="0"
    onpointerdown={(event) => beginDrag(event, "left")}
    onpointermove={drag}
    onpointerup={finishDrag}
    onpointercancel={cancelDrag}
    onkeydown={(event) => trimKeydown(event, "left")}
  ></span>
  <span class="clip-label"
    >{clip.file}{#if dragMode}<small
        >{shown.start.toFixed(2)}s{#if dragMode === "move"}
          · {targetIndex === null
            ? "outside timeline"
            : `Track ${targetIndex + 1}`}{/if}</small
      >{/if}</span
  >
  <span
    class="trim-handle trim-right"
    data-testid="video-trim-right"
    aria-label="Trim clip end"
    role="slider"
    aria-valuemin={rounded(shown.start + MIN_CLIP_DURATION)}
    aria-valuemax={rounded(timelineDuration)}
    aria-valuenow={rounded(shown.start + shown.duration)}
    tabindex="0"
    onpointerdown={(event) => beginDrag(event, "right")}
    onpointermove={drag}
    onpointerup={finishDrag}
    onpointercancel={cancelDrag}
    onkeydown={(event) => trimKeydown(event, "right")}
  ></span>
</div>

<style>
  .video-clip {
    position: absolute;
    top: 8px;
    height: 40px;
    min-width: 14px;
    display: flex;
    align-items: center;
    border: 1px solid color-mix(in srgb, var(--nc-cyan-400) 75%, transparent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--nc-cyan-500) 28%, var(--nc-bg-2));
    color: var(--nc-fg-1);
    cursor: grab;
    overflow: hidden;
    user-select: none;
    touch-action: none;
    box-sizing: border-box;
  }
  .video-clip:hover,
  .video-clip.selected {
    border-color: var(--nc-cyan-300);
    box-shadow: 0 0 0 2px
      color-mix(in srgb, var(--nc-cyan-400) 30%, transparent);
  }
  .video-clip.dragging {
    cursor: grabbing;
    z-index: 5;
  }
  .clip-label {
    min-width: 0;
    flex: 1;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--nc-font-mono);
    font-size: 11px;
    pointer-events: none;
  }
  .clip-label small {
    display: block;
    font-size: 9px;
    margin-top: 3px;
  }
  .trim-handle {
    width: 7px;
    align-self: stretch;
    flex: 0 0 7px;
    cursor: ew-resize;
    background: color-mix(in srgb, var(--nc-cyan-300) 45%, transparent);
    touch-action: none;
  }
  .trim-left {
    border-radius: 4px 0 0 4px;
  }
  .trim-right {
    border-radius: 0 4px 4px 0;
  }
</style>
