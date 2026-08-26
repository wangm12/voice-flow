import { describe, expect, it } from "vitest";
import {
  hudBorderBeamFor,
  hudOrbFor,
  hudStatusLabel,
  perceptualLevel,
  shouldMountHudOrb,
} from "./hudOrb";

describe("hudOrbFor", () => {
  it("uses breathing while recording", () => {
    expect(hudOrbFor("starting", "idle", { reduced: false, level: 0 })).toEqual({
      state: "breathing",
      paused: false,
      speed: 1.35,
      dim: false,
    });
    expect(hudOrbFor("recording", "idle", { reduced: false, level: 0.8 })).toEqual({
      state: "breathing",
      paused: false,
      speed: 1.35,
      dim: false,
    });
    expect(hudOrbFor("recording_limited", "idle", { reduced: false, level: 1 })).toEqual({
      state: "breathing",
      paused: false,
      speed: 1.35,
      dim: true,
    });
  });

  it("uses shaping across processing phases", () => {
    expect(hudOrbFor("processing", "asr", { reduced: false, level: 0 })).toEqual({
      state: "shaping",
      paused: false,
      speed: 1.2,
      dim: false,
    });
    expect(hudOrbFor("processing", "finalizing_audio", { reduced: false, level: 0 })?.state).toBe("shaping");
    expect(hudOrbFor("processing", "cleanup", { reduced: false, level: 0 })?.state).toBe("shaping");
    expect(hudOrbFor("processing", "delivery", { reduced: false, level: 0 })?.state).toBe("shaping");
    expect(
      hudOrbFor("processing", "asr", {
        reduced: false,
        level: 0,
        selectedActionState: "preparing_rewrite",
      })?.state,
    ).toBe("shaping");
  });

  it("keeps shaping while waiting to retry", () => {
    expect(hudOrbFor("rate_limited", "waiting_retry", { reduced: false, level: 0 })?.state).toBe("shaping");
    expect(hudOrbFor("processing", "waiting_retry", { reduced: false, level: 0 })?.state).toBe("shaping");
  });

  it("pauses a breathing orb for idle guidance and drops the orb on final HUD states", () => {
    expect(hudOrbFor("idle", "idle", { reduced: false, level: 0 })).toEqual({
      state: "breathing",
      paused: true,
      speed: 1.35,
      dim: false,
    });
    expect(hudOrbFor("copied", "idle", { reduced: false, level: 0 })).toBeNull();
    expect(hudOrbFor("unverified", "idle", { reduced: false, level: 0 })).toBeNull();
    expect(hudOrbFor("degraded", "idle", { reduced: false, level: 0 })).toBeNull();
    expect(hudOrbFor("done", "idle", { reduced: false, level: 0 })).toBeNull();
    expect(hudOrbFor("error", "idle", { reduced: false, level: 0 })).toBeNull();
    expect(hudOrbFor("history", "idle", { reduced: false, level: 0 })).toBeNull();
  });

  it("pauses every live orb when reduced motion is on", () => {
    expect(hudOrbFor("recording", "idle", { reduced: true, level: 0.8 })?.paused).toBe(true);
    expect(hudOrbFor("processing", "cleanup", { reduced: true, level: 0 })?.paused).toBe(true);
    expect(hudOrbFor("starting", "idle", { reduced: true, level: 0 })?.paused).toBe(true);
  });
});

describe("shouldMountHudOrb", () => {
  it("keeps idle empty unless a caption is showing", () => {
    expect(shouldMountHudOrb("idle", false)).toBe(false);
    expect(shouldMountHudOrb("idle", true)).toBe(true);
  });

  it("hides the orb on copied, history, error, and done", () => {
    expect(shouldMountHudOrb("copied", true)).toBe(false);
    expect(shouldMountHudOrb("history", true)).toBe(false);
    expect(shouldMountHudOrb("error", true)).toBe(false);
    expect(shouldMountHudOrb("done", true)).toBe(false);
    expect(shouldMountHudOrb("done", false)).toBe(false);
    expect(shouldMountHudOrb("unverified", true)).toBe(false);
    expect(shouldMountHudOrb("degraded", true)).toBe(false);
  });

  it("mounts orbs for live dictation states", () => {
    expect(shouldMountHudOrb("recording", false)).toBe(true);
    expect(shouldMountHudOrb("processing", false)).toBe(true);
  });
});

describe("perceptualLevel", () => {
  it("uses the loudest sample on a compressive curve", () => {
    expect(perceptualLevel([0.1, 0.4, 0.2])).toBeCloseTo(Math.pow(0.4, 0.58), 5);
    expect(perceptualLevel([])).toBe(0);
  });
});

describe("hudStatusLabel", () => {
  it("uses Listening while recording and Thinking while processing", () => {
    expect(hudStatusLabel("starting", hudOrbFor("starting", "idle", { reduced: false, level: 0 }))).toBe("Listening…");
    expect(hudStatusLabel("recording", hudOrbFor("recording", "idle", { reduced: false, level: 0 }))).toBe("Listening…");
    expect(hudStatusLabel("processing", hudOrbFor("processing", "asr", { reduced: false, level: 0 }))).toBe("Thinking…");
    expect(hudStatusLabel("processing", hudOrbFor("processing", "cleanup", { reduced: false, level: 0 }))).toBe("Thinking…");
    expect(hudStatusLabel("processing", hudOrbFor("processing", "delivery", { reduced: false, level: 0 }))).toBe("Thinking…");
    expect(hudStatusLabel("rate_limited", hudOrbFor("rate_limited", "idle", { reduced: false, level: 0 }))).toBe("Thinking…");
    expect(hudStatusLabel("done", hudOrbFor("done", "idle", { reduced: false, level: 0 }))).toBe("Done");
  });

  it("keeps terminal copy without an orb", () => {
    expect(hudStatusLabel("copied", null)).toBe("Copied");
    expect(hudStatusLabel("unverified", null)).toBe("Copied");
    expect(hudStatusLabel("history", null)).toBe("Saved");
    expect(hudStatusLabel("error", null)).toBe("Error");
    expect(hudStatusLabel("degraded", null)).toBe("Done");
    expect(hudStatusLabel("done", null, { doneCheck: true })).toBe("Done");
  });
});

describe("hudBorderBeamFor", () => {
  it("uses a colorful md beam while listening", () => {
    expect(hudBorderBeamFor("starting")).toEqual({ size: "md", colorVariant: "colorful", strength: 1, duration: 3.2 });
    expect(hudBorderBeamFor("recording")).toEqual({ size: "md", colorVariant: "colorful", strength: 1, duration: 3.2 });
    expect(hudBorderBeamFor("recording_limited")).toEqual({ size: "md", colorVariant: "colorful", strength: 1, duration: 3.2 });
  });

  it("uses a colorful line beam while thinking", () => {
    expect(hudBorderBeamFor("processing")).toEqual({ size: "line", colorVariant: "colorful", strength: 0.7 });
    expect(hudBorderBeamFor("rate_limited")).toEqual({ size: "line", colorVariant: "colorful", strength: 0.7 });
  });

  it("turns the beam off for idle and terminal HUD states", () => {
    expect(hudBorderBeamFor("idle")).toBeNull();
    expect(hudBorderBeamFor("copied")).toBeNull();
    expect(hudBorderBeamFor("done")).toBeNull();
    expect(hudBorderBeamFor("error")).toBeNull();
    expect(hudBorderBeamFor("history")).toBeNull();
    expect(hudBorderBeamFor("unverified")).toBeNull();
    expect(hudBorderBeamFor("degraded")).toBeNull();
  });
});
