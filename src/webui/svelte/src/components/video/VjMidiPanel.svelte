<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    targetLabel,
    type ControlTarget,
    type MidiInput,
    type MidiMapping,
  } from "../../lib/video/vj";
  let {
    mappings,
    learning = null,
    lastInput = null,
    locked = false,
    onchange,
    oninput,
    oncancel,
  }: {
    mappings: MidiMapping[];
    learning?: ControlTarget | null;
    lastInput?: { device: string; input: MidiInput } | null;
    locked?: boolean;
    onchange: (m: MidiMapping[]) => void;
    oninput: (device: string, data: number[]) => void;
    oncancel: () => void;
  } = $props();
  let access: MIDIAccess | null = null;
  let devices = $state<{ id: string; name: string }[]>([]);
  let selected = $state("all");
  let error = $state("");
  let enabled = $state(false);
  async function connect() {
    error = "";
    try {
      if (!navigator.requestMIDIAccess)
        throw new Error(
          "Web MIDI is unavailable in this browser. Host MIDI still works through Mtrack.",
        );
      access = await navigator.requestMIDIAccess({ sysex: false });
      access.onstatechange = refresh;
      enabled = true;
      refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : "MIDI access failed";
    }
  }
  function refresh() {
    if (!access) return;
    devices = Array.from(access.inputs.values()).map((input) => ({
      id: input.id,
      name: input.name ?? input.id,
    }));
    for (const input of access.inputs.values())
      input.onmidimessage = (e) => {
        if (e.data && (selected === "all" || selected === input.id))
          oninput(`browser:${input.id}`, Array.from(e.data));
      };
  }
  onDestroy(() => {
    if (access) {
      access.onstatechange = null;
      for (const input of access.inputs.values()) input.onmidimessage = null;
    }
  });
  function patch(id: string, patch: Partial<MidiMapping>) {
    onchange(mappings.map((m) => (m.id === id ? { ...m, ...patch } : m)));
  }
</script>

<section class="midi-panel" aria-label="MIDI controls">
  <header>
    <strong>MIDI controls</strong><button class="btn btn-sm" onclick={connect}
      >{enabled ? "Refresh inputs" : "Connect browser MIDI"}</button
    >
  </header>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if enabled}<label
      >Browser input<select bind:value={selected}
        ><option value="all">All browser inputs</option
        >{#each devices as d (d.id)}<option value={d.id}>{d.name}</option
          >{/each}</select
      ></label
    >{/if}
  <div
    class="monitor"
    title="Host MIDI uses the input configured in Mtrack. Browser MIDI uses controllers connected to this computer."
  >
    <span class:lit={!!lastInput}>●</span>
    {lastInput
      ? `${lastInput.device === "host" ? "Host" : "Browser"} · Ch ${lastInput.input.channel + 1} · ${lastInput.input.kind.toUpperCase()} ${lastInput.input.number} · ${lastInput.input.value}`
      : "Host / browser input monitor"}
  </div>
  {#if learning}<div class="learning">
      <strong>{targetLabel(learning)}</strong><span
        >Move a fader or press a MIDI key</span
      ><button class="btn btn-sm" onclick={oncancel}>Cancel</button>
    </div>{/if}
  {#each mappings as mapping (mapping.id)}
    <div class="binding">
      <header>
        <strong>{targetLabel(mapping.target)}</strong><button
          class="btn btn-sm"
          disabled={locked}
          title="Remove MIDI binding"
          onclick={() => onchange(mappings.filter((m) => m.id !== mapping.id))}
          >×</button
        >
      </header>
      <div class="row">
        <label
          >Channel<input
            type="number"
            min="1"
            max="16"
            value={mapping.channel + 1}
            disabled={locked}
            onchange={(e) =>
              patch(mapping.id, {
                channel: Math.max(0, Math.min(15, +e.currentTarget.value - 1)),
              })}
          /></label
        ><label
          >Type<select
            value={mapping.kind}
            disabled={locked}
            onchange={(e) =>
              patch(mapping.id, {
                kind: e.currentTarget.value as "note" | "cc",
              })}
            ><option value="note">Note</option><option value="cc">CC</option
            ></select
          ></label
        ><label
          >Number<input
            type="number"
            min="0"
            max="127"
            value={mapping.number}
            disabled={locked}
            onchange={(e) =>
              patch(mapping.id, {
                number: Math.max(0, Math.min(127, +e.currentTarget.value)),
              })}
          /></label
        >
      </div>
      <div class="row">
        <label
          >Mode<select
            value={mapping.mode}
            disabled={locked}
            onchange={(e) =>
              patch(mapping.id, {
                mode: e.currentTarget.value as MidiMapping["mode"],
              })}
            ><option value="absolute">Absolute</option><option value="toggle"
              >Toggle</option
            ><option value="momentary">Momentary</option></select
          ></label
        ><label
          >Min<input
            type="number"
            min="0"
            max="1"
            step=".01"
            disabled={locked}
            value={mapping.min}
            onchange={(e) =>
              patch(mapping.id, {
                min: Math.max(0, Math.min(1, +e.currentTarget.value)),
              })}
          /></label
        ><label
          >Max<input
            type="number"
            min="0"
            max="1"
            step=".01"
            disabled={locked}
            value={mapping.max}
            onchange={(e) =>
              patch(mapping.id, {
                max: Math.max(0, Math.min(1, +e.currentTarget.value)),
              })}
          /></label
        >
      </div>
      <label
        >Input<select
          disabled={locked}
          value={mapping.device}
          onchange={(e) => patch(mapping.id, { device: e.currentTarget.value })}
          ><option value="">Any input</option><option value="host"
            >Host input</option
          >{#if mapping.device.startsWith("browser:") && !devices.some((d) => `browser:${d.id}` === mapping.device)}<option
              value={mapping.device}>Learned browser input</option
            >{/if}{#each devices as d (d.id)}<option value={`browser:${d.id}`}
              >{d.name}</option
            >{/each}</select
        ></label
      >
    </div>
  {:else}<div
      class="empty"
      title="Enable MIDI learn in the performance toolbar, select a clip or control, then move your controller."
    >
      No MIDI bindings
    </div>{/each}
</section>

<style>
  .midi-panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
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
  .binding {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid #354052;
    border-radius: 6px;
    padding: 10px;
    font-size: 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: #9dacc2;
    font-size: 11px;
    flex: 1;
  }
  input,
  select {
    width: 100%;
    min-width: 0;
    background: #111822;
    color: #e3eaf4;
    border: 1px solid #354052;
    padding: 6px;
    border-radius: 5px;
  }
  .monitor,
  .empty {
    padding: 10px;
    background: #101722;
    font-size: 12px;
    color: #9dacc2;
  }
  .lit {
    color: #6ee7cc;
  }
  .learning {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border: 1px solid #d99cff;
    color: #e7c0ff;
    font-size: 12px;
  }
  p {
    font-size: 12px;
    color: #ffb0b0;
  }
  .btn {
    font-size: 11px;
  }
</style>
