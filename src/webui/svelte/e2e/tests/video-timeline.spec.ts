// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

import { expect, test, type Locator, type Page } from "@playwright/test";
import { parse } from "yaml";
import { SONGS } from "../mock-server/test-data";

import {
  ConfigService,
  PlayerService,
} from "../../src/gen/player/v1/player_pb";
import { getSongConfigReply, listSongFilesReply } from "../support/config-rpc";
import { listSongsReply } from "../support/library-rpc";
import { mockRpc } from "../support/rpc";

const SONG = "Test Song Alpha";
const ENC = encodeURIComponent(SONG);
const INITIAL_YAML = `name: ${SONG}
kind: song
custom_field:
  keep_me: true
tracks:
  - name: kick
    file: kick.wav
video:
  canvas:
    width: 1920
    height: 1080
    frame_rate: 50
    background: "#000000"
  tracks:
    - name: Main
      clips:
        - id: opening
          start: 0
          duration: 10
          file: opening.mp4
          source_in: 0
`;

async function installStatefulSongRoutes(
  page: Page,
  initialYaml = INITIAL_YAML,
  songName = SONG,
) {
  let yaml = initialYaml;
  const writes: string[] = [];
  const rpc = await mockRpc(page);
  rpc.unary(ConfigService.method.getSongConfig, () => getSongConfigReply(yaml));
  rpc.unary(ConfigService.method.saveSongConfig, (req) => {
    yaml = req.yaml;
    writes.push(yaml);
    return {};
  });
  const songs = structuredClone(SONGS);
  if (!songs.songs.some((song) => song.name === songName)) {
    songs.songs.push({ ...songs.songs[0], name: songName });
  }
  rpc.unary(PlayerService.method.listSongs, listSongsReply(songs));
  rpc.unary(
    ConfigService.method.listSongFiles,
    listSongFilesReply([
      { name: "opening.mp4", type: "other" },
      { name: "camera-b.mp4", type: "other" },
    ]),
  );

  return { rpc, writes, currentYaml: () => yaml };
}

async function openVideo(page: Page, yaml = INITIAL_YAML, songName = SONG) {
  const state = await installStatefulSongRoutes(page, yaml, songName);
  await page.goto(`/#/songs/${encodeURIComponent(songName)}`);
  await page.getByRole("tab", { name: "Video" }).click();
  await expect(page.getByTestId("video-timeline-editor")).toBeVisible();
  await page.getByText("Manual placement", { exact: true }).click();
  return state;
}

async function dragBy(page: Page, locator: Locator, deltaX: number) {
  await locator.scrollIntoViewIfNeeded();
  const box = await locator.boundingBox();
  if (!box) throw new Error("timeline control has no bounding box");
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + deltaX, y, { steps: 5 });
  await page.mouse.up();
}

async function selectedValues(page: Page) {
  return {
    start: Number(await page.getByTestId("video-clip-start").inputValue()),
    duration: Number(
      await page.getByTestId("video-clip-duration").inputValue(),
    ),
    sourceIn: Number(
      await page.getByTestId("video-clip-source-in").inputValue(),
    ),
  };
}

