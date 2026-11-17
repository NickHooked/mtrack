import { describe, it, expect } from "vitest";
import { deckConfig, newOutput, reconcileDeck, vjConfigError } from "./vj";
import type { VideoConfig } from "./types";
function video(): VideoConfig {
  return {
    canvas: { width: 640, height: 360, frame_rate: 25, background: "#000000" },
    tracks: ["a", "b", "c"].map((id) => ({
      name: id,
      clips: [
        {
          id,
          start: 0,
          duration: 10,
          source_in: 0,
          file: "",
          source: { type: "test_pattern" },
        },
      ],
    })),
  };
}
describe("deck persistence across timeline edits", () => {
  it("remaps output and MIDI layer references when an earlier track is removed", () => {
    const before = video();
    before.vj = deckConfig(before);
    const output = newOutput(1);
    output.slices[0].source = { type: "layer", index: 2 };
    before.vj.outputs = [output];
    before.vj.layers[2].opacity = 0.3;
    before.vj.midi = [
      {
        id: "cc",
        device: "host",
        channel: 0,
        kind: "cc",
        number: 1,
        target: { type: "layer_opacity", layer: 2 },
        mode: "absolute",
        min: 0,
        max: 1,
      },
    ];
    const result = reconcileDeck(before, {
      ...before,
      tracks: before.tracks.slice(1),
    });
    expect(result.vj?.outputs[0].slices[0].source).toEqual({
      type: "layer",
      index: 1,
    });
    expect(result.vj?.layers[1].opacity).toBe(0.3);
    expect(result.vj?.midi[0].target).toEqual({
      type: "layer_opacity",
      layer: 1,
    });
    expect(result.vj?.grid[1][0]).toBe("c");
  });
  it("removes dangling routes without rerouting them to a different layer", () => {
    const before = video();
    before.vj = deckConfig(before);
    const output = newOutput(1);
    output.slices[0].source = { type: "layer", index: 0 };
    before.vj.outputs = [output];
    const result = reconcileDeck(before, {
      ...before,
      tracks: before.tracks.slice(1),
    });
    expect(result.vj?.outputs[0].slices).toEqual([]);
  });
  it("retains explicitly cleared cells and rejects invalid output geometry", () => {
    const before = video();
    before.vj = deckConfig(before);
    before.vj.grid[0][0] = null;
    const result = reconcileDeck(before, {
      ...before,
      tracks: before.tracks.map((t, i) =>
        i === 0 ? { ...t, name: "Renamed" } : t,
      ),
    });
    expect(result.vj?.grid[0][0]).toBeNull();
    result.vj!.outputs = [newOutput(1)];
    result.vj!.outputs[0].width = 0;
    expect(vjConfigError(result)).toContain("dimensions");
  });
});

it("normalizes a minimal YAML deck using the same defaults as the backend", async () => {
  const { parseVideoConfig } = await import("./timeline-state");
  const parsed = parseVideoConfig({
    ...video(),
    vj: {
      outputs: [
        {
          id: "out",
          name: "Screen",
          width: 640,
          height: 360,
          slices: [{ id: "slice", name: "All" }],
        },
      ],
    },
  })!;
  expect(vjConfigError(parsed)).toBeNull();
  expect(parsed.vj?.outputs[0].slices[0].corners).toHaveLength(4);
  expect(parsed.vj?.outputs[0].gamma).toBe(1);
});

it("restores the original routing snapshot on timeline undo", () => {
  const before = video();
  before.vj = deckConfig(before);
  const output = newOutput(1);
  output.slices[0].source = { type: "layer", index: 2 };
  before.vj.outputs = [output];
  const after = reconcileDeck(before, {
    ...before,
    tracks: before.tracks.slice(1),
  });
  const restored = reconcileDeck(after, structuredClone(before));
  expect(restored.vj?.outputs[0].slices[0].source).toEqual({
    type: "layer",
    index: 2,
  });
  expect(restored.vj?.grid).toEqual(before.vj.grid);
});
