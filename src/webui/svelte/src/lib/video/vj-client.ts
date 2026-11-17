// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
import type { MidiInput, PerformanceState } from "./vj";
export interface VjStatus {
  running: boolean;
  starting: boolean;
  state?: PerformanceState;
  fps?: number;
  error?: string | null;
}
export class VjClient {
  private socket: WebSocket;
  private url: string | null = null;
  constructor(
    song: string,
    private callbacks: {
      status: (s: VjStatus) => void;
      frame: (url: string | null) => void;
      error: (error: string) => void;
      midi: (device: string, input: MidiInput) => void;
      connection: (connected: boolean) => void;
    },
  ) {
    this.socket = new WebSocket(
      `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/api/songs/${encodeURIComponent(song)}/vj/ws`,
    );
    this.socket.binaryType = "blob";
    this.socket.onopen = () => callbacks.connection(true);
    this.socket.onclose = () => {
      callbacks.connection(false);
      callbacks.status({ running: false, starting: false });
      this.clear();
    };
    this.socket.onerror = () => callbacks.error("Cannot connect to VJ output.");
    this.socket.onmessage = (e) => {
      if (typeof e.data !== "string") {
        const previous = this.url;
        this.url = URL.createObjectURL(e.data);
        callbacks.frame(this.url);
        if (previous) URL.revokeObjectURL(previous);
        return;
      }
      try {
        const message = JSON.parse(e.data);
        if (message.type === "state") {
          callbacks.status(message);
          if (!message.running && !message.starting) this.clear();
        } else if (message.type === "error") callbacks.error(message.error);
        else if (message.type === "midi" && message.input)
          callbacks.midi(message.device, message.input);
      } catch {
        callbacks.error("Invalid VJ response.");
      }
    };
  }
  send(command: unknown) {
    if (this.socket.readyState !== WebSocket.OPEN) return false;
    this.socket.send(JSON.stringify(command));
    return true;
  }
  private clear() {
    this.callbacks.frame(null);
    if (this.url) URL.revokeObjectURL(this.url);
    this.url = null;
  }
  destroy() {
    this.socket.onopen = null;
    this.socket.onerror = null;
    this.socket.onclose = null;
    this.socket.onmessage = null;
    this.socket.close();
    this.clear();
  }
}
