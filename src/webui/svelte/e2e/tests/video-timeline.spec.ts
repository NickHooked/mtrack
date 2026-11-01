// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

import { expect, test, type Locator, type Page } from "@playwright/test";
import { parse } from "yaml";
import { SONGS } from "../mock-server/test-data";

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
  const encodedSong = encodeURIComponent(songName);

  await page.route(`**/api/songs/${encodedSong}`, async (route) => {
    const request = route.request();
    if (request.method() === "PUT") {
      yaml = request.postData() ?? "";
      writes.push(yaml);
      await route.fulfill({ status: 200, json: { status: "updated" } });
      return;
    }
    await route.fulfill({ status: 200, contentType: "text/yaml", body: yaml });
  });
  await page.route("**/api/songs", async (route) => {
    const songs = structuredClone(SONGS);
    if (!songs.songs.some((song) => song.name === songName)) {
      songs.songs.push({ ...songs.songs[0], name: songName });
    }
    await route.fulfill({ json: songs });
  });
  await page.route(`**/api/songs/${encodedSong}/files`, async (route) => {
    await route.fulfill({
      json: {
        files: [
          { name: "opening.mp4", type: "other" },
          { name: "camera-b.mp4", type: "other" },
        ],
      },
    });
  });

  return { writes, currentYaml: () => yaml };
}

async function openVideo(page: Page, yaml = INITIAL_YAML, songName = SONG) {
  const state = await installStatefulSongRoutes(page, yaml, songName);
  await page.goto(`/#/songs/${encodeURIComponent(songName)}`);
  await page.getByRole("tab", { name: "Video" }).click();
  await expect(page.getByTestId("video-timeline-editor")).toBeVisible();
  return state;
}

async function dragBy(page: Page, locator: Locator, deltaX: number) {
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
    await page.route("**/api/status", async (route) => {
      await route.fulfill({
        json: {
          build: { version: "test", git_hash: "test", build_time: "test" },
          hardware: {
            init_done: true,
            hostname: "test",
            profile: "test",
            audio: { status: "connected", name: "test" },
            midi: { status: "not_connected", name: null },
            dmx: { status: "not_connected", name: null },
            trigger: { status: "not_connected", name: null },
          },
          controllers: [],
          capabilities: { video_preview: false },
        },
      });
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
    await installStatefulSongRoutes(page);
    await page.route(`**/api/songs/${nextEnc}`, async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/yaml",
        body: INITIAL_YAML.replace(`name: ${SONG}`, `name: ${nextSong}`),
      });
    });
    await page.route(`**/api/songs/${nextEnc}/files`, async (route) => {
      await route.fulfill({ json: { files: [] } });
    });
    await page.route("**/api/songs", async (route) => {
      const songs = structuredClone(SONGS);
      songs.songs.push({ ...songs.songs[0], name: nextSong });
      await route.fulfill({ json: songs });
    });

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
