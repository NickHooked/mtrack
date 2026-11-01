// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.

import { expect, test } from "@playwright/test";
import {
  decodePreviewFrame,
  isControlMessage,
  previewWebSocketUrl,
  VideoPreviewClient,
  type PreviewSocket,
} from "../../src/lib/video/preview";
import type { VideoConfig } from "../../src/lib/video/types";

const CONFIG: VideoConfig = {
  canvas: { width: 1920, height: 1080, frame_rate: 50, background: "#000000" },
  tracks: [],
};

function frame(
  epoch: bigint,
  position: bigint,
  jpeg = [0xff, 0xd8, 0xff, 0xd9],
) {
  const bytes = new Uint8Array(24 + jpeg.length);
  bytes.set([0x4d, 0x54, 0x52, 0x4b, 1, 0, 0, 0]);
  const view = new DataView(bytes.buffer);
  view.setBigUint64(8, epoch);
  view.setBigUint64(16, position);
  bytes.set(jpeg, 24);
  return bytes.buffer;
}

class FakeSocket implements PreviewSocket {
  static readonly OPEN = 1;
  binaryType = "blob";
  readyState = FakeSocket.OPEN;
  sent: string[] = [];
  closed = false;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
    this.readyState = 3;
  }
  open() {
    this.onopen?.(new Event("open"));
  }
  serverClose() {
    this.readyState = 3;
    this.onclose?.(new CloseEvent("close"));
  }
  message(data: string | ArrayBuffer) {
    this.onmessage?.({ data } as MessageEvent);
  }
}

