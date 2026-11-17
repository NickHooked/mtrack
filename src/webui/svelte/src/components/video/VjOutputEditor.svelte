<script lang="ts">
  function copyData<T>(value: T): T {
    return JSON.parse(JSON.stringify(value));
  }
  import { onDestroy, untrack } from "svelte";
  import {
    fullCorners,
    newOutput,
    newSlice,
    validQuad,
    type OutputCanvas,
    type OutputSlice,
    type Point,
  } from "../../lib/video/vj";
  let {
    outputs,
    tracks,
    running = false,
    locked = false,
    onchange,
  }: {
    outputs: OutputCanvas[];
    tracks: string[];
    running?: boolean;
    locked?: boolean;
    onchange: (outputs: OutputCanvas[]) => void;
  } = $props();
  let selected = $state("");
  let selectedSlice = $state("");
  let imageUrl = $state<string | null>(null);
  let socket: WebSocket | null = null;
  let output = $derived(outputs.find((o) => o.id === selected) ?? outputs[0]);
  let slice = $derived(
    output?.slices.find((s) => s.id === selectedSlice) ?? output?.slices[0],
  );
  let drag = $state<{ index: number; rect: DOMRect; points: Point[] } | null>(
    null,
  );
  let draft = $state<Point[] | null>(null);
  function clearImage() {
    if (imageUrl) URL.revokeObjectURL(imageUrl);
    imageUrl = null;
  }
  $effect(() => {
    const id = output?.id;
    const active = running;
    socket?.close();
    socket = null;
    untrack(clearImage);
    if (id && active) {
      const ws = new WebSocket(
        `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/api/video/vj/output/${encodeURIComponent(id)}/ws`,
      );
      socket = ws;
      ws.binaryType = "blob";
      ws.onmessage = (e) => {
        if (socket !== ws) return;
        if (typeof e.data === "string") {
          clearImage();
          return;
        }
        const url = URL.createObjectURL(e.data);
        clearImage();
        imageUrl = url;
      };
      return () => {
        ws.onmessage = null;
        ws.close();
      };
    }
  });
  onDestroy(() => {
    socket?.close();
    clearImage();
  });
  function patch(patch: Partial<OutputCanvas>) {
    if (output && !locked)
      onchange(
        outputs.map((o) => (o.id === output.id ? { ...o, ...patch } : o)),
      );
  }
  function patchSlice(patchValue: Partial<OutputSlice>) {
    if (output && slice)
      patch({
        slices: output.slices.map((s) =>
          s.id === slice.id ? { ...s, ...patchValue } : s,
        ),
      });
  }
  function add() {
    if (outputs.length >= 8) return;
    const value = newOutput(outputs.length + 1);
    selected = value.id;
    onchange([...outputs, value]);
  }
  function down(event: PointerEvent, index: number) {
    if (locked || !slice) return;
    event.preventDefault();
    const svg = (event.currentTarget as SVGElement).closest("svg");
    if (svg)
      drag = {
        index,
        rect: svg.getBoundingClientRect(),
        points: copyData(slice.corners),
      };
  }
  function move(event: PointerEvent) {
    if (!drag) return;
    const points = copyData(drag.points);
    points[drag.index] = {
      x: Math.max(
        -2,
        Math.min(3, (event.clientX - drag.rect.left) / drag.rect.width),
      ),
      y: Math.max(
        -2,
        Math.min(3, (event.clientY - drag.rect.top) / drag.rect.height),
      ),
    };
    if (validQuad(points)) draft = points;
  }
  function up() {
    if (draft) patchSlice({ corners: draft });
    drag = null;
    draft = null;
  }
  function nudge(event: KeyboardEvent, index: number) {
    if (
      !slice ||
      !output ||
      locked ||
      !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)
    )
      return;
    event.preventDefault();
    const points = copyData(slice.corners);
    const step = event.shiftKey ? 10 : 1;
    points[index].x +=
      (event.key === "ArrowLeft"
        ? -step
        : event.key === "ArrowRight"
          ? step
          : 0) / output.width;
    points[index].y +=
      (event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0) /
      output.height;
    if (validQuad(points)) patchSlice({ corners: points });
  }
  function crop(key: "x" | "y" | "width" | "height", value: number) {
    if (!slice) return;
    const input = { ...slice.input, [key]: value / 100 };
    input.x = Math.max(0, Math.min(input.x, 0.999));
    input.y = Math.max(0, Math.min(input.y, 0.999));
    input.width = Math.max(0.001, Math.min(input.width, 1 - input.x));
    input.height = Math.max(0.001, Math.min(input.height, 1 - input.y));
    patchSlice({ input });
  }
