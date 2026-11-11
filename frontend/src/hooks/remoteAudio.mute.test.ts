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
  state = "running";
  destination = {};
  createDynamicsCompressor() { return new FakeCompressor(); }
  createGain() { return new FakeGain(); }
  createMediaElementSource() { return new FakeSource(); }
  resume() { return Promise.resolve(); }
  close() { return Promise.resolve(); }
}

describe("RemotePlayer speaker mute", () => {
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
