// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

import {
  DEFAULT_VIDEO_CANVAS,
  type VideoClip,
  type VideoConfig,
  type VideoTrack,
} from "./types";

export const MIN_CLIP_DURATION = 0.01;
const EPSILON = 1e-9;

function roundTimelineValue(value: number): number {
  return Math.round(value * 1000) / 1000;
}

function finiteNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function positiveNumber(value: unknown, fallback: number): number {
  const number = finiteNumber(value, fallback);
  return number > 0 ? number : fallback;
}

export function parseVideoConfig(value: unknown): VideoConfig | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const canvas =
    raw.canvas && typeof raw.canvas === "object"
      ? (raw.canvas as Record<string, unknown>)
      : {};
  const tracks = Array.isArray(raw.tracks) ? raw.tracks : [];
  const reservedIds = new Set<string>();
  for (const entry of tracks) {
    if (!entry || typeof entry !== "object") continue;
    const clips = Array.isArray((entry as Record<string, unknown>).clips)
      ? ((entry as Record<string, unknown>).clips as unknown[])
      : [];
    for (const clipEntry of clips) {
      if (!clipEntry || typeof clipEntry !== "object") continue;
      const id = (clipEntry as Record<string, unknown>).id;
      if (typeof id === "string" && id.trim()) reservedIds.add(id);
    }
  }
  const usedIds = new Set<string>();

  function normalizedId(value: unknown, trackIndex: number, clipIndex: number) {
    if (typeof value === "string" && value.trim() && !usedIds.has(value)) {
      usedIds.add(value);
      return value;
    }
    const root = `video-${trackIndex + 1}-${clipIndex + 1}`;
    let id = root;
    let suffix = 2;
    while (reservedIds.has(id) || usedIds.has(id)) id = `${root}-${suffix++}`;
    usedIds.add(id);
    return id;
  }

  return {
    canvas: {
      width: positiveNumber(canvas.width, DEFAULT_VIDEO_CANVAS.width),
      height: positiveNumber(canvas.height, DEFAULT_VIDEO_CANVAS.height),
      frame_rate: positiveNumber(
        canvas.frame_rate,
        DEFAULT_VIDEO_CANVAS.frame_rate,
      ),
      background:
        typeof canvas.background === "string" &&
        /^#[0-9a-fA-F]{6}$/.test(canvas.background)
          ? canvas.background
          : DEFAULT_VIDEO_CANVAS.background,
    },
    tracks: tracks.map((entry, trackIndex) => {
      const track =
        entry && typeof entry === "object"
          ? (entry as Record<string, unknown>)
          : {};
      const clips = Array.isArray(track.clips) ? track.clips : [];
      return {
        name:
          typeof track.name === "string" && track.name.trim()
            ? track.name
            : `Video ${trackIndex + 1}`,
        clips: clips.map((clipEntry, clipIndex) => {
          const clip =
            clipEntry && typeof clipEntry === "object"
              ? (clipEntry as Record<string, unknown>)
              : {};
          return {
            id: normalizedId(clip.id, trackIndex, clipIndex),
            start: Math.max(0, finiteNumber(clip.start, 0)),
            duration: Math.max(
              MIN_CLIP_DURATION,
              finiteNumber(clip.duration, 1),
            ),
            file: typeof clip.file === "string" ? clip.file : "",
            source_in: Math.max(0, finiteNumber(clip.source_in, 0)),
          };
        }),
      };
    }),
  };
}

export function createVideoConfig(): VideoConfig {
  return { canvas: { ...DEFAULT_VIDEO_CANVAS }, tracks: [] };
}

function isSafeRelativePath(value: string): boolean {
  const normalized = value.replace(/\\/g, "/");
  return (
    !normalized.startsWith("/") &&
    !/^[a-zA-Z]:/.test(normalized) &&
    normalized
      .split("/")
      .every((part) => part !== "" && part !== "." && part !== "..")
  );
}

