import { describe, expect, it, vi } from "vitest";
import { RemotePlayer } from "./remoteAudio";

class FakeGain {
  gain = { value: 1 };
  connect = vi.fn();
  disconnect = vi.fn();
}
class FakeCompressor {
  threshold = { value: 0 };
  knee = { value: 0 };
  ratio = { value: 1 };
  attack = { value: 0 };
  release = { value: 0 };
  connect = vi.fn();
  disconnect = vi.fn();
}
class FakeSource {
  connect = vi.fn();
  disconnect = vi.fn();
}
class FakeAudioContext {
  static last: FakeAudioContext | null = null;
  state = "running";
  destination = {};
  gains: FakeGain[] = [];
  constructor() {
    FakeAudioContext.last = this;
  }
  createDynamicsCompressor() { return new FakeCompressor(); }
  createGain() {
    const gain = new FakeGain();
    this.gains.push(gain);
    return gain;
  }
  createMediaElementSource() { return new FakeSource(); }
  resume() { return Promise.resolve(); }
  close() { return Promise.resolve(); }
}

describe("RemotePlayer speaker mute", () => {
  it("applies the per-user volume to the actual gain node", () => {
    const original = globalThis.AudioContext;
    globalThis.AudioContext = FakeAudioContext as unknown as typeof AudioContext;
    try {
      const player = new RemotePlayer();
      const el = document.createElement("audio");
      player.attach("u1", el);
      const gain = FakeAudioContext.last?.gains[0];
      expect(gain).toBeDefined();

      player.setVolume("u1", 0);
      expect(gain?.gain.value).toBe(0);
      player.setVolume("u1", 3);
      expect(gain?.gain.value).toBe(3);
      player.close();
    } finally {
      globalThis.AudioContext = original;
    }
  });

  it("mutes and restores the native media element as a mobile fallback", () => {
    const original = globalThis.AudioContext;
    globalThis.AudioContext = FakeAudioContext as unknown as typeof AudioContext;
    try {
      const player = new RemotePlayer();
      const el = document.createElement("audio");
      player.attach("u1", el);
      player.setSpeakerMuted(true);
      expect(el.muted).toBe(true);
      expect(el.volume).toBe(0);
      player.setSpeakerMuted(false);
      expect(el.muted).toBe(false);
      expect(el.volume).toBe(1);
      player.close();
    } finally {
      globalThis.AudioContext = original;
    }
  });
});
