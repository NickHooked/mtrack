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
    canvas,
    position,
    onpatch,
    onseek,
  }: {
    clip: VideoClip;
    canvas: VideoCanvas;
    position: number;
    onpatch: (patch: Partial<VideoClip>) => void;
    onseek: (time: number) => void;
  } = $props();
  let path = $state("/opacity"),
    selected = $state(-1),
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
  const width = 1000,
    height = 160,
    margin = 12;
  function x(t: number) {
    return margin + (t / clip.duration) * (width - 2 * margin);
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
    const t = p
        ? Math.max(
            0,
            Math.min(
              clip.duration,
              ((p.x - margin) / (width - 2 * margin)) * clip.duration,
            ),
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
          ((p.x - margin) / (width - 2 * margin)) * clip.duration,
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
    <strong>Automation</strong><select
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
    ><span>{local.toFixed(2)}s · {clip.id}</span>
  </header>
  <svg
    bind:this={svg}
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
    {#each [0, 0.25, 0.5, 0.75, 1] as v}<line
        x1={margin}
        x2={width - margin}
        y1={margin + v * (height - 2 * margin)}
        y2={margin + v * (height - 2 * margin)}
        class="grid"
      />{/each}
    <path
      d={line}
      fill="none"
      stroke={lane?.enabled ? "#20d6e8" : "#888"}
      stroke-width="2"
      vector-effect="non-scaling-stroke"
    /><line
      x1={x(local)}
      x2={x(local)}
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
  <footer>
    <span>0s · {property.max}</span><span
      >{clip.duration.toFixed(2)}s · {property.min}</span
    >
  </footer>
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
    border: 1px solid var(--card-border);
    border-radius: 6px;
    padding: 10px;
    background: var(--nc-bg-2);
  }
  header,
  .point-fields {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  header span,
  footer {
    font-size: 11px;
    color: var(--nc-fg-3);
  }
  svg {
    width: 100%;
    height: 160px;
    margin-top: 10px;
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
  footer {
    display: flex;
    justify-content: space-between;
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
