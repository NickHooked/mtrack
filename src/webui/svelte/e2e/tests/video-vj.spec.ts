import { expect, test, type Page, type WebSocketRoute } from "@playwright/test";
import { parse, stringify } from "yaml";
import { ConfigService } from "../../src/gen/player/v1/player_pb";
import { getSongConfigReply, listSongFilesReply } from "../support/config-rpc";
import { mockRpc } from "../support/rpc";
const SONG = "Test Song Alpha";
const config = {
  name: SONG,
  kind: "song",
  tracks: [{ name: "kick", file: "kick.wav" }],
  video: {
    canvas: { width: 640, height: 360, frame_rate: 25, background: "#000000" },
    tracks: [
      {
        name: "Cameras",
        clips: [
          {
            id: "live",
            start: 0,
            duration: 10,
            source_in: 0,
            source: { type: "test_pattern" },
            file: "",
          },
          {
            id: "file",
            start: 10,
            duration: 10,
            source_in: 0,
            file: "opening.mp4",
          },
        ],
      },
      {
        name: "Overlay",
        clips: [
          {
            id: "other",
            start: 0,
            duration: 10,
            source_in: 0,
            source: { type: "test_pattern" },
            file: "",
          },
        ],
      },
    ],
  },
};
async function open(page: Page) {
  let yaml = stringify(config);
  const writes: string[] = [];
  const commands: Record<string, unknown>[] = [];
  let socket: WebSocketRoute;
  const connections: string[] = [];
  const rpc = await mockRpc(page);
  rpc.unary(ConfigService.method.getSongConfig, () => getSongConfigReply(yaml));
  rpc.unary(ConfigService.method.saveSongConfig, (req) => {
    yaml = req.yaml;
    writes.push(yaml);
    return {};
  });
  rpc.unary(
    ConfigService.method.listSongFiles,
    listSongFilesReply([{ name: "opening.mp4", type: "other" }]),
  );
  await page.route("**/api/video/sources", (route) =>
    route.fulfill({ json: { live_capture: true, devices: [] } }),
  );
  await page.routeWebSocket("**/vj/ws", (ws) => {
    socket = ws;
    connections.push(ws.url());
    ws.onMessage((data) => {
      const message = JSON.parse(String(data));
      commands.push(message);
      if (message.type === "start")
        ws.send(
          JSON.stringify({ type: "state", running: true, starting: false }),
        );
      if (message.type === "stop")
        ws.send(
          JSON.stringify({ type: "state", running: false, starting: false }),
        );
    });
  });
  await page.goto(`/#/songs/${encodeURIComponent(SONG)}`);
  await page.getByRole("tab", { name: "Video", exact: true }).click();
  await page.getByRole("button", { name: "Live / VJ", exact: true }).click();
  await expect(page.getByTestId("vj-workspace")).toBeVisible();
  return {
    commands,
    connections,
    disconnect: () => socket.close(),
    writes,
    read: () => parse(yaml),
    send: (message: unknown) => socket.send(JSON.stringify(message)),
  };
}
test.use({ viewport: { width: 1560, height: 1100 } });
test("maps independent outputs, corner drag and crops survive save and reload", async ({
  page,
}) => {
  const state = await open(page);
  await page.getByRole("button", { name: "Create an output canvas" }).click();
  await page.getByLabel("Output name", { exact: true }).fill("Left projector");
  await page.getByLabel("Output name", { exact: true }).dispatchEvent("change");
  await page.getByLabel("Output width", { exact: true }).fill("640");
  await page
    .getByLabel("Output width", { exact: true })
    .dispatchEvent("change");
  const corner = page.getByRole("button", {
    name: "Mapping corner 1",
    exact: true,
  });
  await corner.focus();
  await page.keyboard.press("ArrowRight");
  await page.getByLabel("Slice input width", { exact: true }).fill("50");
  await page
    .getByLabel("Slice input width", { exact: true })
    .dispatchEvent("change");
  await page.getByText("Edge blending", { exact: true }).click();
  await page.getByLabel("Feather right", { exact: true }).fill("20");
  await page
    .getByLabel("Feather right", { exact: true })
    .dispatchEvent("change");
  await page.getByRole("button", { name: "Duplicate", exact: true }).click();
  await page.getByLabel("Output name", { exact: true }).fill("Right projector");
  await page.getByLabel("Output name", { exact: true }).dispatchEvent("change");
  await page
    .getByLabel("Slice input source", { exact: true })
    .selectOption("layer:1");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(1);
  const saved = state.read().video.vj;
  expect(saved.outputs).toHaveLength(2);
  expect(saved.outputs[0].slices[0].corners[0].x).toBeCloseTo(1 / 640);
  expect(saved.outputs[0].slices[0].input.width).toBe(0.5);
  expect(saved.outputs[0].slices[0].feather[2]).toBe(0.2);
  expect(saved.outputs[1].slices[0].source).toEqual({
    type: "layer",
    index: 1,
  });
  await page.reload();
  await page.getByRole("button", { name: "Live / VJ", exact: true }).click();
  await expect(page.getByLabel("Select output")).toHaveCount(1);
  await expect(page.getByLabel("Select output").locator("option")).toHaveCount(
    2,
  );
});
test("launches columns, learns host MIDI and sends a layer fader", async ({
  page,
}) => {
  const state = await open(page);
  const quantize = page.getByLabel("Launch quantization");
  await expect(quantize).toHaveValue("0");
  await quantize.selectOption("1");
  await expect(quantize).toHaveValue("1");
  await page
    .getByRole("button", { name: "Start VJ output", exact: false })
    .click();
  await expect(
    page.getByRole("button", { name: "■ Stop output", exact: true }),
  ).toBeVisible();
  await page.getByTestId("vj-column-0").click();
  await expect
    .poll(() =>
      state.commands.some(
        (c) =>
          c.type === "control" && JSON.stringify(c.event).includes("column"),
      ),
    )
    .toBe(true);
  await page.getByRole("button", { name: "MIDI learn", exact: true }).click();
  await page.getByLabel("Layer 1 opacity", { exact: true }).click();
  await expect(
    page.getByText("Move a fader or press a MIDI key"),
  ).toBeVisible();
  state.send({
    type: "midi",
    device: "host",
    input: { kind: "cc", channel: 2, number: 7, value: 64 },
  });
  await expect(
    page.getByText("layer opacity 1", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(1);
  expect(state.read().video.vj.midi[0]).toMatchObject({
    device: "host",
    channel: 2,
    kind: "cc",
    number: 7,
    target: { type: "layer_opacity", layer: 0 },
    mode: "absolute",
  });
  expect(state.read().video.vj.quantize).toBe(1);
  await page.getByLabel("Layer 1 opacity", { exact: true }).fill("0.25");
  await page
    .getByLabel("Layer 1 opacity", { exact: true })
    .dispatchEvent("input");
  await expect
    .poll(() =>
      state.commands.some(
        (c) =>
          JSON.stringify(c.event) ===
          '{"target":{"type":"layer_opacity","layer":0},"value":0.25}',
      ),
    )
    .toBe(true);
  await page
    .getByRole("button", { name: "■ Stop output", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Start VJ output" }),
  ).toBeVisible();
});
test("drops media into cells and reorders within a layer", async ({ page }) => {
  const state = await open(page);
  await page
    .getByTestId("video-media-item")
    .filter({ hasText: "opening.mp4" })
    .dragTo(page.getByTestId("vj-cell-0-2"));
  await expect(page.getByTestId("vj-cell-0-2")).toContainText("opening.mp4");
  await page.getByTestId("vj-cell-0-2").dragTo(page.getByTestId("vj-cell-0-3"));
  await expect(page.getByTestId("vj-cell-0-2")).not.toContainText(
    "opening.mp4",
  );
  await expect(page.getByTestId("vj-cell-0-3")).toContainText("opening.mp4");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(1);
  expect(state.read().video.vj.grid[0][2]).toBeNull();
  expect(state.read().video.vj.grid[0][3]).toBeTruthy();
});

test("piano mode releases on pointer and keyboard without leaving a clip held", async ({
  page,
}) => {
  const state = await open(page);
  await page.getByTestId("vj-cell-0-0").click({ button: "right" });
  await page
    .getByLabel("Clip trigger mode", { exact: true })
    .selectOption("piano");
  await page.getByRole("button", { name: "Start VJ output" }).click();
  await expect(
    page.getByRole("button", { name: "■ Stop output", exact: true }),
  ).toBeVisible();
  await page.getByTestId("vj-cell-0-0").click();
  const launches = () =>
    state.commands
      .filter((c) => c.type === "control")
      .map((c) => c.event as { target: { type: string }; value: number })
      .filter((e) => e.target.type === "launch");
  await expect
    .poll(() =>
      launches()
        .slice(-2)
        .map((e) => e.value),
    )
    .toEqual([1, 0]);
  await page.getByTestId("vj-cell-0-0").focus();
  await page.keyboard.press("Space");
  await expect
    .poll(() =>
      launches()
        .slice(-2)
        .map((e) => e.value),
    )
    .toEqual([1, 0]);
  await page.getByTestId("vj-column-0").click();
  const columns = () =>
    state.commands
      .filter((c) => c.type === "control")
      .map((c) => c.event as { target: { type: string }; value: number })
      .filter((e) => e.target.type === "column");
  await expect
    .poll(() =>
      columns()
        .slice(-2)
        .map((e) => e.value),
    )
    .toEqual([1, 0]);
});

test("reconnect keeps the draft and song navigation opens a fresh session", async ({
  page,
}) => {
  const state = await open(page);
  await page.getByRole("button", { name: "Create an output canvas" }).click();
  await page.getByLabel("Output name", { exact: true }).fill("Keep my mapping");
  await page.getByLabel("Output name", { exact: true }).dispatchEvent("change");
  state.disconnect();
  await expect(
    page.getByRole("button", { name: "Reconnect", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Reconnect", exact: true }).click();
  await expect.poll(() => state.connections.length).toBe(2);
  await expect(page.getByLabel("Output name", { exact: true })).toHaveValue(
    "Keep my mapping",
  );
  expect(state.commands.some((command) => command.type === "start")).toBe(
    false,
  );
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(1);
  await expect(page.locator(".save-msg")).toContainText("Saved");
  await page.goto("/#/songs/Test%20Song%20Beta/video");
  await expect(
    page.getByRole("heading", { name: "Test Song Beta", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Live / VJ", exact: true }).click();
  await expect
    .poll(() => state.connections.at(-1))
    .toContain("Test%20Song%20Beta/vj/ws");
});