</script>

<svelte:window
  onpointermove={move}
  onpointerup={up}
  onpointercancel={() => {
    drag = null;
    draft = null;
  }}
/>
<section class="mapping" aria-label="Projection mapping">
  <header>
    <strong>Output & mapping</strong><button
      class="btn btn-sm"
      disabled={locked || outputs.length >= 8}
      onclick={add}>＋ Output</button
    >
  </header>
  {#if output}
    <div class="row">
      <select
        aria-label="Select output"
        value={output.id}
        onchange={(e) => {
          selected = e.currentTarget.value;
          selectedSlice = "";
        }}
        >{#each outputs as o (o.id)}<option value={o.id}>{o.name}</option
          >{/each}</select
      ><a
        class="btn btn-sm"
        href={`/video-output/${encodeURIComponent(output.id)}`}
        target="_blank"
        rel="noopener"
        title="Open this canvas in a separate window. Move it to a projector and enter fullscreen."
        >↗ Open output</a
      >
    </div>
    <div class="row">
      <input
        aria-label="Output name"
        value={output.name}
        disabled={locked}
        onchange={(e) => patch({ name: e.currentTarget.value })}
      /><button
        class="btn btn-sm"
        disabled={locked || outputs.length >= 8}
        title="Duplicate output with independent mapping"
        onclick={() => {
          if (!output) return;
          const copy = copyData(output);
          copy.id = crypto.randomUUID();
          copy.name += " copy";
          selected = copy.id;
          onchange([...outputs, copy]);
        }}>Duplicate</button
      ><button
        class="btn btn-sm danger"
        disabled={locked}
        title="Remove selected output"
        onclick={() => onchange(outputs.filter((o) => o.id !== output?.id))}
        >×</button
      >
    </div>
    <div class="row">
      <label
        >Width<input
          aria-label="Output width"
          type="number"
          min="1"
          max="4096"
          value={output.width}
          disabled={locked}
          onchange={(e) => patch({ width: Math.round(+e.currentTarget.value) })}
        /></label
      ><label
        >Height<input
          aria-label="Output height"
          type="number"
          min="1"
          max="4096"
          value={output.height}
          disabled={locked}
          onchange={(e) =>
            patch({ height: Math.round(+e.currentTarget.value) })}
        /></label
      ><label class="check"
        ><input
          type="checkbox"
          checked={output.enabled}
          disabled={locked}
          onchange={(e) => patch({ enabled: e.currentTarget.checked })}
        />Enabled</label
      >
    </div>
    <div
      class="mapping-preview"
      style:aspect-ratio={`${output.width}/${output.height}`}
    >
      <svg
        viewBox="0 0 1000 1000"
        preserveAspectRatio="none"
        aria-label="Output mapping corners"
      >
        <defs
          ><pattern
            id="vj-map-grid"
            width="100"
            height="100"
            patternUnits="userSpaceOnUse"
            ><path
              d="M 100 0 L 0 0 0 100"
              fill="none"
              stroke="#283349"
              stroke-width="1"
            /></pattern
          ></defs
        >
        <rect width="1000" height="1000" fill="url(#vj-map-grid)" />
        {#if imageUrl}<image
            href={imageUrl}
            width="1000"
            height="1000"
            preserveAspectRatio="none"
          />{/if}
        {#each output.slices as s (s.id)}<polygon
            points={(s.id === slice?.id && draft ? draft : s.corners)
              .map((p) => `${p.x * 1000},${p.y * 1000}`)
              .join(" ")}
            fill="transparent"
            stroke={s.id === slice?.id ? "#6ee7cc" : "#74829b"}
            stroke-width="3"
            vector-effect="non-scaling-stroke"
          />{/each}
        {#if slice}{#each draft ?? slice.corners as p, i (i)}<circle
              cx={p.x * 1000}
              cy={p.y * 1000}
              r="12"
              fill="#6ee7cc"
              stroke="#081a20"
              stroke-width="3"
              role="button"
              tabindex={locked ? -1 : 0}
              aria-label={`Mapping corner ${i + 1}`}
              onpointerdown={(e) => down(e, i)}
              onkeydown={(e) => nudge(e, i)}
              ><title
                >Drag corner · Arrow keys nudge · Shift for 10 pixels</title
              ></circle
            >{/each}{/if}
      </svg>
    </div>
    <div class="row">
      <select
        aria-label="Select slice"
        value={slice?.id ?? ""}
        onchange={(e) => (selectedSlice = e.currentTarget.value)}
        >{#each output.slices as s (s.id)}<option value={s.id}>{s.name}</option
          >{/each}</select
      ><button
        class="btn btn-sm"
        disabled={locked || output.slices.length >= 16}
        onclick={() => {
          if (!output) return;
          const s = newSlice(output.slices.length + 1);
          selectedSlice = s.id;
          patch({ slices: [...output.slices, s] });
        }}>＋ Slice</button
      >
    </div>
    {#if slice}
      <div class="row">
        <input
          aria-label="Slice name"
          disabled={locked}
          value={slice.name}
          onchange={(e) => patchSlice({ name: e.currentTarget.value })}
        /><button
          class="btn btn-sm"
          disabled={locked}
          title="Reset the four corners"
          onclick={() => patchSlice({ corners: fullCorners() })}>Reset</button
        ><button
          class="btn btn-sm danger"
          disabled={locked}
          title="Remove selected slice"
          onclick={() =>
            patch({ slices: output!.slices.filter((s) => s.id !== slice!.id) })}
          >×</button
        >
      </div>
      <label
        >Input source<select
          aria-label="Slice input source"
          disabled={locked}
          value={slice.source.type === "layer"
            ? `layer:${slice.source.index}`
            : "composition"}
          onchange={(e) =>
            patchSlice({
              source:
                e.currentTarget.value === "composition"
                  ? { type: "composition" }
                  : {
                      type: "layer",
                      index: +e.currentTarget.value.split(":")[1],
                    },
            })}
          ><option value="composition">Composition</option
          >{#each tracks as name, i (i)}<option value={`layer:${i}`}
              >Layer {i + 1} · {name}</option
            >{/each}</select
        ></label
      >
      <div class="quad">
        <label title="Crop the input before mapping"
          >Input X %<input
            aria-label="Slice input x"
            type="number"
            min="0"
            max="99.9"
            step=".1"
            disabled={locked}
            value={+(slice.input.x * 100).toFixed(2)}
            onchange={(e) => crop("x", +e.currentTarget.value)}
          /></label
        ><label
          >Input Y %<input
            aria-label="Slice input y"
            type="number"
            min="0"
            max="99.9"
            step=".1"
            disabled={locked}
            value={+(slice.input.y * 100).toFixed(2)}
            onchange={(e) => crop("y", +e.currentTarget.value)}
          /></label
        ><label
          >Width %<input
            aria-label="Slice input width"
            type="number"
            min=".1"
            max="100"
            step=".1"
            disabled={locked}
            value={+(slice.input.width * 100).toFixed(2)}
            onchange={(e) => crop("width", +e.currentTarget.value)}
          /></label
        ><label
          >Height %<input
            aria-label="Slice input height"
            type="number"
            min=".1"
            max="100"
            step=".1"
            disabled={locked}
            value={+(slice.input.height * 100).toFixed(2)}
            onchange={(e) => crop("height", +e.currentTarget.value)}
          /></label
        >
      </div>
      <details>
        <summary title="Soften overlapping projector edges in slice coordinates"
          >Edge blending</summary
        >
        <div class="quad">
          {#each ["Left", "Top", "Right", "Bottom"] as edge, i (edge)}<label
              >{edge} %<input
                aria-label={`Feather ${edge.toLowerCase()}`}
                type="number"
                min="0"
                max="100"
                step=".5"
                value={+(slice.feather[i] * 100).toFixed(2)}
                disabled={locked}
                onchange={(e) => {
                  const feather = [...slice!.feather];
                  feather[i] = Math.max(
                    0,
                    Math.min(1, +e.currentTarget.value / 100),
                  );
                  patchSlice({ feather });
                }}
              /></label
            >{/each}
        </div>
        <label
          >Slice opacity<input
            type="range"
            min="0"
            max="1"
            step=".01"
            value={slice.opacity}
            disabled={locked}
            oninput={(e) => patchSlice({ opacity: +e.currentTarget.value })}
          /></label
        >
      </details>
      <details>
        <summary>Corner coordinates</summary
        >{#each slice.corners as p, i (i)}<div class="row">
            <label
              >Corner {i + 1} X %<input
                type="number"
                step=".1"
                value={+(p.x * 100).toFixed(2)}
                disabled={locked}
                onchange={(e) => {
                  const points = copyData(slice!.corners);
                  points[i].x = +e.currentTarget.value / 100;
                  if (validQuad(points)) patchSlice({ corners: points });
                }}
              /></label
            ><label
              >Y %<input
                type="number"
                step=".1"
                value={+(p.y * 100).toFixed(2)}
                disabled={locked}
                onchange={(e) => {
                  const points = copyData(slice!.corners);
                  points[i].y = +e.currentTarget.value / 100;
                  if (validQuad(points)) patchSlice({ corners: points });
                }}
              /></label
            >
          </div>{/each}
      </details>
    {/if}
    <details>
      <summary>Output calibration</summary>
      <div class="row">
        <label
          >Opacity<input
            type="number"
            min="0"
            max="1"
            step=".01"
            value={output.opacity}
            disabled={locked}
            onchange={(e) => patch({ opacity: +e.currentTarget.value })}
          /></label
        ><label
          >Brightness<input
            type="number"
            min="0"
            max="2"
            step=".05"
            value={output.brightness}
            disabled={locked}
            onchange={(e) => patch({ brightness: +e.currentTarget.value })}
          /></label
        ><label
          >Gamma<input
            type="number"
            min=".1"
            max="4"
            step=".05"
            value={output.gamma}
            disabled={locked}
            onchange={(e) => patch({ gamma: +e.currentTarget.value })}
          /></label
        >
      </div>
      <div class="row">
        <label class="check"
          ><input
            type="checkbox"
            checked={output.flip_x}
            disabled={locked}
            onchange={(e) => patch({ flip_x: e.currentTarget.checked })}
          />Flip X</label
        ><label class="check"
          ><input
            type="checkbox"
            checked={output.flip_y}
            disabled={locked}
            onchange={(e) => patch({ flip_y: e.currentTarget.checked })}
          />Flip Y</label
        ><label class="check"
          ><input
            type="checkbox"
            checked={output.test_pattern}
            disabled={locked}
            onchange={(e) => patch({ test_pattern: e.currentTarget.checked })}
          />Test card</label
        >
      </div>
    </details>
  {:else}<button class="empty" disabled={locked} onclick={add}
      >＋ Create an output canvas</button
    >{/if}
</section>

<style>
  .mapping {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  header,
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  header {
    justify-content: space-between;
  }
  select,
  input {
    min-width: 0;
    width: 100%;
    background: #111822;
    color: #e3eaf4;
    border: 1px solid #354052;
    border-radius: 5px;
    padding: 6px;
    font-size: 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 11px;
    color: #9dacc2;
    flex: 1;
  }
  .row > select,
  .row > input {
    flex: 1;
  }
  .check {
    flex-direction: row;
    align-items: center;
    white-space: nowrap;
  }
  .check input {
    width: auto;
  }
  .quad {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }
  .mapping-preview {
    background: #070b11;
    border: 1px solid #384b63;
    overflow: hidden;
  }
  .mapping-preview svg {
    display: block;
    width: 100%;
    height: 100%;
  }
  circle {
    cursor: move;
    touch-action: none;
  }
  summary {
    font-size: 12px;
    cursor: pointer;
    padding: 6px 0;
    color: #bdcadb;
  }
  details[open] {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .empty {
    padding: 36px;
    background: #111822;
    color: #9bdbcd;
    border: 1px dashed #496b69;
    border-radius: 6px;
  }
  .danger {
    color: #ffa1a1;
  }
  .btn {
    white-space: nowrap;
    font-size: 11px;
  }
</style>
