<script lang="ts">
  import type { VideoCanvas, VideoClip } from "../../lib/video/types";
  import {
    properties,
    evaluatedClip,
    propertyValue,
    valueAt,
    insertPoint,
    type AutomationPoint,
    type AutomationLane,
  } from "../../lib/video/automation.svelte";
  let {
    clip,
    path = $bindable("/opacity"),
    canvas,
    position,
    onpatch,
    onseek,
    pixelsPerSecond,
    timelineWidth,
    trackClips = [],
    onselectclip,
  }: {
    pixelsPerSecond: number;
    timelineWidth: number;
    trackClips?: VideoClip[];
    onselectclip?: (id: string) => void;
    path?: string;
    clip: VideoClip;
    canvas: VideoCanvas;
    position: number;
    onpatch: (patch: Partial<VideoClip>) => void;
    onseek: (time: number) => void;
  } = $props();
  let selected = $state(-1),
    draft = $state<AutomationPoint[] | null>(null);
  let svg: SVGSVGElement;
  let drag: {
    index: number;
    curve: boolean;
    startY: number;
    original: number;
  } | null = null;
  let choices = $derived(properties(clip, canvas)),
    property = $derived(choices.find((p) => p.path === path) ?? choices[0]),
    lane = $derived(clip.automation?.[property.path]),
    points = $derived(draft ?? lane?.points ?? []),
    local = $derived(
      Math.max(0, Math.min(clip.duration, position - clip.start)),
    );
  let width = $derived(timelineWidth);
  const height = 160,
    margin = 12;
  function x(t: number) {
    return (clip.start + t) * pixelsPerSecond;
  }
  function y(v: number) {
    return (
      height -
      margin -
      ((v - property.min) / (property.max - property.min)) *
        (height - 2 * margin)
    );
  }
  function pointer(e: PointerEvent | MouseEvent) {
    return new DOMPoint(e.clientX, e.clientY).matrixTransform(
      svg.getScreenCTM()!.inverse(),
    );
  }
  function write(next: AutomationLane) {
    onpatch({ automation: { ...clip.automation, [property.path]: next } });
  }
  function enable() {
    const v = propertyValue(evaluatedClip(clip, canvas, local), property.path);
    write({
      enabled: !lane?.enabled,
      points: lane?.points.length
        ? lane.points
        : [
            { time: 0, value: v, curve: 0, hold: false },
            { time: clip.duration, value: v, curve: 0, hold: false },
          ],
    });
  }
  function add(e?: MouseEvent) {
    const p = e ? pointer(e) : null;
    if (
      p &&
      (p.x / pixelsPerSecond < clip.start ||
        p.x / pixelsPerSecond > clip.start + clip.duration)
    )
      return;
    const t = p
        ? Math.max(
            0,
            Math.min(clip.duration, p.x / pixelsPerSecond - clip.start),
          )
        : local,
      v = p
        ? Math.max(
            property.min,
            Math.min(
              property.max,
              property.max -
                ((p.y - margin) / (height - 2 * margin)) *
                  (property.max - property.min),
            ),
          )
        : propertyValue(evaluatedClip(clip, canvas, local), property.path);
    write(insertPoint(lane ?? { enabled: true, points: [] }, t, v));
  }
  function start(e: PointerEvent, index: number, curve = false) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    selected = index;
    drag = {
      index,
      curve,
      startY: pointer(e).y,
      original: points[index].curve ?? 0,
    };
    svg.setPointerCapture(e.pointerId);
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const p = pointer(e),
      next = points.map((p) => ({ ...p })),
      a = next[drag.index];
    if (drag.curve)
      a.curve = Math.max(
        -1,
        Math.min(1, drag.original + (p.y - drag.startY) / 70),
      );
    else {
      a.time = Math.max(
        drag.index ? next[drag.index - 1].time + 0.001 : 0,
        Math.min(
          drag.index < next.length - 1
            ? next[drag.index + 1].time - 0.001
            : clip.duration,
          p.x / pixelsPerSecond - clip.start,
        ),
      );
      a.value = Math.max(
        property.min,
        Math.min(
          property.max,
          property.max -
            ((p.y - margin) / (height - 2 * margin)) *
              (property.max - property.min),
        ),
      );
    }
    draft = next;
  }
  function finish() {
    if (draft) write({ enabled: lane?.enabled ?? true, points: draft });
    draft = null;
    drag = null;
  }
  function remove(i: number) {
    write({
      enabled: lane?.enabled ?? true,
      points: points.filter((_, j) => i !== j),
    });
    selected = -1;
  }
  function edit(patch: Partial<AutomationPoint>) {
    if (selected < 0) return;
    const next = points
      .map((p, i) => (i === selected ? { ...p, ...patch } : p))
      .sort((a, b) => a.time - b.time);
    if (
      next.some(
        (p, i) =>
          p.time < 0 ||
          p.time > clip.duration ||
          (i > 0 && p.time <= next[i - 1].time),
      )
    )
      return;
    write({ enabled: lane?.enabled ?? true, points: next });
  }
  let baseline = $derived(
    propertyValue(evaluatedClip(clip, canvas, local), property.path),
  );
  let line = $derived(
    Array.from({ length: 401 }, (_, i) => {
      const t = (i / 400) * clip.duration;
      return `${i ? "L" : "M"}${x(t)},${y(valueAt({ enabled: true, points }, t) ?? baseline)}`;
    }).join(" "),
  );