test.describe("Song video timeline", () => {
  test("blocks unified save while a draft track is empty", async ({ page }) => {
    const state = await openVideo(page);

    await page.getByTestId("video-add-track").click();
    await expect(page.getByTestId("video-track")).toHaveCount(2);
    await expect(page.getByTestId("video-error")).toContainText(
      /add a clip.*before saving/i,
    );

    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.locator(".save-msg")).toContainText(
      /add a clip.*before saving/i,
    );
    expect(state.writes).toHaveLength(0);
  });

  test("moves clips at the default 10 px/s pointer scale", async ({ page }) => {
    await openVideo(page);
    const clip = page.getByTestId("video-clip").filter({ hasText: "opening" });
    await clip.click();

    await dragBy(page, clip, 40);

    const start = Number(
      await page.getByTestId("video-clip-start").inputValue(),
    );
    expect(start).toBeGreaterThan(0);
    expect(start).toBeCloseTo(4, 1);
  });

  test("supports keyboard move, trim, and delete without bubbling trim keys", async ({
    page,
  }) => {
    const yaml = INITIAL_YAML.replace("start: 0", "start: 5").replace(
      "source_in: 0",
      "source_in: 2",
    );
    await openVideo(page, yaml);
    const clip = page.getByTestId("video-clip").filter({ hasText: "opening" });

    await clip.focus();
    await clip.press("ArrowRight");
    await expect(page.getByTestId("video-clip-start")).toHaveValue("5.1");

    const left = page.getByTestId("video-trim-left");
    await expect(left).toHaveAttribute("tabindex", "0");
    await expect(left).toHaveAttribute("aria-valuemin", "3.1");
    await expect(left).toHaveAttribute("aria-valuemax", "15.09");
    await left.focus();
    await left.press("ArrowRight");
    expect(await selectedValues(page)).toEqual({
      start: 5.2,
      duration: 9.9,
      sourceIn: 2.1,
    });

    const right = page.getByTestId("video-trim-right");
    await expect(right).toHaveAttribute("tabindex", "0");
    await expect(right).toHaveAttribute("aria-valuemin", "5.21");
    await expect(right).toHaveAttribute("aria-valuemax", "60");
    await right.focus();
    await right.press("ArrowLeft");
    expect(await selectedValues(page)).toEqual({
      start: 5.2,
      duration: 9.8,
      sourceIn: 2.1,
    });

    await clip.focus();
    await clip.press("Backspace");
    await expect(page.getByTestId("video-clip")).toHaveCount(0);
    await expect(page.getByTestId("video-track")).toHaveCount(0);
  });

  test("applies trim math, accepts touching boundaries, rejects overlaps, and clamps duration", async ({
    page,
  }) => {
    const yaml = `name: ${SONG}
kind: song
tracks:
  - name: kick
    file: kick.wav
video:
  tracks:
    - name: Main
      clips:
        - id: before
          start: 0
          duration: 8
          file: before.mp4
          source_in: 0
        - id: target
          start: 10
          duration: 10
          file: target.mp4
          source_in: 5
        - id: after
          start: 25
          duration: 5
          file: after.mp4
          source_in: 0
`;
    await openVideo(page, yaml);
    const target = page.getByTestId("video-clip").filter({ hasText: "target" });
    await target.click();

    await dragBy(page, target.getByTestId("video-trim-left"), -20);
    expect(await selectedValues(page)).toEqual({
      start: 8,
      duration: 12,
      sourceIn: 3,
    });

    await dragBy(page, target.getByTestId("video-trim-left"), -10);
    await expect(page.getByTestId("video-error")).toContainText(/overlap/i);
    expect(await selectedValues(page)).toEqual({
      start: 8,
      duration: 12,
      sourceIn: 3,
    });

    await dragBy(page, target.getByTestId("video-trim-left"), 20);
    expect(await selectedValues(page)).toEqual({
      start: 10,
      duration: 10,
      sourceIn: 5,
    });

    await dragBy(page, target.getByTestId("video-trim-right"), 50);
    expect(await selectedValues(page)).toEqual({
      start: 10,
      duration: 15,
      sourceIn: 5,
    });

    await dragBy(page, target.getByTestId("video-trim-right"), 10);
    await expect(page.getByTestId("video-error")).toContainText(/overlap/i);
    expect(await selectedValues(page)).toEqual({
      start: 10,
      duration: 15,
      sourceIn: 5,
    });

    await dragBy(page, target.getByTestId("video-trim-right"), -300);
    expect(await selectedValues(page)).toEqual({
      start: 10,
      duration: 0.01,
      sourceIn: 5,
    });
  });

  test("saves deletion to YAML and reloads without an empty track", async ({
    page,
  }) => {
    const state = await openVideo(page);
    await page.getByTestId("video-clip").click();
    await page.getByTestId("video-delete-clip").click();
    await expect(page.getByTestId("video-track")).toHaveCount(0);

    await page.getByRole("button", { name: "Save" }).click();
    await expect.poll(() => state.writes.length).toBe(1);
    const saved = parse(state.currentYaml());
    expect(saved.custom_field).toEqual({ keep_me: true });
    expect(saved.video.tracks).toEqual([]);

    await page.reload();
    await page.getByRole("tab", { name: "Video" }).click();
    await expect(page.getByTestId("video-track")).toHaveCount(0);
    await expect(page.getByTestId("video-clip")).toHaveCount(0);
  });

  test("normalizes missing, blank, and duplicate clip IDs stably and globally", async ({
    page,
  }) => {
    const yaml = `name: ${SONG}
kind: song
tracks:
  - name: kick
    file: kick.wav
video:
  tracks:
    - name: Main
      clips:
        - id: keeper
          start: 0
          duration: 1
          file: one.mp4
        - id: duplicate
          start: 2
          duration: 1
          file: two.mp4
        - id: "   "
          start: 4
          duration: 1
          file: three.mp4
    - name: Overlay
      clips:
        - id: duplicate
          start: 0
          duration: 1
          file: four.mp4
        - start: 2
          duration: 1
          file: five.mp4
`;
    await openVideo(page, yaml);

    const ids = await page
      .getByTestId("video-clip")
      .evaluateAll((clips) =>
        clips.map((clip) => clip.getAttribute("data-clip-id")),
      );
    expect(ids).toHaveLength(5);
    expect(ids).toContain("keeper");
    expect(ids.every((id) => id !== null && id.trim().length > 0)).toBe(true);
    expect(new Set(ids).size).toBe(ids.length);

    await page.reload();
    await page.getByRole("tab", { name: "Video" }).click();
    const reloadedIds = await page
      .getByTestId("video-clip")
      .evaluateAll((clips) =>
        clips.map((clip) => clip.getAttribute("data-clip-id")),
      );
    expect(reloadedIds).toEqual(ids);
  });

  test("preserves the selected clip and dirty draft across tab switches", async ({
    page,
  }) => {
    await openVideo(page);
    await page.getByTestId("video-clip").click();
    await page.getByTestId("video-clip-start").fill("3");
    await page.getByTestId("video-clip-start").dispatchEvent("change");
    await page.getByRole("tab", { name: "Tracks" }).click();
    await page.getByRole("tab", { name: "Video" }).click();
    await expect(page.getByTestId("video-clip-start")).toHaveValue("3");
    await expect(page.locator(".unsaved")).toBeVisible();
  });

  test("starts a dedicated paused preview and supports play, pause, seek, and stop", async ({
    page,
  }) => {
    await openVideo(page);
    const preview = page.getByTestId("video-preview");
    await expect(preview).toBeVisible();
    await expect(page.getByTestId("video-preview-start")).toBeEnabled();

    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-image")).toBeVisible();
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /paused/i,
    );
    await expect(page.getByTestId("video-preview-time")).toContainText(
      "00:00.000",
    );

    await page.getByTestId("video-preview-play").click();
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /playing/i,
    );
    await page.getByTestId("video-preview-pause").click();
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /paused/i,
    );

    await page.getByTestId("video-preview-seek").fill("5000");
    await page.getByTestId("video-preview-seek").dispatchEvent("change");
    await expect(page.getByTestId("video-preview-time")).toContainText(
      "00:05.000",
    );
    await page.getByTestId("video-preview-stop").click();
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /stopped/i,
    );
  });

  test("marks an edited draft stale, retains its frame, and reloads explicitly", async ({
    page,
  }) => {
    await openVideo(page);
    await page.getByTestId("video-preview-start").click();
    const image = page.getByTestId("video-preview-image");
    await expect(image).toBeVisible();
    const first = await image.getAttribute("src");

    await page.getByTestId("video-clip").click();
    await page.getByTestId("video-clip-start").fill("2");
    await page.getByTestId("video-clip-start").dispatchEvent("change");
    await expect(page.getByTestId("video-preview-stale")).toBeVisible();
    await expect(image).toHaveAttribute("src", first ?? "");

    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-stale")).toHaveCount(0);
    await expect(page.getByTestId("video-preview-time")).toContainText(
      "00:00.000",
    );
  });

  test("clearly disables preview when the server capability is unavailable", async ({
    page,
  }) => {
    await page.route("**/api/video/capabilities", async (route) => {
      await route.fulfill({ json: { video_preview: false } });
    });
    await openVideo(page);
    await expect(page.getByTestId("video-preview-unavailable")).toContainText(
      /unavailable/i,
    );
    await expect(page.getByTestId("video-preview-start")).toBeDisabled();
  });

  test("preview controls remain usable at a mobile viewport", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await openVideo(page);
    await expect(page.getByTestId("video-preview")).toBeVisible();
    await expect(
      page.getByRole("button", { name: /fullscreen/i }),
    ).toBeVisible();
    await expect(page.getByLabel("Video preview position")).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    for (const control of [
      page.getByTestId("video-preview-start"),
      page.getByTestId("video-preview-play"),
      page.getByTestId("video-preview-pause"),
      page.getByTestId("video-preview-stop"),
      page.getByRole("button", { name: /fullscreen video preview/i }),
    ]) {
      expect((await control.boundingBox())?.height).toBeGreaterThanOrEqual(44);
    }
  });

  test("recovers from a busy close with an explicit fresh retry", async ({
    page,
  }) => {
    const busySong = "Busy Preview Test";
    const busyEnc = encodeURIComponent(busySong);
    const controlUrl = `http://127.0.0.1:3111/test/video-preview/${busyEnc}`;
    await page.request.post(`${controlUrl}/busy`, { data: { busy: true } });
    await openVideo(
      page,
      INITIAL_YAML.replace(`name: ${SONG}`, `name: ${busySong}`),
      busySong,
    );
    await expect(page.getByTestId("video-preview-error")).toContainText(
      /another video preview session is already active/i,
    );
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /^busy$/i,
    );
    await expect(page.getByTestId("video-preview-retry")).toBeEnabled();
    await expect
      .poll(async () => (await page.request.get(`${controlUrl}/stats`)).json())
      .toMatchObject({ connections: 1 });

    await page.request.post(`${controlUrl}/busy`, { data: { busy: false } });
    await page.getByTestId("video-preview-retry").click();
    await expect(page.getByTestId("video-preview-start")).toBeEnabled();
    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-image")).toBeVisible();
    await expect(page.getByTestId("video-preview-status")).toContainText(
      /paused/i,
    );
    await expect
      .poll(async () => (await page.request.get(`${controlUrl}/stats`)).json())
      .toMatchObject({ connections: 2 });
  });

  test("does not complete reload from an old same-epoch buffered frame", async ({
    page,
  }) => {
    await openVideo(
      page,
      INITIAL_YAML.replace('background: "#000000"', 'background: "#00ff00"'),
    );
    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-image")).toBeVisible();
    await page.getByTestId("video-clip").click();
    await page.getByTestId("video-clip-start").fill("2");
    await page.getByTestId("video-clip-start").dispatchEvent("change");
    await expect(page.getByTestId("video-preview-stale")).toBeVisible();

    await page.getByTestId("video-preview-start").click();
    await page.waitForTimeout(100);
    await expect(page.getByTestId("video-preview-stale")).toBeVisible();
    await expect(page.getByTestId("video-preview-stale")).toHaveCount(0, {
      timeout: 2_000,
    });
  });

  test("resets preview state and socket when switching songs on the video tab", async ({
    page,
  }) => {
    const nextSong = "Preview Switch Target";
    const nextEnc = encodeURIComponent(nextSong);
    const { rpc } = await installStatefulSongRoutes(page);
    rpc.unary(ConfigService.method.getSongConfig, (req) =>
      getSongConfigReply(
        INITIAL_YAML.replace(`name: ${SONG}`, `name: ${req.name}`),
      ),
    );
    const songs = structuredClone(SONGS);
    songs.songs.push({ ...songs.songs[0], name: nextSong });
    rpc.unary(PlayerService.method.listSongs, listSongsReply(songs));

    const sockets: string[] = [];
    let firstSocketClosed = false;
    page.on("websocket", (socket) => {
      if (socket.url().includes("/video-preview/ws")) {
        sockets.push(socket.url());
        if (sockets.length === 1) {
          socket.on("close", () => (firstSocketClosed = true));
        }
      }
    });
    await page.goto(`/#/songs/${ENC}/video`);
    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-image")).toBeVisible();
    await page.getByTestId("video-clip").click();
    await page.getByTestId("video-clip-start").fill("2");
    await page.getByTestId("video-clip-start").dispatchEvent("change");
    await expect(page.getByTestId("video-preview-stale")).toBeVisible();

    await page.evaluate(
      (hash) => (window.location.hash = hash),
      `#/songs/${nextEnc}/video`,
    );
    await page.getByRole("button", { name: "Confirm" }).click();

    await expect(page.getByRole("heading", { name: nextSong })).toBeVisible();
    await expect(page.getByTestId("video-preview-image")).toHaveCount(0);
    await expect(page.getByTestId("video-preview-stale")).toHaveCount(0);
    await expect(page.getByTestId("video-preview-start")).toBeEnabled();
    await expect(page.getByTestId("video-preview-play")).toBeDisabled();
    await expect.poll(() => sockets.length).toBe(2);
    await expect.poll(() => firstSocketClosed).toBe(true);
    expect(sockets[1]).toContain(`/api/songs/${nextEnc}/video-preview/ws`);
  });

  test("shows a protocol-disabled preview as unavailable", async ({ page }) => {
    await openVideo(
      page,
      INITIAL_YAML.replace('background: "#000000"', 'background: "#0000ff"'),
    );
    await page.getByTestId("video-preview-start").click();
    await expect(page.getByTestId("video-preview-error")).toContainText(
      /video preview is disabled in this build/i,
    );
    await expect(page.getByTestId("video-preview-unavailable")).toBeVisible();
    await expect(page.getByTestId("video-preview-start")).toBeDisabled();
  });

  test("fullscreen targets the preview viewport and exits cleanly", async ({
    page,
  }) => {
    await openVideo(page);
    const button = page.getByRole("button", {
      name: "Fullscreen video preview",
    });
    if (await button.isEnabled()) {
      await button.click();
      await expect
        .poll(() =>
          page.evaluate(() =>
            document.fullscreenElement?.getAttribute("data-testid"),
          ),
        )
        .toBe("video-preview-viewport");
      await page
        .getByRole("button", { name: "Exit fullscreen video preview" })
        .click();
      await expect
        .poll(() => page.evaluate(() => document.fullscreenElement))
        .toBeNull();
    }
  });

  test("closes the dedicated preview websocket when leaving the video tab", async ({
    page,
  }) => {
    let resolveClosed!: () => void;
    const closed = new Promise<boolean>((resolve) => {
      resolveClosed = () => resolve(true);
    });
    page.on("websocket", (socket) => {
      if (socket.url().includes("/video-preview/ws")) {
        socket.on("close", resolveClosed);
      }
    });
    await installStatefulSongRoutes(page);
    await page.goto(`/#/songs/${ENC}`);
    await page.getByRole("tab", { name: "Video" }).click();
    await expect(page.getByTestId("video-preview-start")).toBeEnabled();
    await page.getByRole("tab", { name: "Tracks" }).click();
    await expect(closed).resolves.toBe(true);
  });
});