test.describe("video preview protocol client", () => {
  test("reports connecting, connected, and a server close", () => {
    const socket = new FakeSocket();
    const connections: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onConnection: (state) => connections.push(state),
    });

    client.connect();
    socket.open();
    socket.serverClose();

    expect(connections).toEqual(["connecting", "connected", "disconnected"]);
  });

  test("validates the exact MTRK header and decodes big-endian fields", () => {
    expect(decodePreviewFrame(frame(7n, 1234n))).toMatchObject({
      epoch: 7n,
      positionMs: 1234n,
    });
    expect(() => decodePreviewFrame(new Uint8Array(23).buffer)).toThrow(
      /24-byte header/i,
    );
    const badMagic = new Uint8Array(frame(1n, 2n));
    badMagic[0] = 0;
    expect(() => decodePreviewFrame(badMagic.buffer)).toThrow(/magic/i);
    const badVersion = new Uint8Array(frame(1n, 2n));
    badVersion[4] = 2;
    expect(() => decodePreviewFrame(badVersion.buffer)).toThrow(/version/i);
    const badReserved = new Uint8Array(frame(1n, 2n));
    badReserved[6] = 1;
    expect(() => decodePreviewFrame(badReserved.buffer)).toThrow(/reserved/i);
  });

  test("rejects empty, arbitrary, and unterminated JPEG payloads", () => {
    expect(() => decodePreviewFrame(frame(1n, 2n, []))).toThrow(/jpeg/i);
    expect(() => decodePreviewFrame(frame(1n, 2n, [1, 2, 3, 4]))).toThrow(
      /jpeg/i,
    );
    expect(() => decodePreviewFrame(frame(1n, 2n, [0xff, 0xd8, 1, 2]))).toThrow(
      /jpeg/i,
    );
    expect(() => decodePreviewFrame(frame(1n, 2n, [1, 2, 0xff, 0xd9]))).toThrow(
      /jpeg/i,
    );
  });

  test("accepts a deterministic JPEG payload with SOI and EOI markers", () => {
    const jpeg = [
      0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46, 0x49, 0x46, 0xff, 0xd9,
    ];
    expect(Array.from(decodePreviewFrame(frame(1n, 2n, jpeg)).jpeg)).toEqual(
      jpeg,
    );
  });

  test("accepts only Rust's exact session-level busy and disabled messages", () => {
    for (const state of ["stopped", "playing", "paused", "shutting_down"]) {
      expect(
        isControlMessage({
          type: "state",
          request_id: null,
          epoch: 1,
          state,
          position_ms: 2,
        }),
      ).toBe(true);
      expect(
        isControlMessage({
          type: "error",
          request_id: "request",
          epoch: 1,
          state,
          position_ms: 2,
          error: "failed",
        }),
      ).toBe(true);
    }
    for (const state of ["busy", "disabled"]) {
      expect(
        isControlMessage({
          type: "error",
          request_id: "session",
          epoch: 0,
          state,
          position_ms: 0,
          error: `${state} preview`,
        }),
      ).toBe(true);
    }
    for (const request_id of [null, "unknown"]) {
      expect(
        isControlMessage({
          type: "error",
          request_id,
          epoch: 0,
          state: "busy",
          position_ms: 0,
          error: "busy preview",
        }),
      ).toBe(false);
    }
    expect(
      isControlMessage({
        type: "error",
        request_id: null,
        epoch: 0,
        state: "unknown",
        position_ms: 0,
        error: "unknown preview state",
      }),
    ).toBe(false);
  });

  test("builds the dedicated encoded song websocket URL", () => {
    expect(
      previewWebSocketUrl("A/B & C", {
        protocol: "https:",
        host: "mtrack.local:8443",
      }),
    ).toBe(
      "wss://mtrack.local:8443/api/songs/A%2FB%20%26%20C/video-preview/ws",
    );
  });

  test("uses unique request IDs and ignores stale epochs and old responses", async () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const positions: bigint[] = [];
    const urls: string[] = [];
    const revoked: string[] = [];
    let nextUrl = 0;
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      createObjectURL: () => {
        const url = `blob:${++nextUrl}`;
        urls.push(url);
        return url;
      },
      revokeObjectURL: (url) => revoked.push(url),
      onState: (state) => states.push(state.state),
      onFrame: (value) => positions.push(value.positionMs),
    });
    client.connect();
    socket.open();
    const startId = client.start(CONFIG, 250, false);
    const playId = client.play();
    expect(startId).not.toBe(playId);
    expect(startId).not.toBe("session");
    expect(playId).not.toBe("session");
    expect(socket.sent.map((value) => JSON.parse(value).type)).toEqual([
      "start",
      "play",
    ]);

    socket.message(
      JSON.stringify({
        type: "state",
        request_id: playId,
        epoch: 2,
        state: "playing",
        position_ms: 300,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: startId,
        epoch: 1,
        state: "paused",
        position_ms: 250,
      }),
    );
    socket.message(frame(2n, 301n));
    await expect.poll(() => positions.length).toBe(1);
    socket.message(frame(1n, 99n));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(states.at(-1)).toBe("playing");
    expect(positions).toEqual([301n]);

    socket.message(frame(2n, 302n));
    await expect.poll(() => positions.length).toBe(2);
    expect(revoked).toEqual([urls[0]]);
    client.destroy();
    expect(socket.closed).toBe(true);
    expect(JSON.parse(socket.sent.at(-1) ?? "{}").type).toBe("stop");
    expect(revoked).toEqual([urls[0], urls[1]]);
  });

  test("rejects unknown request IDs at the current and newer epoch", () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
    });
    client.connect();
    for (const [request_id, epoch, state] of [
      ["unknown", 0, "paused"],
      ["unknown-newer", 10, "playing"],
    ] as const) {
      socket.message(
        JSON.stringify({
          type: "state",
          request_id,
          epoch,
          state,
          position_ms: 0,
        }),
      );
    }
    expect(states).toEqual([]);
  });

  test("rejects null-ID transport controls without mutating epoch or position", async () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const errors: string[] = [];
    const positions: bigint[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      createObjectURL: () => "blob:frame",
      onState: (message) => states.push(message.state),
      onError: (message) => errors.push(message.error),
      onFrame: (value) => positions.push(value.positionMs),
    });
    client.connect();

    for (const message of [
      {
        type: "state",
        request_id: null,
        epoch: 20,
        state: "playing",
        position_ms: 200,
      },
      {
        type: "state",
        request_id: null,
        epoch: 21,
        state: "stopped",
        position_ms: 210,
      },
      {
        type: "error",
        request_id: null,
        epoch: 22,
        state: "stopped",
        position_ms: 220,
        error: "uncorrelated transport error",
      },
    ]) {
      socket.message(JSON.stringify(message));
    }
    socket.message(frame(1n, 17n));
    await expect.poll(() => positions).toEqual([17n]);
    expect(states).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("accepts session busy and disabled without changing transport or command correlation", async () => {
    const socket = new FakeSocket();
    const errors: Array<{ state: string; error: string }> = [];
    const states: string[] = [];
    const positions: bigint[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      createObjectURL: () => "blob:frame",
      onError: ({ state, error }) => errors.push({ state, error }),
      onState: ({ state }) => states.push(state),
      onFrame: (value) => positions.push(value.positionMs),
    });
    client.connect();
    const requestId = client.play();

    socket.message(
      JSON.stringify({
        type: "error",
        request_id: "session",
        epoch: 99,
        state: "busy",
        position_ms: 999,
        error: "another video preview session is already active",
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 1,
        state: "playing",
        position_ms: 23,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "error",
        request_id: "session",
        epoch: 99,
        state: "disabled",
        position_ms: 999,
        error: "video preview is disabled in this build",
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 1,
        state: "stopped",
        position_ms: 23,
      }),
    );
    socket.message(frame(1n, 23n));
    await expect.poll(() => positions).toEqual([23n]);
    expect(errors).toEqual([
      {
        state: "busy",
        error: "another video preview session is already active",
      },
      {
        state: "disabled",
        error: "video preview is disabled in this build",
      },
    ]);
    expect(states).toEqual(["playing", "stopped"]);
  });

  test("rejects null and unknown busy errors plus session transport state", () => {
    const socket = new FakeSocket();
    const errors: string[] = [];
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onError: ({ error }) => errors.push(error),
      onState: ({ state }) => states.push(state),
    });
    client.connect();

    for (const request_id of [null, "unknown"]) {
      socket.message(
        JSON.stringify({
          type: "error",
          request_id,
          epoch: 99,
          state: "busy",
          position_ms: 999,
          error: "must be rejected",
        }),
      );
    }
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: "session",
        epoch: 99,
        state: "playing",
        position_ms: 999,
      }),
    );

    expect(errors).toEqual([]);
    expect(states).toEqual([]);
  });

  test("consumes terminal responses and rejects duplicates", () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
    });
    client.connect();
    const requestId = client.start(CONFIG, 0, false);
    const paused = JSON.stringify({
      type: "state",
      request_id: requestId,
      epoch: 1,
      state: "paused",
      position_ms: 0,
    });
    socket.message(paused);
    socket.message(paused);
    expect(states).toEqual(["paused"]);
  });

  test("accepts a correlated terminal state after a playing acknowledgement", () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
    });
    client.connect();
    const requestId = client.play();
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 0,
        state: "playing",
        position_ms: 0,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 0,
        state: "playing",
        position_ms: 1,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 0,
        state: "stopped",
        position_ms: 100,
      }),
    );
    expect(states).toEqual(["playing", "stopped"]);
  });

  test("accepts a correlated terminal error after a playing acknowledgement", () => {
    const socket = new FakeSocket();
    const errors: string[] = [];
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
      onError: (message) => errors.push(message.error),
    });
    client.connect();
    const requestId = client.seek(20);
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: requestId,
        epoch: 1,
        state: "playing",
        position_ms: 20,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "error",
        request_id: requestId,
        epoch: 1,
        state: "shutting_down",
        position_ms: 20,
        error: "decoder failed",
      }),
    );
    expect(states).toEqual(["playing"]);
    expect(errors).toEqual(["decoder failed"]);
  });

  test("rejects a superseded lifecycle response and old same-epoch response", () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
    });
    client.connect();
    const oldId = client.play();
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: oldId,
        epoch: 1,
        state: "playing",
        position_ms: 0,
      }),
    );
    const currentId = client.seek(50);
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: oldId,
        epoch: 1,
        state: "stopped",
        position_ms: 10,
      }),
    );
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: currentId,
        epoch: 1,
        state: "playing",
        position_ms: 50,
      }),
    );
    expect(states).toEqual(["playing", "playing"]);
  });

  test("retires superseded pending requests instead of retaining them", () => {
    const socket = new FakeSocket();
    const states: string[] = [];
    const client = new VideoPreviewClient("Song", {
      socketFactory: () => socket,
      onState: (message) => states.push(message.state),
    });
    client.connect();
    const retiredIds = Array.from({ length: 25 }, () => client.seek(10));
    const currentId = client.seek(20);
    for (const requestId of retiredIds) {
      socket.message(
        JSON.stringify({
          type: "state",
          request_id: requestId,
          epoch: 1,
          state: "paused",
          position_ms: 10,
        }),
      );
    }
    socket.message(
      JSON.stringify({
        type: "state",
        request_id: currentId,
        epoch: 1,
        state: "paused",
        position_ms: 20,
      }),
    );
    expect(states).toEqual(["paused"]);
  });
});
