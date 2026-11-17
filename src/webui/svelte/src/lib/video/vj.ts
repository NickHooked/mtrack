// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
import type { VideoConfig } from "./types";
export type BlendMode = "normal" | "add" | "screen" | "multiply";
export interface LayerSettings {
  opacity: number;
  mute: boolean;
  solo: boolean;
  blend: BlendMode;
  crossfader: "none" | "a" | "b";
  brightness: number;
  contrast: number;
  saturation: number;
  invert: number;
}
export interface ClipSettings {
  trigger: "restart" | "toggle" | "piano";
  looping: boolean;
  auto_next: boolean;
}
export interface Point {
  x: number;
  y: number;
}
export interface OutputSlice {
  id: string;
  name: string;
  source: { type: "composition" } | { type: "layer"; index: number };
  input: { x: number; y: number; width: number; height: number };
  corners: Point[];
  feather: number[];
  opacity: number;
  enabled: boolean;
}
export interface OutputCanvas {
  id: string;
  name: string;
  width: number;
  height: number;
  enabled: boolean;
  opacity: number;
  gamma: number;
  brightness: number;
  flip_x: boolean;
  flip_y: boolean;
  test_pattern: boolean;
  slices: OutputSlice[];
}
export type ControlTarget =
  | { type: "launch"; layer: number; column: number }
  | { type: "column"; column: number }
  | {
      type: "stop_layer" | "layer_opacity" | "layer_mute" | "layer_solo";
      layer: number;
    }
  | {
      type:
        | "stop_all"
        | "master_opacity"
        | "crossfader"
        | "blackout"
        | "freeze"
        | "tap_tempo";
    };