test.describe("Video media pool", () => {
  test.use({ viewport: { width: 1280, height: 1000 } });

  test("drags media onto a new track and supports undo and redo", async ({
    page,
  }) => {
    const state = await openVideo(page);
    await page.getByText("Manual placement", { exact: true }).click();
    await expect(page.getByTestId("video-add-file")).not.toBeVisible();
    await page
      .getByTestId("video-media-item")
      .filter({ hasText: "camera-b.mp4" })
      .dragTo(page.getByTestId("video-new-track-drop"));
    await expect(page.getByTestId("video-track")).toHaveCount(2);
    await expect(page.getByTestId("video-clip")).toHaveCount(2);
    await page.getByRole("button", { name: "Undo video edit" }).click();
    await expect(page.getByTestId("video-track")).toHaveCount(1);
    await page.getByRole("button", { name: "Redo video edit" }).click();
    await expect(page.getByTestId("video-track")).toHaveCount(2);
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => state.writes.length).toBe(1);
    expect(parse(state.currentYaml()).video.tracks[1].clips[0].file).toBe(
      "camera-b.mp4",
    );
    expect(parse(state.currentYaml()).tracks[0].file).toBe("kick.wav");
  });

  test("snaps a dropped file to an edit and rejects overlap without losing media", async ({
    page,
  }) => {
    await openVideo(page);
    await page.getByText("Manual placement", { exact: true }).click();
    const file = page
      .getByTestId("video-media-item")
      .filter({ hasText: "camera-b.mp4" });
    const lane = page.getByTestId("video-lane");
    await file.dragTo(lane, { targetPosition: { x: 104, y: 25 } });
    await expect(page.getByTestId("video-clip-start")).toHaveValue("10");
    await expect(page.getByTestId("video-clip")).toHaveCount(2);
    await file.dragTo(lane, { targetPosition: { x: 50, y: 25 } });
    await expect(page.getByTestId("video-error")).toContainText("overlaps");
    await expect(page.getByTestId("video-clip")).toHaveCount(2);
    await expect(file).toBeVisible();
  });

  test("moves between tracks and preserves composition properties when saved", async ({
    page,
  }) => {
    const yaml = INITIAL_YAML.replace(
      "source_in: 0",
      "source_in: 0\n          x: 32\n          width: 320\n          opacity: 0.6\n          fade_in: 1",
    );
    const state = await openVideo(page, yaml);
    await page.getByText("Manual placement", { exact: true }).click();
    await page.getByTestId("video-add-track").click();
    await page.getByTestId("video-clip").scrollIntoViewIfNeeded();
    const source = await page.getByTestId("video-clip").boundingBox();
    const target = await page.getByTestId("video-lane").nth(1).boundingBox();
    if (!source || !target) throw new Error("missing timeline geometry");
    await page.mouse.move(
      source.x + source.width / 2,
      source.y + source.height / 2,
    );
    await page.mouse.down();
    await page.mouse.move(source.x + source.width / 2 + 30, target.y + 25, {
      steps: 8,
    });
    await page.mouse.up();
    await expect(page.getByTestId("video-track")).toHaveCount(1);
    await expect(page.getByTestId("video-track-name")).toHaveValue("Video 2");
    await expect(page.getByTestId("video-clip-start")).toHaveValue("3");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => state.writes.length).toBe(1);
    expect(parse(state.currentYaml()).video.tracks[0].clips[0]).toMatchObject({
      x: 32,
      width: 320,
      opacity: 0.6,
      fade_in: 1,
      start: 3,
    });
  });

  test("imports into the song media pool, without adding an audio track", async ({
    page,
  }) => {
    const state = await openVideo(page);
    state.rpc.unary(ConfigService.method.beginUpload, {
      uploadId: "video-import",
      path: "/uploads/video-import",
    });
    state.rpc.unary(ConfigService.method.addTrackFiles, {
      song: SONG,
      files: [{ file: "new.webm", replaced: false }],
    });
    await page.route("**/uploads/video-import", (route) =>
      route.fulfill({ status: 200, body: "" }),
    );
    await page.getByLabel("Import video files", { exact: true }).setInputFiles({
      name: "new.webm",
      mimeType: "video/webm",
      buffer: Buffer.from("fake video for upload transport test"),
    });
    await expect(
      page.getByTestId("video-media-item").filter({ hasText: "new.webm" }),
    ).toBeVisible();
    await expect(page.getByTestId("video-import-status")).toContainText(
      "1 video imported",
    );
    expect(state.rpc.calls(ConfigService.method.beginUpload)[0].song).toBe(
      SONG,
    );
    expect(state.writes).toHaveLength(0);
    await expect(page.getByTestId("video-clip")).toHaveCount(1);
  });

  test("refuses duplicate import and leaves existing files intact", async ({
    page,
  }) => {
    const state = await openVideo(page);
    await page.getByLabel("Import video files", { exact: true }).setInputFiles({
      name: "opening.mp4",
      mimeType: "video/mp4",
      buffer: Buffer.from("duplicate"),
    });
    await expect(page.getByTestId("video-import-status")).toContainText(
      "kept the existing file",
    );
    expect(state.rpc.calls(ConfigService.method.beginUpload)).toHaveLength(0);
  });
});

