<!-- Copyright (C) 2026 Michael Wilson <mike@mdwn.dev> -->
<script lang="ts">
  import { VIDEO_ACCEPT, VIDEO_MEDIA_TYPE } from "../../lib/video/media";
  interface Props {
    files: string[];
    busy?: boolean;
    locked?: boolean;
    onimport: (files: File[]) => void;
    onadd: (file: string) => void;
  }
  let {
    files,
    busy = false,
    locked = false,
    onimport,
    onadd,
  }: Props = $props();
  let query = $state("");
  let over = $state(false);
  let input: HTMLInputElement;
  let filtered = $derived(
    files.filter((file) => file.toLowerCase().includes(query.toLowerCase())),
  );
  function importFiles(list: FileList | null) {
    if (list?.length && !busy && !locked) onimport(Array.from(list));
  }
</script>

<section class="media-pool" aria-label="Video media pool">
  <header><strong>Media pool</strong><span>{files.length} files</span></header>
  <input
    class="search"
    type="search"
    aria-label="Search video media"
    placeholder="Search media…"
    bind:value={query}
  />
  <button
    type="button"
    class="import"
    class:over
    disabled={busy || locked}
    onclick={() => input.click()}
    ondragover={(event) => {
      if (event.dataTransfer?.types.includes("Files")) {
        event.preventDefault();
        over = true;
      }
    }}
    ondragleave={() => (over = false)}
    ondrop={(event) => {
      event.preventDefault();
      over = false;
      importFiles(event.dataTransfer?.files ?? null);
    }}
  >
    {busy ? "Importing…" : "＋ Import videos"}
    <small>{locked ? "Unlock to import media" : "or drop files here"}</small>
  </button>
  <input
    bind:this={input}
    hidden
    type="file"
    multiple
    accept={VIDEO_ACCEPT}
    aria-label="Import video files"
    onchange={(event) => {
      importFiles(event.currentTarget.files);
      event.currentTarget.value = "";
    }}
  />
  <div class="media-list">
    {#each filtered as file (file)}
      <button
        class="media-item"
        type="button"
        draggable={!locked}
        disabled={locked}
        data-testid="video-media-item"
        data-file={file}
        title={`Drag ${file} to a track, or click to append`}
        ondragstart={(event) => {
          if (event.dataTransfer) {
            event.dataTransfer.setData(VIDEO_MEDIA_TYPE, file);
            event.dataTransfer.effectAllowed = "copy";
          }
        }}
        onclick={() => onadd(file)}
      >
        <span class="film"
          >▶ <small>{file.split(".").pop()?.toUpperCase()}</small></span
        >
        <span class="filename">{file}</span><span class="append">＋</span>
      </button>
    {:else}
      <p>{query ? "No matching videos." : "Import a video to get started."}</p>
    {/each}
  </div>
  <footer>
    Drag to a track · Click to append<br />Files stay in this song’s folder.
  </footer>
</section>

<style>
  .media-pool {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    border: 1px solid var(--card-border);
    border-radius: 8px;
    padding: 12px;
    background: var(--nc-bg-2);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 13px;
  }
  header span,
  footer,
  p {
    font-size: 11px;
    color: var(--nc-fg-3);
  }
  .search {
    width: 100%;
    box-sizing: border-box;
    background: var(--nc-bg-1);
    border: 1px solid var(--card-border);
    border-radius: 5px;
    padding: 8px;
    color: var(--nc-fg-1);
  }
  .import {
    border: 1px dashed var(--nc-fg-4);
    border-radius: 6px;
    padding: 10px;
    background: transparent;
    color: var(--nc-fg-1);
    cursor: pointer;
  }
  .import small {
    display: block;
    color: var(--nc-fg-3);
    margin-top: 4px;
  }
  .import.over {
    border-color: var(--nc-cyan-400);
    background: var(--nc-bg-1);
  }
  .media-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow: auto;
    max-height: 230px;
    min-height: 90px;
  }
  .media-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: var(--nc-bg-1);
    border: 1px solid var(--card-border);
    border-radius: 5px;
    padding: 8px;
    color: var(--nc-fg-1);
    cursor: grab;
  }
  .media-item:hover {
    border-color: var(--nc-cyan-400);
  }
  .film {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    color: var(--nc-cyan-400);
  }
  .film small {
    font-size: 8px;
  }
  .filename {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 11px;
  }
  .append {
    color: var(--nc-fg-3);
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  footer {
    line-height: 1.6;
    margin-top: auto;
  }
</style>
