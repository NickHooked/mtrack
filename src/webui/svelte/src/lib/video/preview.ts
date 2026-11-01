// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

import type { VideoConfig } from "./types";

const HEADER_BYTES = 24;
const MINIMUM_JPEG_BYTES = 4;
const OPEN = 1;
const SESSION_REQUEST_ID = "session";

export type PreviewPlaybackState =
  | "stopped"
  | "playing"
  | "paused"
  | "shutting_down";
export type PreviewConnectionLevelState = "busy" | "disabled";
export type PreviewRuntimeState =
  | PreviewPlaybackState
  | PreviewConnectionLevelState;
type PreviewCommandType = "start" | "play" | "pause" | "seek" | "stop";
type PendingRequest = {
  sequence: number;
  command: PreviewCommandType;
};
export type PreviewConnectionState =
  | "disconnected"
  | "connecting"
  | "connected";

export interface DecodedPreviewFrame {
  epoch: bigint;
  positionMs: bigint;
  jpeg: Uint8Array;
}

export interface PreviewFrame extends DecodedPreviewFrame {
  url: string;
}

export interface PreviewStateMessage {
  type: "state";
  request_id: string | null;
  epoch: number;
  state: PreviewPlaybackState;
  position_ms: number;
}

export interface PreviewErrorMessage {
  type: "error";
  request_id: string | null;
  epoch: number;
  state: PreviewRuntimeState;
  position_ms: number;
  error: string;
}