test("video ruler seeks the preview and moves its playhead", async ({
  page,
}) => {
  await openVideo(page);
  await page.getByTestId("video-preview-start").click();
  await expect(page.getByTestId("video-preview-image")).toBeVisible();
  await page
    .getByRole("button", { name: "Seek video timeline" })
    .click({ position: { x: 50, y: 10 } });
  await expect(page.getByTestId("video-preview-time")).toContainText(
    "00:05.000",
  );
  await expect(page.locator(".ruler-playhead")).toHaveCSS("left", "50px");
});

test.describe("Live video sources", () => {
  test.use({ viewport: { width: 1280, height: 1100 } });
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/video/sources", (route) =>
      route.fulfill({
        json: {
          live_capture: true,
          devices: [
            {
              name: "HDMI Input 1",
              device: "/dev/v4l/by-id/usb-capture-video-index0",
            },
          ],
        },
      }),
    );
  });
  test("drags a camera to the timeline, persists its source and supports undo", async ({
    page,
  }) => {
    const state = await openVideo(page);
    await page.getByText("Manual placement", { exact: true }).click();
    await page.getByRole("tab", { name: "Live sources", exact: true }).click();
    await page
      .getByRole("button", { name: /HDMI Input 1/ })
      .dragTo(page.getByTestId("video-new-track-drop"));
    await expect(page.getByTestId("video-clip")).toHaveCount(2);
    await expect(page.getByTestId("video-clip-file")).toHaveValue(
      "LIVE /dev/v4l/by-id/usb-capture-video-index0",
    );
    await expect(page.getByTestId("video-clip-source-in")).toBeDisabled();
    await page.getByRole("button", { name: "Undo video edit" }).click();
    await expect(page.getByTestId("video-clip")).toHaveCount(1);
    await page.getByRole("button", { name: "Redo video edit" }).click();
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => state.writes.length).toBe(1);
    const clip = parse(state.currentYaml()).video.tracks[1].clips[0];
    expect(clip.source).toEqual({
      type: "v4l2",
      device: "/dev/v4l/by-id/usb-capture-video-index0",
    });
    expect(clip.source_in).toBe(0);
    expect(clip.file || "").toBe("");
  });
  test("live clips survive reload and trimming does not add a media offset", async ({
    page,
  }) => {
    const state = await openVideo(
      page,
      INITIAL_YAML.replace("file: opening.mp4", "source: {type: test_pattern}"),
    );
    await page.getByTestId("video-clip").click();
    await dragBy(page, page.getByTestId("video-trim-left"), 20);
    await expect(page.getByTestId("video-clip-source-in")).toHaveValue("0");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => state.writes.length).toBe(1);
    const clip = parse(state.currentYaml()).video.tracks[0].clips[0];
    expect(clip.source).toEqual({ type: "test_pattern" });
    expect(clip.start).toBeGreaterThan(0);
  });
});
