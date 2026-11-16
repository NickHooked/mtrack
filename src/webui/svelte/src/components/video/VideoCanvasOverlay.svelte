<script lang="ts">
  import type { VideoCanvas, VideoClip } from "../../lib/video/types";
  let {
    clip,
    canvas,
    selectedMask = $bindable(-1),
    drawingMask = $bindable(false),
    onpatch,
  }: {
    clip: VideoClip;
    canvas: VideoCanvas;
    selectedMask?: number;
    drawingMask?: boolean;
    onpatch: (patch: Partial<VideoClip>) => void;
  } = $props();
  let svg: SVGSVGElement;
  type Point = { x: number; y: number };
  let drawingPoints = $state<Point[]>([]);
  let vertexDraft = $state<Point[] | null>(null);
  let vertexIndex: number | null = null;
  $effect(() => {
    void drawingMask;
    drawingPoints = [];
  });
  function closePolygon() {
    if (drawingPoints.length < 3 || (clip.masks?.length ?? 0) >= 8) return;
    const x = Math.min(...drawingPoints.map((p) => p.x)),
      y = Math.min(...drawingPoints.map((p) => p.y));
    const width = Math.max(...drawingPoints.map((p) => p.x)) - x,
      height = Math.max(...drawingPoints.map((p) => p.y)) - y;
    if (width < 0.000001 || height < 0.000001) return;
    const masks = [
      ...(clip.masks ?? []),
      {
        shape: "polygon" as const,
        x,
        y,
        width,
        height,
        points: drawingPoints.map((p) => ({
          x: (p.x - x) / width,
          y: (p.y - y) / height,
        })),
        feather: 20,
        invert: false,
        opacity: 1,
      },
    ];
    onpatch({ masks });
    selectedMask = masks.length - 1;
    drawingMask = false;
  }
  function drawPoint(event: PointerEvent) {
    if (event.button !== 0 || drawingPoints.length >= 64) return;
    const p = point(event);
    const x = (p.x - clipBox.x) / clipBox.width,
      y = (p.y - clipBox.y) / clipBox.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return;
    drawingPoints = [...drawingPoints, { x, y }];
  }
  function drawingKeys(event: KeyboardEvent) {
    if (
      !drawingMask ||
      (event.target instanceof HTMLElement &&
        ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName))
    )
      return;
    if (["Enter", "Escape", "Backspace"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
    }
    if (event.key === "Enter") closePolygon();
    if (event.key === "Escape") drawingMask = false;
    if (event.key === "Backspace") drawingPoints = drawingPoints.slice(0, -1);
  }
  function patchVertices(points: Point[]) {
    onpatch({
      masks: clip.masks?.map((m, i) =>
        i === selectedMask ? { ...m, points } : m,
      ),
    });
  }
  function vertexKeys(event: KeyboardEvent, index: number) {
    if (
      (event.key === "Delete" || event.key === "Backspace") &&
      vertices.length > 3
    ) {
      event.preventDefault();
      event.stopPropagation();
      patchVertices(vertices.filter((_, i) => i !== index));
    }
    const d: Record<string, Point> = {
      ArrowLeft: { x: -1, y: 0 },
      ArrowRight: { x: 1, y: 0 },
      ArrowUp: { x: 0, y: -1 },
      ArrowDown: { x: 0, y: 1 },
    };
    if (d[event.key]) {
      event.preventDefault();
      event.stopPropagation();
      const step = event.shiftKey ? 10 : 1;
      patchVertices(
        vertices.map((p, i) =>
          i === index
            ? {
                x: Math.max(
                  0,
                  Math.min(1, p.x + (d[event.key].x * step) / box.width),
                ),
                y: Math.max(
                  0,
                  Math.min(1, p.y + (d[event.key].y * step) / box.height),
                ),
              }
            : p,
        ),
      );
    }
  }
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
  let vertices = $derived(vertexDraft ?? mask?.points ?? []);
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
    if (vertexIndex !== null) {
      const p = point(event);
      vertexDraft = vertices.map((v, i) =>
        i === vertexIndex
          ? {
              x: Math.max(0, Math.min(1, (p.x - box.x) / box.width)),
              y: Math.max(0, Math.min(1, (p.y - box.y) / box.height)),
            }
          : v,
      );
      return;
    }
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
    if (vertexDraft) patchVertices(vertexDraft);
    vertexDraft = null;
    vertexIndex = null;
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

<svelte:window onkeydown={drawingKeys} />

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
    vertexDraft = null;
    vertexIndex = null;
  }}
>
  {#if !drawingMask}
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
        x={box.x +
          (corner.includes("e") ? box.width : 0) -
          handle / 2 +
          (mask?.shape === "polygon"
            ? corner.includes("e")
              ? handle
              : -handle
            : 0)}
        y={box.y +
          (corner.includes("s") ? box.height : 0) -
          handle / 2 +
          (mask?.shape === "polygon"
            ? corner.includes("s")
              ? handle
              : -handle
            : 0)}
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
    {#if mask?.shape === "polygon"}
      <polygon
        points={vertices
          .map((p) => `${box.x + p.x * box.width},${box.y + p.y * box.height}`)
          .join(" ")}
        fill="none"
        stroke="#ffb74d"
        vector-effect="non-scaling-stroke"
      />
      {#each vertices as p, i (i)}
        <circle
          cx={box.x + p.x * box.width}
          cy={box.y + p.y * box.height}
          r={handle / 2}
          fill="#ffb74d"
          stroke="#111"
          role="button"
          tabindex="0"
          aria-label={`Mask point ${i + 1}`}
          data-testid={`video-mask-point-${i}`}
          onkeydown={(e) => vertexKeys(e, i)}
          onpointerdown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            e.stopPropagation();
            vertexIndex = i;
            svg.setPointerCapture(e.pointerId);
          }}
        />
      {/each}
    {/if}
  {:else}
    <rect
      x="0"
      y="0"
      width={canvas.width}
      height={canvas.height}
      fill="transparent"
      role="button"
      tabindex="0"
      aria-label="Draw mask points"
      data-testid="video-mask-draw"
      onpointerdown={drawPoint}
      onkeydown={drawingKeys}
    />
    <polyline
      points={drawingPoints
        .map(
          (p) =>
            `${clipBox.x + p.x * clipBox.width},${clipBox.y + p.y * clipBox.height}`,
        )
        .join(" ")}
      fill="none"
      stroke="#ffb74d"
      vector-effect="non-scaling-stroke"
    />
    {#each drawingPoints as p, i (i)}
      <circle
        cx={clipBox.x + p.x * clipBox.width}
        cy={clipBox.y + p.y * clipBox.height}
        r={handle / 2}
        fill="#ffb74d"
        role="button"
        tabindex="0"
        aria-label={i === 0 ? "Close mask" : `Draft point ${i + 1}`}
        onpointerdown={(e) => {
          e.stopPropagation();
          if (i === 0) closePolygon();
        }}
        onkeydown={(e) => {
          if (e.key === "Enter") {
            e.stopPropagation();
            closePolygon();
          }
        }}
      />
    {/each}
  {/if}
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
  circle {
    pointer-events: auto;
    cursor: crosshair;
    touch-action: none;
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