export interface PreviewSocket {
  binaryType: BinaryType;
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

interface LocationLike {
  protocol: string;
  host: string;
}

export interface VideoPreviewClientOptions {
  socketFactory?: (url: string) => PreviewSocket;
  createObjectURL?: (blob: Blob) => string;
  revokeObjectURL?: (url: string) => void;
  onConnection?: (state: PreviewConnectionState) => void;
  onState?: (message: PreviewStateMessage) => void;
  onError?: (message: PreviewErrorMessage) => void;
  onFrame?: (frame: PreviewFrame) => void;
}

export function decodePreviewFrame(data: ArrayBuffer): DecodedPreviewFrame {
  if (data.byteLength < HEADER_BYTES) {
    throw new Error("video preview frame is shorter than its 24-byte header");
  }
  const bytes = new Uint8Array(data);
  if (
    bytes[0] !== 0x4d ||
    bytes[1] !== 0x54 ||
    bytes[2] !== 0x52 ||
    bytes[3] !== 0x4b
  ) {
    throw new Error("invalid video preview frame magic");
  }
  if (bytes[4] !== 1) {
    throw new Error("unsupported video preview frame version");
  }
  if (bytes[5] !== 0 || bytes[6] !== 0 || bytes[7] !== 0) {
    throw new Error("video preview frame reserved bytes must be zero");
  }
  const jpeg = bytes.subarray(HEADER_BYTES);
  if (
    jpeg.byteLength < MINIMUM_JPEG_BYTES ||
    jpeg[0] !== 0xff ||
    jpeg[1] !== 0xd8 ||
    jpeg[jpeg.byteLength - 2] !== 0xff ||
    jpeg[jpeg.byteLength - 1] !== 0xd9
  ) {
    throw new Error("video preview frame has an invalid JPEG payload");
  }
  const view = new DataView(data);
  return {
    epoch: view.getBigUint64(8),
    positionMs: view.getBigUint64(16),
    jpeg: jpeg.slice(),
  };
}

export function previewWebSocketUrl(
  songName: string,
  location: LocationLike = globalThis.location ?? {
    protocol: "http:",
    host: "localhost",
  },
): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}/api/songs/${encodeURIComponent(songName)}/video-preview/ws`;
}

export function isControlMessage(
  value: unknown,
): value is PreviewStateMessage | PreviewErrorMessage {
  if (!value || typeof value !== "object") return false;
  const message = value as Record<string, unknown>;
  const validEnvelope =
    (typeof message.request_id === "string" || message.request_id === null) &&
    typeof message.epoch === "number" &&
    Number.isSafeInteger(message.epoch) &&
    message.epoch >= 0 &&
    typeof message.position_ms === "number" &&
    Number.isSafeInteger(message.position_ms) &&
    message.position_ms >= 0;
  if (!validEnvelope) return false;

  if (isConnectionLevelState(message.state)) {
    return (
      message.type === "error" &&
      message.request_id === SESSION_REQUEST_ID &&
      typeof message.error === "string"
    );
  }
  if (!isPlaybackState(message.state)) return false;
  if (message.request_id === SESSION_REQUEST_ID) return false;
  return (
    message.type === "state" ||
    (message.type === "error" && typeof message.error === "string")
  );
}

function isPlaybackState(value: unknown): value is PreviewPlaybackState {
  return (
    value === "stopped" ||
    value === "playing" ||
    value === "paused" ||
    value === "shutting_down"
  );
}

function isConnectionLevelState(
  value: unknown,
): value is PreviewConnectionLevelState {
  return value === "busy" || value === "disabled";
}

export class VideoPreviewClient {
  private socket: PreviewSocket | null = null;
  private requestCounter = 0;
  private commandSequence = 0;
  private pendingRequests = new Map<string, PendingRequest>();
  private activeLifecycleRequest: string | null = null;
  private epoch = 0n;
  private frameUrl: string | null = null;
  private binarySerial = 0;
  private destroyed = false;
  private readonly options: Required<
    Pick<
      VideoPreviewClientOptions,
      "socketFactory" | "createObjectURL" | "revokeObjectURL"
    >
  > &
    VideoPreviewClientOptions;

  constructor(
    private readonly songName: string,
    options: VideoPreviewClientOptions = {},
  ) {
    this.options = {
      ...options,
      socketFactory: options.socketFactory ?? ((url) => new WebSocket(url)),
      createObjectURL:
        options.createObjectURL ?? ((blob) => URL.createObjectURL(blob)),
      revokeObjectURL:
        options.revokeObjectURL ?? ((url) => URL.revokeObjectURL(url)),
    };
  }

  connect(): void {
    if (this.socket || this.destroyed) return;
    this.options.onConnection?.("connecting");
    const socket = this.options.socketFactory(
      previewWebSocketUrl(this.songName),
    );
    socket.binaryType = "arraybuffer";
    socket.onopen = () => this.options.onConnection?.("connected");
    socket.onmessage = (event) => void this.receive(event.data);
    socket.onerror = () => {
      this.options.onError?.({
        type: "error",
        request_id: null,
        epoch: Number(this.epoch),
        state: "stopped",
        position_ms: 0,
        error: "Video preview connection failed.",
      });
    };
    socket.onclose = () => {
      if (this.socket === socket) this.socket = null;
      this.options.onConnection?.("disconnected");
    };
    this.socket = socket;
  }

  start(config: VideoConfig, positionMs: number, autoplay = false): string {
    return this.command("start", {
      config,
      position_ms: Math.max(0, Math.round(positionMs)),
      autoplay,
    });
  }

  play(): string {
    return this.command("play");
  }

  pause(): string {
    return this.command("pause");
  }

  seek(positionMs: number): string {
    return this.command("seek", {
      position_ms: Math.max(0, Math.round(positionMs)),
    });
  }

  stop(): string {
    return this.command("stop");
  }

  destroy(): void {
    if (this.destroyed) return;
    const socket = this.socket;
    if (socket?.readyState === OPEN) {
      try {
        this.stop();
      } catch {
        // Closing a socket can race this best-effort stop.
      }
    }
    this.destroyed = true;
    this.binarySerial += 1;
    this.revokeFrameUrl();
    if (socket) {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      socket.close(1000, "video preview closed");
    }
    this.socket = null;
    this.pendingRequests.clear();
    this.activeLifecycleRequest = null;
    this.options.onConnection?.("disconnected");
  }

  private command(
    type: PreviewCommandType,
    fields: Record<string, unknown> = {},
  ): string {
    const socket = this.socket;
    if (!socket || socket.readyState !== OPEN) {
      throw new Error("Video preview is not connected.");
    }
    const requestId = `preview-${Date.now().toString(36)}-${++this.requestCounter}`;
    const sequence = ++this.commandSequence;
    // Commands are strictly ordered. Once a newer command is sent, no response
    // from an older pending command or playback lifecycle can mutate the UI.
    this.pendingRequests.clear();
    this.activeLifecycleRequest = null;
    this.pendingRequests.set(requestId, { sequence, command: type });
    socket.send(JSON.stringify({ type, request_id: requestId, ...fields }));
    return requestId;
  }

  private async receive(data: unknown): Promise<void> {
    if (typeof data === "string") {
      this.receiveControl(data);
      return;
    }
    const serial = ++this.binarySerial;
    let buffer: ArrayBuffer;
    if (data instanceof ArrayBuffer) {
      buffer = data;
    } else if (data instanceof Blob) {
      buffer = await data.arrayBuffer();
    } else {
      return;
    }
    if (this.destroyed || serial !== this.binarySerial) return;
    try {
      const decoded = decodePreviewFrame(buffer);
      if (decoded.epoch < this.epoch) return;
      this.epoch = decoded.epoch;
      const jpeg = Uint8Array.from(decoded.jpeg).buffer;
      const url = this.options.createObjectURL(
        new Blob([jpeg], { type: "image/jpeg" }),
      );
      this.revokeFrameUrl();
      this.frameUrl = url;
      this.options.onFrame?.({ ...decoded, url });
    } catch (error) {
      this.options.onError?.({
        type: "error",
        request_id: null,
        epoch: Number(this.epoch),
        state: "stopped",
        position_ms: 0,
        error:
          error instanceof Error
            ? error.message
            : "Invalid video preview frame.",
      });
    }
  }

  private receiveControl(json: string): void {
    let value: unknown;
    try {
      value = JSON.parse(json);
    } catch {
      return;
    }
    if (!isControlMessage(value)) return;
    const requestId = value.request_id;
    if (requestId === SESSION_REQUEST_ID) {
      // Session errors are connection-level and must not alter transport or
      // command/lifecycle correlation.
      this.options.onError?.(value as PreviewErrorMessage);
      return;
    }
    if (requestId === null) {
      return;
    }

    const pending = this.pendingRequests.get(requestId);
    const isActiveLifecycle = requestId === this.activeLifecycleRequest;
    if (!pending && !isActiveLifecycle) return;
    if (pending && pending.sequence < this.commandSequence) return;

    const messageEpoch = BigInt(value.epoch);
    if (messageEpoch < this.epoch) return;
    if (
      isActiveLifecycle &&
      value.type === "state" &&
      value.state !== "stopped"
    ) {
      return;
    }

    if (pending) {
      this.pendingRequests.delete(requestId);
      const startsLifecycle =
        value.type === "state" &&
        value.state === "playing" &&
        (pending.command === "start" ||
          pending.command === "play" ||
          pending.command === "seek");
      if (startsLifecycle) this.activeLifecycleRequest = requestId;
    }
    if (
      value.type === "error" ||
      value.state === "paused" ||
      value.state === "stopped" ||
      value.state === "shutting_down"
    ) {
      this.activeLifecycleRequest = null;
    }
    this.epoch = messageEpoch;
    if (value.type === "error") this.options.onError?.(value);
    else this.options.onState?.(value);
  }

  private revokeFrameUrl(): void {
    if (!this.frameUrl) return;
    this.options.revokeObjectURL(this.frameUrl);
    this.frameUrl = null;
  }
}
