<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import type { VideoClip } from "../../lib/video/types";

  interface Props {
    clip: VideoClip;
    onpatch?: (patch: Partial<VideoClip>) => void;
    ondelete?: () => void;
  }

  let { clip, onpatch, ondelete }: Props = $props();

  function numberValue(target: HTMLInputElement, fallback: number): number {
    const value = Number(target.value);
    return Number.isFinite(value) ? value : fallback;
  }

  function enterCommit(
    event: KeyboardEvent & { currentTarget: HTMLInputElement },
    commit: (target: HTMLInputElement) => void,
  ) {
    if (event.key !== "Enter") return;
    commit(event.currentTarget);
    event.currentTarget.blur();
  }
</script>

<div class="properties" data-testid="video-properties">
  <div class="properties-title">
    <strong>Selected clip</strong>
    <span>{clip.id}</span>
  </div>
  <label>
    <span>Start (s)</span>
    <input
      type="number"
      min="0"
      step="0.01"
      data-testid="video-clip-start"
      value={clip.start}
      onchange={(event) =>
        onpatch?.({ start: numberValue(event.currentTarget, clip.start) })}
      onkeydown={(event) =>
        enterCommit(event, (target) =>
          onpatch?.({ start: numberValue(target, clip.start) }),
        )}
    />
  </label>
  <label>
    <span>Duration (s)</span>
    <input
      type="number"
      min="0.01"
      step="0.01"
      data-testid="video-clip-duration"
      value={clip.duration}
      onchange={(event) =>
        onpatch?.({
          duration: numberValue(event.currentTarget, clip.duration),
        })}
      onkeydown={(event) =>
        enterCommit(event, (target) =>
          onpatch?.({ duration: numberValue(target, clip.duration) }),
        )}
    />
  </label>
  <label>
    <span>Source in (s)</span>
    <input
      type="number"
      min="0"
      step="0.01"
      data-testid="video-clip-source-in"
      value={clip.source_in}
      onchange={(event) =>
        onpatch?.({
          source_in: numberValue(event.currentTarget, clip.source_in),
        })}
      onkeydown={(event) =>
        enterCommit(event, (target) =>
          onpatch?.({ source_in: numberValue(target, clip.source_in) }),
        )}
    />
  </label>
  <label class="file-field">
    <span>File</span>
    <input
      type="text"
      data-testid="video-clip-file"
      value={clip.file}
      onchange={(event) =>
        onpatch?.({ file: event.currentTarget.value.trim() })}
      onkeydown={(event) =>
        enterCommit(event, (target) =>
          onpatch?.({ file: target.value.trim() }),
        )}
    />
  </label>
  <button
    type="button"
    class="btn btn-sm btn-danger"
    data-testid="video-delete-clip"
    onclick={ondelete}>Delete clip</button
  >
</div>

<style>
  .properties {
    display: grid;
    grid-template-columns: repeat(3, minmax(110px, 1fr)) minmax(180px, 2fr) auto;
    gap: 10px;
    align-items: end;
    padding: 12px;
    border: 1px solid var(--card-border);
    border-radius: var(--nc-radius-md);
    background: var(--nc-bg-2);
  }
  .properties-title {
    grid-column: 1 / -1;
    display: flex;
    gap: 8px;
    align-items: baseline;
    color: var(--nc-fg-1);
    font-size: 13px;
  }
  .properties-title span {
    color: var(--nc-fg-3);
    font: 11px var(--nc-font-mono);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--nc-fg-3);
    font-size: 11px;
    font-weight: 600;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    border: 1px solid var(--card-border);
    border-radius: 5px;
    padding: 7px 8px;
    background: var(--nc-bg-1);
    color: var(--nc-fg-1);
    font: 12px var(--nc-font-mono);
  }
  input:focus {
    outline: none;
    border-color: var(--nc-cyan-400);
  }
  @media (max-width: 800px) {
    .properties {
      grid-template-columns: repeat(2, minmax(120px, 1fr));
    }
    .file-field {
      grid-column: 1 / -1;
    }
  }
</style>
