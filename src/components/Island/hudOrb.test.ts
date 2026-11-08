import { describe, expect, it } from "vitest";
import {
  HUD_LABEL_FADE_MS,
  HUD_LABEL_IN_DELAY_MS,
  HUD_LABEL_IN_DURATION_MS,
  HUD_LABEL_OUT_MS,
  HUD_MOTION_MS,
  HUD_THINK_BEAM_FADE_MS,
  activeHudStatusLabel,
  formatHudElapsed,
  hudAudioLevel,
  hudBorderBeamFor,
  hudOrbFor,
  hudStageTimerKey,
  hudStatusLabel,
  perceptualLevel,
  shouldHideOnConfirmedPaste,
} from "./hudOrb";

describe("hudOrbFor", () => {
  it("restores the breathing orb while listening and pauses it without fresh microphone frames", () => {
    expect(hudOrbFor("starting", "idle", { reduced: false, level: 0, levelFresh: false })).toMatchObject({
      state: "breathing",
      paused: true,
    });
    expect(hudOrbFor("recording", "idle", { reduced: false, level: 0.8, levelFresh: true })).toMatchObject({
      state: "breathing",
      paused: false,
      dim: false,
    });
    expect(hudOrbFor("recording", "idle", { reduced: false, level: 0.8, levelFresh: false })?.paused).toBe(true);
    expect(hudOrbFor("recording_limited", "idle", { reduced: false, level: 1, levelFresh: true })).toMatchObject({
      state: "breathing",
      paused: false,
      dim: true,
    });
  });

  it("uses the working orb across processing phases", () => {
    expect(hudOrbFor("processing", "asr", { reduced: false, level: 0 })).toEqual({
      state: "working",
      paused: false,
      speed: 1,
      dim: false,
    });
    expect(hudOrbFor("processing", "finalizing_audio", { reduced: false, level: 0 })?.state).toBe("working");
    expect(hudOrbFor("processing", "cleanup", { reduced: false, level: 0 })?.state).toBe("working");
    expect(hudOrbFor("processing", "delivery", { reduced: false, level: 0 })?.state).toBe("working");
    expect(
      hudOrbFor("processing", "asr", {
        reduced: false,
        level: 0,
        selectedActionState: "preparing_rewrite",
      })?.state,
    ).toBe("working");
  });

  it("keeps the working orb while waiting to retry", () => {
    expect(hudOrbFor("rate_limited", "waiting_retry", { reduced: false, level: 0 })?.state).toBe("working");
    expect(hudOrbFor("processing", "waiting_retry", { reduced: false, level: 0 })?.state).toBe("working");
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
    expect(hudOrbFor("recording", "idle", { reduced: true, level: 0.8, levelFresh: true })?.paused).toBe(true);
    expect(hudOrbFor("processing", "cleanup", { reduced: true, level: 0 })?.paused).toBe(true);
    expect(hudOrbFor("starting", "idle", { reduced: true, level: 0, levelFresh: false })?.paused).toBe(true);
  });

  it("pauses live orbs while the page is hidden", () => {
    expect(hudOrbFor("recording", "idle", {
      reduced: false,
      pageVisible: false,
      level: 0.8,
      levelFresh: true,
    })?.paused).toBe(true);
    expect(hudOrbFor("processing", "cleanup", {
      reduced: false,
      pageVisible: false,
      level: 0,
    })?.paused).toBe(true);
  });

  it("keeps pill transitions in the shared 150–300ms motion window", () => {
    expect(HUD_MOTION_MS).toBe(220);
    expect(HUD_LABEL_OUT_MS).toBe(150);
    expect(HUD_LABEL_IN_DELAY_MS).toBe(0);
    expect(HUD_LABEL_IN_DURATION_MS).toBe(HUD_MOTION_MS);
    expect(HUD_LABEL_FADE_MS).toBe(HUD_MOTION_MS);
    expect(HUD_THINK_BEAM_FADE_MS).toBe(HUD_MOTION_MS);
  });
});

describe("perceptualLevel", () => {
  it("uses the loudest sample on a compressive curve", () => {
    expect(perceptualLevel([0.1, 0.4, 0.2])).toBeCloseTo(Math.pow(0.4, 0.58), 5);
    expect(perceptualLevel([])).toBe(0);
  });
});

