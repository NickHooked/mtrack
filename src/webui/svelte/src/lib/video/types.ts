// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

export interface VideoCanvas {
  width: number;
  height: number;
  frame_rate: number;
  background: string;
}

export type VideoSource =
  | { type: "v4l2"; device: string }
  | { type: "test_pattern" };

export interface VideoClip {
  source?: VideoSource;
  id: string;
  start: number;
  duration: number;
  file: string;
  source_in: number;
}

export interface VideoTrack {
  name: string;
  clips: VideoClip[];
}

export interface VideoConfig {
  canvas: VideoCanvas;
  tracks: VideoTrack[];
}

export const DEFAULT_VIDEO_CANVAS: VideoCanvas = {
  width: 1920,
  height: 1080,
  frame_rate: 50,
  background: "#000000",
};
