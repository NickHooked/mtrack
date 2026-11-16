<script lang="ts">
  import type { VideoCanvas, VideoClip } from "../../lib/video/types";
  let {
    clip,
    canvas,
    selectedMask = -1,
    onpatch,
  }: {
    clip: VideoClip;
    canvas: VideoCanvas;
    selectedMask?: number;
    onpatch: (patch: Partial<VideoClip>) => void;
  } = $props();
  let svg: SVGSVGElement;
  type Box = { x: number; y: number; width: number; height: number };
  let draft = $state<Box | null>(null);
  let drag: { mode: string; point: { x: number; y: number }; box: Box } | null =
    null;
  const corners = ["nw", "ne", "sw", "se"];
  let clipBox = $derived({
    x: clip.x ?? 0,
    y: clip.y ?? 0,
    width: clip.width ?? canvas.width,
    height: clip.height ?? canvas.height,
  });
  let mask = $derived(clip.masks?.[selectedMask]);
  let box = $derived(
    draft ??
      (mask
        ? {
            x: clipBox.x + mask.x * clipBox.width,
            y: clipBox.y + mask.y * clipBox.height,
            width: mask.width * clipBox.width,
            height: mask.height * clipBox.height,
          }
        : clipBox),
  );
  let handle = $derived(Math.max(canvas.width, canvas.height) / 65);
  function point(event: PointerEvent) {
    return new DOMPoint(event.clientX, event.clientY).matrixTransform(
      svg.getScreenCTM()!.inverse(),
    );
  }
  function start(event: PointerEvent, mode: string) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    drag = { mode, point: point(event), box: { ...box } };
    svg.setPointerCapture(event.pointerId);
  }
  function bounded(b: Box): Box {
    if (mask) {
      const width = Math.min(
        clipBox.width,
        Math.max(clipBox.width / 1000, b.width),
      );
      const height = Math.min(
        clipBox.height,
        Math.max(clipBox.height / 1000, b.height),
      );
      return {
        x: Math.max(
          clipBox.x,
          Math.min(clipBox.x + clipBox.width - width, b.x),
        ),
        y: Math.max(
          clipBox.y,
          Math.min(clipBox.y + clipBox.height - height, b.y),
        ),
        width,
        height,
      };
    }
    const width = Math.min(4096, Math.max(1, Math.round(b.width)));
    const height = Math.min(
      4096,
      Math.floor(8847360 / width),
      Math.max(1, Math.round(b.height)),
    );
    return {
      x: Math.max(-4096, Math.min(4096, Math.round(b.x))),
      y: Math.max(-4096, Math.min(4096, Math.round(b.y))),
      width,
      height,
    };
  }
  function move(event: PointerEvent) {
    if (!drag) return;
    const p = point(event),
      dx = p.x - drag.point.x,
      dy = p.y - drag.point.y,
      b = { ...drag.box };
    if (drag.mode === "move") {
      b.x += dx;
      b.y += dy;
    } else {
      const west = drag.mode.includes("w"),
        north = drag.mode.includes("n");
      b.width = Math.max(1, b.width + (west ? -dx : dx));
      b.height = Math.max(1, b.height + (north ? -dy : dy));
      if (event.shiftKey) {
        const sx = b.width / drag.box.width,
          sy = b.height / drag.box.height;
        const scale = Math.abs(sx - 1) >= Math.abs(sy - 1) ? sx : sy;
        b.width = drag.box.width * scale;
        b.height = drag.box.height * scale;
      }
      if (west) b.x = drag.box.x + drag.box.width - b.width;
      if (north) b.y = drag.box.y + drag.box.height - b.height;
    }
    draft = bounded(b);
  }
  function commit(b: Box) {
    if (mask) {
      const normalized = (v: number) => Math.round(v * 1000000) / 1000000;
      onpatch({
        masks: clip.masks?.map((m, i) =>
          i === selectedMask
            ? {
                ...m,
                x: normalized((b.x - clipBox.x) / clipBox.width),
                y: normalized((b.y - clipBox.y) / clipBox.height),
                width: normalized(b.width / clipBox.width),
                height: normalized(b.height / clipBox.height),
              }
            : m,
        ),
      });
    } else onpatch(b);
  }
  function finish() {
    if (drag && draft) commit(draft);
    draft = null;
    drag = null;
  }
  function keys(event: KeyboardEvent, resize = false) {
    const directions: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const d = directions[event.key];
    if (!d) return;
    event.preventDefault();
    const step = event.shiftKey ? 10 : 1;
    commit(
      bounded({
        ...box,
        [resize ? "width" : "x"]: box[resize ? "width" : "x"] + d[0] * step,
        [resize ? "height" : "y"]: box[resize ? "height" : "y"] + d[1] * step,
      }),
    );
  }
</script>

<svg
  bind:this={svg}
  viewBox={`0 0 ${canvas.width} ${canvas.height}`}
  preserveAspectRatio="xMidYMid meet"
  class="overlay"
  role="group"
  aria-label="Canvas transform controls"
  onpointermove={move}
  onpointerup={finish}
  onpointercancel={() => {
    draft = null;
    drag = null;
  }}
>
  {#if mask?.shape === "ellipse"}<ellipse
      cx={box.x + box.width / 2}
      cy={box.y + box.height / 2}
      rx={box.width / 2}
      ry={box.height / 2}
      fill="none"
      stroke="#ffb74d"
      vector-effect="non-scaling-stroke"
    />{/if}
  <rect
    x={box.x}
    y={box.y}
    width={box.width}
    height={box.height}
    fill="transparent"
    stroke={mask ? "#ffb74d" : "#20d6e8"}
    stroke-dasharray={mask ? "5 3" : undefined}
    vector-effect="non-scaling-stroke"
    role="button"
    tabindex="0"
    aria-label={mask ? "Move selected mask" : "Move selected clip on canvas"}
    data-testid="video-canvas-move"
    onpointerdown={(e) => start(e, "move")}
    onkeydown={(e) => keys(e)}
  />
  {#each corners as corner (corner)}
    <rect
      x={box.x + (corner.includes("e") ? box.width : 0) - handle / 2}
      y={box.y + (corner.includes("s") ? box.height : 0) - handle / 2}
      width={handle}
      height={handle}
      fill={mask ? "#ffb74d" : "#20d6e8"}
      stroke="#111"
      vector-effect="non-scaling-stroke"
      role="button"
      tabindex="0"
      aria-label={`Resize ${mask ? "mask" : "clip"} ${corner}`}
      data-testid={`video-canvas-resize-${corner}`}
      onpointerdown={(e) => start(e, corner)}
      onkeydown={(e) => keys(e, true)}
    />
  {/each}
</svg>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    overflow: hidden;
  }
  rect {
    pointer-events: auto;
    touch-action: none;
    cursor: move;
  }
  rect[data-testid*="resize"] {
    cursor: nwse-resize;
  }
  rect:focus {
    stroke: white;
    stroke-width: 2px;
  }
</style>
