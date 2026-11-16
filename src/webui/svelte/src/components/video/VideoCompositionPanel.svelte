<script lang="ts">
  import type {
    VideoCanvas,
    VideoClip,
    VideoMask,
  } from "../../lib/video/types";
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
  const fields = [
    "x",
    "y",
    "width",
    "height",
    "opacity",
    "fade_in",
    "fade_out",
  ] as const;
  const labels = {
    x: "X (px)",
    y: "Y (px)",
    width: "Width (px)",
    height: "Height (px)",
    opacity: "Opacity (0–1)",
    fade_in: "Fade in (s)",
    fade_out: "Fade out (s)",
  };
  function value(field: (typeof fields)[number]) {
    return (
      clip[field] ??
      (field === "width"
        ? canvas.width
        : field === "height"
          ? canvas.height
          : field === "opacity"
            ? 1
            : 0)
    );
  }
  function addMask(shape: VideoMask["shape"]) {
    drawingMask = false;
    const masks = [
      ...(clip.masks ?? []),
      {
        shape,
        x: 0.1,
        y: 0.1,
        width: 0.8,
        height: 0.8,
        feather: 20,
        invert: false,
        opacity: 1,
      },
    ];
    onpatch({ masks });
    selectedMask = masks.length - 1;
  }
  function patchMask(index: number, patch: Partial<VideoMask>) {
    onpatch({
      masks: clip.masks?.map((m, i) => (i === index ? { ...m, ...patch } : m)),
    });
  }
</script>

<section aria-label="Canvas composition" data-testid="video-composition">
  <header>
    <strong>Canvas & masks</strong><button
      class="btn btn-sm"
      onclick={() => {
        selectedMask = -1;
        drawingMask = false;
      }}>Move / resize clip</button
    ><button
      class="btn btn-sm"
      onclick={() =>
        onpatch({ x: 0, y: 0, width: canvas.width, height: canvas.height })}
      >Fill canvas</button
    >
  </header>
  <p>
    Select a clip on the timeline, then drag its outline or corner handles in
    the preview. Shift keeps the aspect ratio. Masks move with the clip.
  </p>
  <div class="actions">
    <button
      class="btn btn-sm"
      disabled={(clip.masks?.length ?? 0) >= 8}
      onclick={() => {
        drawingMask = !drawingMask;
      }}>{drawingMask ? "Cancel drawing" : "Draw freeform mask"}</button
    >
    {#if drawingMask}<p>
        Click points in the preview. Click the first point or press Enter to
        close (3–64 points). Escape cancels.
      </p>{/if}
    <button
      class="btn btn-sm"
      disabled={(clip.masks?.length ?? 0) >= 8}
      onclick={() => addMask("rectangle")}>Add rectangle mask</button
    ><button
      class="btn btn-sm"
      disabled={(clip.masks?.length ?? 0) >= 8}
      onclick={() => addMask("ellipse")}>Add ellipse mask</button
    >
  </div>
  {#each clip.masks ?? [] as mask, index (index)}
    <div class="mask" class:active={selectedMask === index}>
      <header>
        <button
          class="btn btn-sm"
          aria-pressed={selectedMask === index}
          onclick={() => {
            selectedMask = index;
            drawingMask = false;
          }}>Edit mask {index + 1} · {mask.shape}</button
        ><button
          class="btn btn-sm"
          aria-label={`Remove mask ${index + 1}`}
          onclick={() => {
            onpatch({ masks: clip.masks?.filter((_, i) => i !== index) });
            selectedMask = -1;
          }}>Remove</button
        >
      </header>
      {#if mask.shape === "polygon"}<p>
          Drag points to reshape. Arrow keys nudge; Delete removes a focused
          point (minimum 3).
        </p>{/if}
      <div class="fields">
        {#each ["x", "y", "width", "height"] as field (field)}
          <label
            >Mask {field} (%)<input
              type="number"
              step="0.1"
              min="0"
              max="100"
              value={Math.round(mask[field as "x"] * 10000) / 100}
              onchange={(e) =>
                patchMask(index, {
                  [field]: Number(e.currentTarget.value) / 100,
                })}
            /></label
          >
        {/each}
        <label
          >Feather (px)<input
            type="number"
            min="0"
            max="4096"
            value={mask.feather}
            data-testid="video-mask-feather"
            onchange={(e) =>
              patchMask(index, { feather: Number(e.currentTarget.value) })}
          /></label
        >
        <label
          >Mask strength (0–1)<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            value={mask.opacity}
            onchange={(e) =>
              patchMask(index, { opacity: Number(e.currentTarget.value) })}
          /></label
        >
        <label class="invert"
          ><input
            type="checkbox"
            checked={mask.invert}
            onchange={(e) =>
              patchMask(index, { invert: e.currentTarget.checked })}
          />Invert (cut a hole)</label
        >
      </div>
    </div>
  {/each}
  <strong>Clip transform</strong>
  <div class="fields">
    {#each fields as field (field)}
      <label
        >{labels[field]}<input
          type="number"
          step={field === "opacity" || field.startsWith("fade") ? 0.01 : 1}
          value={value(field)}
          data-testid={`video-composition-${field}`}
          onchange={(e) => onpatch({ [field]: Number(e.currentTarget.value) })}
        /></label
      >
    {/each}
  </div>
  {#if clip.masks?.length}<p>
      Masks keep the inside of their shape; invert cuts a hole. Feather softens
      inward in output pixels. Multiple masks intersect; strength 0 disables a
      mask.
    </p>{/if}
</section>

<style>
  section {
    padding: 12px;
    border: 1px solid var(--card-border);
    border-radius: 6px;
    background: var(--nc-bg-2);
  }
  header,
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  header strong {
    margin-right: auto;
  }
  p {
    font-size: 12px;
    color: var(--nc-fg-3);
    line-height: 1.5;
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 8px;
    margin: 10px 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 11px;
    color: var(--nc-fg-3);
  }
  input {
    width: 100%;
    box-sizing: border-box;
    padding: 7px;
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    border: 1px solid var(--card-border);
    border-radius: 4px;
  }
  .mask {
    margin-top: 10px;
    padding: 10px;
    border: 1px solid var(--card-border);
    border-radius: 5px;
  }
  .mask.active {
    border-color: var(--nc-cyan-400);
  }
  .invert {
    flex-direction: row;
    align-items: center;
  }
  .invert input {
    width: auto;
  }
</style>
