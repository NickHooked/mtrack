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
) {
  let yaml = initialYaml;
  const writes: string[] = [];

  await page.route(`**/api/songs/${ENC}`, async (route) => {
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
    await route.fulfill({ json: structuredClone(SONGS) });
  });
  await page.route(`**/api/songs/${ENC}/files`, async (route) => {
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

async function openVideo(page: Page, yaml = INITIAL_YAML) {
  const state = await installStatefulSongRoutes(page, yaml);
  await page.goto(`/#/songs/${ENC}`);
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
});