describe("active HUD evidence and timing", () => {
  it("keeps processing copy on one label across supported phase events", () => {
    const t = (source: string) => source;
    expect(activeHudStatusLabel("recording", "idle", false, t)).toBe("录音中");
    for (const phase of ["asr", "cleanup", "finalizing_audio", "soniox_recovery", "cascade_accurate", "waiting_retry", "delivery"] as const) {
      expect(activeHudStatusLabel("processing", phase, true, t)).toBe("处理中");
      expect(activeHudStatusLabel("processing", phase, false, t)).toBe("处理中");
    }
    expect(activeHudStatusLabel("rate_limited", "waiting_retry", true, t)).toBe("处理中");
  });

  it("uses only real levels from the current 750ms freshness window", () => {
    expect(hudAudioLevel([0.8], 1_000, 1_000)).toEqual({
      fresh: true,
      level: Math.pow(0.8, 0.58),
    });
    expect(hudAudioLevel([0.8], 1_000, 1_750).fresh).toBe(true);
    expect(hudAudioLevel([0.8], 1_000, 1_751)).toEqual({ fresh: false, level: 0 });
    expect(hudAudioLevel([0.8], 1_001, 1_000)).toEqual({ fresh: false, level: 0 });
    expect(hudAudioLevel([], 1_000, 1_000)).toEqual({ fresh: true, level: 0 });
  });

  it("binds elapsed time to generation and stage and formats elapsed duration", () => {
    expect(hudStageTimerKey(4, "processing", "cleanup", true)).not.toBe(
      hudStageTimerKey(5, "processing", "cleanup", true),
    );
    expect(hudStageTimerKey(4, "processing", "cleanup", true)).not.toBe(
      hudStageTimerKey(4, "processing", "asr", true),
    );
    expect(formatHudElapsed(65_000)).toBe("01:05");
    expect(formatHudElapsed(3_661_000)).toBe("1:01:01");
  });
});

describe("hudStatusLabel", () => {
  it("uses localized state labels and a single processing label", () => {
    const t = (source: string) => source;
    expect(hudStatusLabel("starting", null, { translate: t })).toBe("准备录音");
    expect(hudStatusLabel("recording", null, { translate: t })).toBe("录音中");
    expect(hudStatusLabel("recording_limited", null, { translate: t })).toBe("录音已达上限");
    for (const phase of ["asr", "cleanup", "delivery", "waiting_retry"] as const) {
      expect(hudStatusLabel("processing", null, { phase, phaseConfirmed: false, translate: t })).toBe("处理中");
    }
    expect(hudStatusLabel("rate_limited", null, { translate: t })).toBe("处理中");
    expect(hudStatusLabel("done", null, { translate: t })).toBe("已完成");
  });

  it("hides only a completed, fallback-free paste and leaves onboarding visible", () => {
    expect(shouldHideOnConfirmedPaste({ state: "done", delivery_method: "paste" })).toBe(true);
    expect(shouldHideOnConfirmedPaste({ state: "done", delivery_method: "paste", fallback_reason: null })).toBe(true);
    expect(shouldHideOnConfirmedPaste({ state: "done", delivery_method: "paste", fallback_reason: "paste_unverified" })).toBe(false);
    expect(shouldHideOnConfirmedPaste({ state: "done", delivery_method: "clipboard" })).toBe(false);
    expect(shouldHideOnConfirmedPaste({ state: "done", delivery_method: "onboarding" })).toBe(false);
    expect(shouldHideOnConfirmedPaste({ state: "copied", delivery_method: "paste" })).toBe(false);
  });

  it("keeps terminal copy without an orb", () => {
    expect(hudStatusLabel("copied", null)).toBe("已复制");
    expect(hudStatusLabel("unverified", null)).toBe("结果未确认");
    expect(hudStatusLabel("history", null)).toBe("已保存到历史");
    expect(hudStatusLabel("error", null)).toBe("错误");
    expect(hudStatusLabel("degraded", null)).toBe("部分结果");
    expect(hudStatusLabel("done", null, { doneCheck: true })).toBe("已完成");
  });
});

describe("hudBorderBeamFor", () => {
  it("uses the restrained monochrome md beam while listening", () => {
    expect(hudBorderBeamFor("starting")).toEqual({ size: "md", colorVariant: "mono", strength: 1, duration: 1.96, brightness: 1.3 });
    expect(hudBorderBeamFor("recording")).toEqual({ size: "md", colorVariant: "mono", strength: 1, duration: 1.96, brightness: 1.3 });
    expect(hudBorderBeamFor("recording_limited")).toEqual({ size: "md", colorVariant: "mono", strength: 1, duration: 1.96, brightness: 1.3 });
  });

  it("uses the restrained monochrome line beam while thinking", () => {
    expect(hudBorderBeamFor("processing")).toEqual({ size: "line", colorVariant: "mono", strength: 0.65, duration: 3.1, brightness: 1.3 });
    expect(hudBorderBeamFor("rate_limited")).toEqual({ size: "line", colorVariant: "mono", strength: 0.65, duration: 3.1, brightness: 1.3 });
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