export interface MidiInput {
  kind: "note" | "cc";
  channel: number;
  number: number;
  value: number;
}
export interface MidiMapping {
  id: string;
  device: string;
  channel: number;
  kind: "note" | "cc";
  number: number;
  target: ControlTarget;
  mode: "absolute" | "toggle" | "momentary";
  min: number;
  max: number;
}
export interface VjConfig {
  bpm: number;
  quantize: number;
  columns: number;
  grid: (string | null)[][];
  layers: LayerSettings[];
  clips: Record<string, ClipSettings>;
  outputs: OutputCanvas[];
  midi: MidiMapping[];
}
export interface PerformanceState {
  clock: number;
  bpm: number;
  beat: number;
  master: number;
  crossfader: number;
  blackout: boolean;
  frozen: boolean;
  layers: {
    active: {
      id: string;
      column: number;
      started: number;
      generation: number;
    } | null;
    queued: { column: number; due: number } | null;
    settings: LayerSettings;
  }[];
}
export const defaultLayer = (): LayerSettings => ({
  opacity: 1,
  mute: false,
  solo: false,
  blend: "normal",
  crossfader: "none",
  brightness: 0,
  contrast: 1,
  saturation: 1,
  invert: 0,
});
export const defaultClip = (): ClipSettings => ({
  trigger: "restart",
  looping: true,
  auto_next: false,
});
export const fullCorners = (): Point[] => [
  { x: 0, y: 0 },
  { x: 1, y: 0 },
  { x: 1, y: 1 },
  { x: 0, y: 1 },
];
export const newSlice = (number: number): OutputSlice => ({
  id: crypto.randomUUID(),
  name: `Slice ${number}`,
  source: { type: "composition" },
  input: { x: 0, y: 0, width: 1, height: 1 },
  corners: fullCorners(),
  feather: [0, 0, 0, 0],
  opacity: 1,
  enabled: true,
});
export function newOutput(
  number: number,
  width = 1280,
  height = 720,
): OutputCanvas {
  return {
    id: crypto.randomUUID(),
    name: `Output ${number}`,
    width,
    height,
    enabled: true,
    opacity: 1,
    gamma: 1,
    brightness: 1,
    flip_x: false,
    flip_y: false,
    test_pattern: false,
    slices: [newSlice(1)],
  };
}
/** Reconcile timeline edits while keeping deck cells and settings stable by clip ID. */
export function deckConfig(video: VideoConfig): VjConfig {
  const stored = video.vj;
  const columns = Math.max(
    1,
    Math.min(
      64,
      stored?.columns ??
        Math.max(8, ...video.tracks.map((t) => t.clips.length)),
    ),
  );
  const ids = new Set(video.tracks.flatMap((t) => t.clips.map((c) => c.id)));
  return {
    bpm: stored?.bpm ?? 120,
    quantize: stored?.quantize ?? 0,
    columns,
    grid: video.tracks.map((track, i) =>
      Array.from({ length: columns }, (_, j) => {
        const id = stored?.grid?.length
          ? stored.grid[i]?.[j]
          : track.clips[j]?.id;
        return id && track.clips.some((c) => c.id === id) ? id : null;
      }),
    ),
    layers: video.tracks.map((_, i) => ({
      ...defaultLayer(),
      ...stored?.layers?.[i],
    })),
    clips: Object.fromEntries(
      Object.entries(stored?.clips ?? {})
        .filter(([id]) => ids.has(id))
        .map(([id, options]) => [id, { ...defaultClip(), ...options }]),
    ),
    outputs: (stored?.outputs ?? []).map((output) =>
      Object.assign(
        {
          enabled: true,
          opacity: 1,
          gamma: 1,
          brightness: 1,
          flip_x: false,
          flip_y: false,
          test_pattern: false,
        },
        output,
        {
          slices: (output.slices ?? []).map((slice) =>
            Object.assign(
              {
                enabled: true,
                opacity: 1,
                source: { type: "composition" as const },
                corners: fullCorners(),
                feather: [0, 0, 0, 0],
              },
              slice,
              {
                input: Object.assign(
                  { x: 0, y: 0, width: 1, height: 1 },
                  slice.input,
                ),
              },
            ),
          ),
        },
      ),
    ),
    midi: (stored?.midi ?? []).map((mapping) =>
      Object.assign(
        { device: "", mode: "absolute" as const, min: 0, max: 1 },
        mapping,
      ),
    ),
  };
}
export function targetLabel(target: ControlTarget): string {
  if (target.type === "launch")
    return `Layer ${target.layer + 1} · clip ${target.column + 1}`;
  if (target.type === "column") return `Column ${target.column + 1}`;
  return `${target.type.replaceAll("_", " ")}${"layer" in target ? ` ${target.layer + 1}` : ""}`;
}
export function validQuad(points: Point[]): boolean {
  if (
    points.length !== 4 ||
    points.some(
      (p) =>
        !Number.isFinite(p.x) ||
        !Number.isFinite(p.y) ||
        p.x < -2 ||
        p.x > 3 ||
        p.y < -2 ||
        p.y > 3,
    )
  )
    return false;
  const crosses = points.map((a, i) => {
    const b = points[(i + 1) % 4],
      c = points[(i + 2) % 4];
    return (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
  });
  return crosses.every((c) => c > 1e-6) || crosses.every((c) => c < -1e-6);
}

/** Track edits retain routing by track identity, never silently shift a projector to another layer. */
export function reconcileDeck(
  previous: VideoConfig,
  next: VideoConfig,
): VideoConfig {
  if (!previous.vj || !next.vj) return next;
  // Undo/redo carries its own already-consistent routing snapshot.
  if (JSON.stringify(previous.vj) !== JSON.stringify(next.vj)) return next;
  const saved = next.vj;
  const used = new Set<number>();
  const oldIndices = next.tracks.map((track) => {
    const candidates = previous.tracks.map((old, index) => ({
      index,
      score:
        (old.name === track.name ? 1024 : 0) +
        old.clips.filter((c) => track.clips.some((n) => n.id === c.id)).length,
    }));
    const available = candidates
      .filter((c) => !used.has(c.index) && c.score > 0)
      .sort((a, b) => b.score - a.score);
    // Ambiguous merged/renamed tracks lose their route instead of targeting an
    // arbitrary layer. The user can explicitly choose the source again.
    const best =
      available.length && available[0].score !== available[1]?.score
        ? available[0].index
        : -1;
    if (best >= 0) used.add(best);
    return best;
  });
  const grid = next.tracks.map((track, i) => {
    const oldIndex = oldIndices[i];
    const row = Array.from({ length: saved.columns }, (_, column) => {
      const id = saved.grid[oldIndex]?.[column];
      return id && track.clips.some((c) => c.id === id) ? id : null;
    });
    for (const clip of track.clips) {
      if (row.includes(clip.id)) continue;
      const oldTrack = previous.tracks.findIndex((t) =>
        t.clips.some((c) => c.id === clip.id),
      );
      if (oldTrack === oldIndex && oldTrack >= 0) continue; // Explicitly cleared cell stays clear.
      const oldColumn = saved.grid[oldTrack]?.indexOf(clip.id) ?? -1;
      const column =
        oldColumn >= 0 && !row[oldColumn] ? oldColumn : row.indexOf(null);
      if (column >= 0) row[column] = clip.id;
    }
    return row;
  });
  const ids = new Set(next.tracks.flatMap((t) => t.clips.map((c) => c.id)));
  return {
    ...next,
    vj: {
      ...saved,
      grid,
      layers: oldIndices.map((i) => saved.layers[i] ?? defaultLayer()),
      clips: Object.fromEntries(
        Object.entries(saved.clips).filter(([id]) => ids.has(id)),
      ),
      outputs: saved.outputs.map((output) => ({
        ...output,
        slices: output.slices.flatMap((slice) => {
          if (slice.source.type === "composition") return [slice];
          const index = oldIndices.indexOf(slice.source.index);
          return index < 0
            ? []
            : [{ ...slice, source: { type: "layer" as const, index } }];
        }),
      })),
      midi: saved.midi.flatMap((mapping) => {
        if (!("layer" in mapping.target)) return [mapping];
        const layer = oldIndices.indexOf(mapping.target.layer);
        return layer < 0
          ? []
          : [{ ...mapping, target: { ...mapping.target, layer } }];
      }),
    },
  };
}
export function vjConfigError(video: VideoConfig): string | null {
  const vj = video.vj;
  if (!vj) return null;
  const range = (v: number, min: number, max: number) =>
    Number.isFinite(v) && v >= min && v <= max;
  if (!range(video.canvas.frame_rate, 1, 60))
    return "VJ compositions support 1–60 fps.";
  if (!range(vj.bpm, 20, 300))
    return "VJ tempo must be between 20 and 300 BPM.";
  if (!Number.isInteger(vj.columns) || !range(vj.columns, 1, 64))
    return "VJ decks support 1–64 columns.";
  if (vj.outputs.length > 8) return "At most 8 output canvases are supported.";
  let total = 0;
  const ids = new Set<string>();
  for (const output of vj.outputs) {
    if (
      !output.id ||
      output.id === "composition" ||
      ids.has(output.id) ||
      !output.name.trim()
    )
      return "Output names and unique IDs are required.";
    ids.add(output.id);
    if (
      !Number.isInteger(output.width) ||
      !Number.isInteger(output.height) ||
      !range(output.width, 1, 4096) ||
      !range(output.height, 1, 4096)
    )
      return "Output dimensions must be 1–4096 pixels.";
    total += output.width * output.height;
    if (
      !range(output.opacity, 0, 1) ||
      !range(output.brightness, 0, 2) ||
      !range(output.gamma, 0.1, 4)
    )
      return "Output calibration is out of range.";
    if (output.slices.length > 16)
      return "At most 16 slices per output are supported.";
    for (const slice of output.slices) {
      if (!validQuad(slice.corners) || !slice.name.trim())
        return "A slice needs a name and four non-crossing corners.";
      if (slice.source.type === "layer" && !video.tracks[slice.source.index])
        return "A slice references a removed layer.";
      if (
        !range(slice.opacity, 0, 1) ||
        slice.feather.some((v) => !range(v, 0, 1))
      )
        return "Slice opacity and feather must be between 0 and 100%.";
      const r = slice.input;
      if (
        !range(r.x, 0, 1) ||
        !range(r.y, 0, 1) ||
        !range(r.width, 0.0001, 1) ||
        !range(r.height, 0.0001, 1) ||
        r.x + r.width > 1.000001 ||
        r.y + r.height > 1.000001
      )
        return "Slice input crops must fit within the composition.";
    }
  }
  if (total > 16777216)
    return "Output canvases exceed the combined 16 megapixel limit.";
  return null;
}