</script>

<section
  class="automation"
  data-testid="video-automation"
  aria-label="Video automation"
>
  <header>
    <span class="clip-label" title={clip.id}>{clip.id}</span>
    <select
      aria-label="Automation property"
      bind:value={path}
      onchange={() => {
        selected = -1;
        draft = null;
      }}
      >{#each choices as p (p.path)}<option value={p.path}>{p.label}</option
        >{/each}</select
    ><button
      class="btn btn-sm"
      aria-pressed={lane?.enabled ?? false}
      title="Read plays saved automation. Off uses the static clip value."
      onclick={enable}>{lane?.enabled ? "Read" : "Off"}</button
    ><button
      class="btn btn-sm"
      title="Add at the playhead. Double-click the graph to draw points; drag segment handles to bend curves."
      onclick={() => add()}>＋ Point</button
    ><span
      >{clip.start.toFixed(2)}–{(clip.start + clip.duration).toFixed(2)}s</span
    >
  </header>
  <svg
    bind:this={svg}
    style:width="{width}px"
    viewBox={`0 0 ${width} ${height}`}
    preserveAspectRatio="none"
    role="group"
    aria-label="Automation curve"
    data-testid="automation-curve"
    ondblclick={add}
    onpointermove={move}
    onpointerup={finish}
    onpointercancel={() => {
      draft = null;
      drag = null;
    }}
  >
    {#each [0, 0.25, 0.5, 0.75, 1] as v (v)}<line
        x1={margin}
        x2={width - margin}
        y1={margin + v * (height - 2 * margin)}
        y2={margin + v * (height - 2 * margin)}
        class="grid"
      />{/each}
    {#each trackClips.filter((c) => c.id !== clip.id) as other (other.id)}
      <g
        role="button"
        tabindex="0"
        aria-label={`Edit automation ${other.id}`}
        onclick={() => onselectclip?.(other.id)}
        onkeydown={(e) => {
          if (e.key === "Enter") onselectclip?.(other.id);
        }}
      >
        <title>{other.id}</title>
        <rect
          x={other.start * pixelsPerSecond}
          y="0"
          width={other.duration * pixelsPerSecond}
          {height}
          fill="#20d6e8"
          fill-opacity="0.04"
        />
        <path
          d={Array.from({ length: 101 }, (_, i) => {
            const t = (i / 100) * other.duration;
            const v =
              valueAt(other.automation?.[property.path], t) ??
              propertyValue(evaluatedClip(other, canvas, t), property.path);
            return `${i ? "L" : "M"}${(other.start + t) * pixelsPerSecond},${y(v)}`;
          }).join(" ")}
          fill="none"
          stroke="#7da3a7"
          stroke-width="2"
        />
      </g>
    {/each}
    <rect
      x={x(0)}
      y="0"
      width={clip.duration * pixelsPerSecond}
      {height}
      fill="#20d6e8"
      fill-opacity="0.06"
      pointer-events="none"
    />
    <path
      d={line}
      fill="none"
      stroke={lane?.enabled ? "#20d6e8" : "#888"}
      stroke-width="2"
      vector-effect="non-scaling-stroke"
    /><line
      x1={position * pixelsPerSecond}
      x2={position * pixelsPerSecond}
      y1="0"
      y2={height}
      stroke="#fff"
      opacity="0.5"
    />
    {#each points as p, i (i)}<circle
        cx={x(p.time)}
        cy={y(p.value)}
        r="5"
        fill={selected === i ? "#ffb74d" : "#20d6e8"}
        role="button"
        tabindex="0"
        aria-label={`Automation point ${i + 1}`}
        data-testid={`automation-point-${i}`}
        onpointerdown={(e) => start(e, i)}
        onkeydown={(e) => {
          if (e.key === "Delete" || e.key === "Backspace") {
            e.preventDefault();
            remove(i);
          }
          if (e.key === "Enter") {
            selected = i;
            onseek(clip.start + p.time);
          }
        }}
      />
      {#if i < points.length - 1}<rect
          x={x((p.time + points[i + 1].time) / 2) - 4}
          y={y(
            valueAt(
              { enabled: true, points },
              (p.time + points[i + 1].time) / 2,
            )!,
          ) - 4}
          width="8"
          height="8"
          fill="#ffb74d"
          role="button"
          tabindex="0"
          aria-label={`Curve segment ${i + 1}`}
          onpointerdown={(e) => start(e, i, true)}
          onkeydown={(e) => {
            if (e.key === "Enter") selected = i;
          }}
        />{/if}{/each}
  </svg>
  {#if selected >= 0 && points[selected]}<div class="point-fields">
      <label
        >Time<input
          aria-label="Automation point time"
          type="number"
          min="0"
          max={clip.duration}
          step="0.01"
          value={points[selected].time}
          onchange={(e) => edit({ time: Number(e.currentTarget.value) })}
        /></label
      ><label
        >Value<input
          aria-label="Automation point value"
          type="number"
          min={property.min}
          max={property.max}
          step="0.01"
          value={points[selected].value}
          onchange={(e) =>
            edit({
              value: Math.max(
                property.min,
                Math.min(property.max, Number(e.currentTarget.value)),
              ),
            })}
        /></label
      ><label
        >Curve<input
          aria-label="Automation curvature"
          type="range"
          min="-1"
          max="1"
          step="0.01"
          value={points[selected].curve}
          onchange={(e) => edit({ curve: Number(e.currentTarget.value) })}
        /></label
      ><label
        ><input
          type="checkbox"
          checked={points[selected].hold}
          onchange={(e) => edit({ hold: e.currentTarget.checked })}
        />Hold</label
      ><button class="btn btn-sm" onclick={() => remove(selected)}
        >Delete point</button
      >
    </div>{/if}
</section>

<style>
  .automation {
    display: grid;
    grid-template-columns: 150px 1fr;
    min-width: max-content;
    border-bottom: 1px solid var(--card-border);
    background: var(--nc-bg-2);
  }
  header {
    position: sticky;
    left: 0;
    z-index: 2;
    background: var(--nc-bg-2);
    padding: 8px;
    width: 150px;
    box-sizing: border-box;
    display: flex;
    flex-wrap: wrap;
    align-content: start;
    gap: 6px;
  }
  header select,
  .clip-label {
    width: 100%;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .point-fields {
    grid-column: 2;
    padding: 6px;
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  header span {
    font-size: 11px;
    color: var(--nc-fg-3);
  }
  svg {
    display: block;
    height: 160px;
    overflow: visible;
    touch-action: none;
    background: var(--nc-bg-1);
  }
  .grid {
    stroke: var(--card-border);
    stroke-width: 1;
  }
  circle {
    cursor: move;
  }
  rect {
    cursor: ns-resize;
  }
  .point-fields label {
    display: flex;
    gap: 5px;
    align-items: center;
    font-size: 12px;
  }
  .point-fields input[type="number"] {
    width: 85px;
  }
  select,
  input {
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    border: 1px solid var(--card-border);
    padding: 4px;
    border-radius: 4px;
  }
</style>