export function videoConfigError(config: VideoConfig): string | null {
  const ids = new Set<string>();
  for (const track of config.tracks) {
    if (!track.name.trim()) return "Name every video track before saving.";
    if (track.clips.length === 0) {
      return `Add a clip to “${track.name}” before saving, or remove the empty track.`;
    }
    for (const clip of track.clips) {
      if (!clip.id.trim() || ids.has(clip.id)) {
        return "Each video clip needs a unique ID before saving.";
      }
      ids.add(clip.id);
      if (!Number.isFinite(clip.start) || clip.start < 0) {
        return `Set a valid start time for “${clip.file || clip.id}” before saving.`;
      }
      if (!Number.isFinite(clip.duration) || clip.duration <= 0) {
        return `Set a positive duration for “${clip.file || clip.id}” before saving.`;
      }
      if (!Number.isFinite(clip.start + clip.duration)) {
        return `Shorten “${clip.file || clip.id}” before saving.`;
      }
      if (!Number.isFinite(clip.source_in) || clip.source_in < 0) {
        return `Set a valid source in for “${clip.file || clip.id}” before saving.`;
      }
      if (!clip.file.trim() || !isSafeRelativePath(clip.file)) {
        return `Choose a safe relative file for “${clip.id}” before saving.`;
      }
    }
    for (let index = 0; index < track.clips.length; index++) {
      for (let other = index + 1; other < track.clips.length; other++) {
        if (clipsOverlap(track.clips[index], track.clips[other])) {
          return `Resolve overlapping clips on “${track.name}” before saving.`;
        }
      }
    }
  }
  return null;
}

export function videoEnd(config: VideoConfig): number {
  return config.tracks.reduce(
    (end, track) =>
      track.clips.reduce(
        (trackEnd, clip) => Math.max(trackEnd, clip.start + clip.duration),
        end,
      ),
    0,
  );
}

export function clipsOverlap(left: VideoClip, right: VideoClip): boolean {
  return (
    left.start + left.duration - right.start > EPSILON &&
    right.start + right.duration - left.start > EPSILON
  );
}

export function overlapError(
  track: VideoTrack,
  candidate: VideoClip,
  ignoreId?: string,
): string | null {
  const conflict = track.clips.find(
    (clip) => clip.id !== ignoreId && clipsOverlap(clip, candidate),
  );
  return conflict
    ? `Clip overlaps “${conflict.file || conflict.id}” on this track.`
    : null;
}

export function uniqueClipId(config: VideoConfig, file: string): string {
  const used = new Set(
    config.tracks.flatMap((track) => track.clips.map((clip) => clip.id)),
  );
  const basename = file.split(/[\\/]/).pop() ?? "clip";
  const root =
    basename
      .replace(/\.[^.]+$/, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "clip";
  if (!used.has(root)) return root;
  let suffix = 2;
  while (used.has(`${root}-${suffix}`)) suffix++;
  return `${root}-${suffix}`;
}

export function moveClip(clip: VideoClip, start: number): VideoClip {
  return { ...clip, start: roundTimelineValue(Math.max(0, start)) };
}

export function trimClipLeft(clip: VideoClip, newStart: number): VideoClip {
  const minimumStart = Math.max(0, clip.start - clip.source_in);
  const maximumStart = clip.start + clip.duration - MIN_CLIP_DURATION;
  const start = roundTimelineValue(
    Math.min(maximumStart, Math.max(minimumStart, newStart)),
  );
  const delta = start - clip.start;
  return {
    ...clip,
    start,
    duration: roundTimelineValue(
      Math.max(MIN_CLIP_DURATION, clip.duration - delta),
    ),
    source_in: roundTimelineValue(Math.max(0, clip.source_in + delta)),
  };
}

export function trimClipRight(clip: VideoClip, newEnd: number): VideoClip {
  return {
    ...clip,
    duration: roundTimelineValue(
      Math.max(MIN_CLIP_DURATION, newEnd - clip.start),
    ),
  };
}

export function normalizeClipPatch(
  clip: VideoClip,
  patch: Partial<VideoClip>,
): VideoClip {
  return {
    ...clip,
    ...patch,
    start: Math.max(0, finiteNumber(patch.start ?? clip.start, clip.start)),
    duration: Math.max(
      MIN_CLIP_DURATION,
      finiteNumber(patch.duration ?? clip.duration, clip.duration),
    ),
    source_in: Math.max(
      0,
      finiteNumber(patch.source_in ?? clip.source_in, clip.source_in),
    ),
  };
}
