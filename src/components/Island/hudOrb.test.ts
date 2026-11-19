import { describe, expect, it } from "vitest";
import {
  DEFAULT_HUD_VISUALS,
  HUD_COLOR_VARIANTS,
  HUD_LABEL_FADE_MS,
  HUD_LABEL_IN_DELAY_MS,
  HUD_LABEL_IN_DURATION_MS,
  HUD_MOTION_MS,
  HUD_PROCESSING_BEAM,
  HUD_PROCESSING_STYLES,
  HUD_STARTUP_NOTICE_DELAY_MS,
  HUD_VOICE_BEAM,
  HUD_VOICE_STYLES,
  activeHudStatusLabel,
  formatHudElapsed,
  hudAudioLevel,
  hudBorderBeamFor,
  hudOrbFor,
  hudProcessingHueRotateDeg,
  hudStageTimerKey,
  hudStatusLabel,
  hudVoiceBeamFor,
  hudVoiceDriveLevel,
  normalizeHudVisuals,
  perceptualLevel,
  resolveProcessingBeamConfig,
  resolveVoiceBeamConfig,
  sampleDemoVoiceLevel,
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
    expect(HUD_LABEL_IN_DELAY_MS).toBe(0);
    expect(HUD_LABEL_IN_DURATION_MS).toBe(HUD_MOTION_MS);
    expect(HUD_LABEL_FADE_MS).toBe(HUD_MOTION_MS);
    expect(HUD_STARTUP_NOTICE_DELAY_MS).toBe(250);
    expect(HUD_VOICE_BEAM).toEqual(resolveVoiceBeamConfig(DEFAULT_HUD_VISUALS));
    expect(HUD_VOICE_BEAM.type).toBe("pill");
    expect(HUD_VOICE_BEAM.colorVariant).toBe("colorful");
    expect(HUD_VOICE_BEAM.flow).toBeGreaterThan(0);
    expect(HUD_VOICE_BEAM.attack).toBeLessThan(0.1);
    expect(HUD_VOICE_BEAM.release).toBeLessThan(0.25);
    expect(HUD_VOICE_BEAM.bloomHeight).toBeLessThan(1.5);
    expect(HUD_PROCESSING_BEAM).toEqual({
      size: "line",
      colorVariant: "colorful",
      strength: 1,
      duration: 2.4,
      brightness: 1.3,
    });
  });
});

describe("perceptualLevel", () => {
  it("uses the loudest sample on a compressive curve", () => {
    expect(perceptualLevel([0.1, 0.4, 0.2])).toBeCloseTo(Math.pow(0.4, 0.58), 5);
    expect(perceptualLevel([])).toBe(0);
  });
});

describe("active HUD evidence and timing", () => {
  it("suppresses startup wait copy briefly, collapses processing copy to 处理中, and keeps rate-limit retry", () => {
    const t = (source: string) => source;
    expect(activeHudStatusLabel("starting", "idle", false, t, { startupElapsedMs: 0 })).toBe("录音中");
    expect(activeHudStatusLabel("starting", "idle", false, t, { startupElapsedMs: 249 })).toBe("录音中");
    expect(activeHudStatusLabel("starting", "idle", false, t, { startupElapsedMs: 250 })).toBe("准备录音");
    expect(activeHudStatusLabel("recording", "idle", false, t)).toBe("录音中");
    for (const phase of ["finalizing_audio", "asr", "soniox_recovery", "cascade_accurate", "cleanup", "delivery"] as const) {
      expect(activeHudStatusLabel("processing", phase, true, t)).toBe("处理中");
      expect(activeHudStatusLabel("processing", phase, false, t)).toBe("处理中");
    }
    expect(activeHudStatusLabel("processing", "waiting_retry", true, t)).toBe("限流重试");
    expect(activeHudStatusLabel("processing", "waiting_retry", false, t)).toBe("处理中");
    expect(activeHudStatusLabel("rate_limited", "waiting_retry", true, t)).toBe("限流重试");
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
    expect(hudStageTimerKey(4, "recording", "finalizing_audio", false)).toBe(
      hudStageTimerKey(4, "recording_limited", "idle", false),
    );
    expect(formatHudElapsed(65_000)).toBe("01:05");
    expect(formatHudElapsed(3_661_000)).toBe("1:01:01");
  });
});

describe("hudStatusLabel", () => {
  it("uses localized state labels and a single processing label", () => {
    const t = (source: string) => source;
    expect(hudStatusLabel("starting", null, { startupElapsedMs: 0, translate: t })).toBe("录音中");
    expect(hudStatusLabel("starting", null, { startupElapsedMs: 250, translate: t })).toBe("准备录音");
    expect(hudStatusLabel("recording", null, { translate: t })).toBe("录音中");
    expect(hudStatusLabel("recording_limited", null, { translate: t })).toBe("录音已达上限");
    for (const phase of ["asr", "cleanup", "delivery", "waiting_retry"] as const) {
      expect(hudStatusLabel("processing", null, { phase, phaseConfirmed: false, translate: t })).toBe("处理中");
    }
    expect(hudStatusLabel("processing", null, { phase: "waiting_retry", phaseConfirmed: true, translate: t })).toBe("限流重试");
    expect(hudStatusLabel("rate_limited", null, { translate: t })).toBe("限流重试");
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

describe("hudVoiceBeamFor", () => {
  it("uses the colorful pill voice glow while listening", () => {
    expect(hudVoiceBeamFor("starting")).toEqual(HUD_VOICE_BEAM);
    expect(hudVoiceBeamFor("recording")).toEqual(HUD_VOICE_BEAM);
    expect(hudVoiceBeamFor("recording_limited")).toEqual(HUD_VOICE_BEAM);
  });

  it("turns the voice glow off during processing, idle, and terminal HUD states", () => {
    expect(hudVoiceBeamFor("processing")).toBeNull();
    expect(hudVoiceBeamFor("rate_limited")).toBeNull();
    expect(hudVoiceBeamFor("idle")).toBeNull();
    expect(hudVoiceBeamFor("copied")).toBeNull();
    expect(hudVoiceBeamFor("done")).toBeNull();
    expect(hudVoiceBeamFor("error")).toBeNull();
    expect(hudVoiceBeamFor("history")).toBeNull();
    expect(hudVoiceBeamFor("unverified")).toBeNull();
    expect(hudVoiceBeamFor("degraded")).toBeNull();
  });
});

describe("hudBorderBeamFor", () => {
  it("turns border beam off while listening now that voice-glow drives recording", () => {
    expect(hudBorderBeamFor("starting")).toBeNull();
    expect(hudBorderBeamFor("recording")).toBeNull();
    expect(hudBorderBeamFor("recording_limited")).toBeNull();
  });

  it("uses the colorful bottom line beam while processing and stops on rate-limit retry", () => {
    expect(hudBorderBeamFor("processing")).toEqual({
      size: "line",
      colorVariant: "colorful",
      strength: 1,
      duration: 2.4,
      brightness: 1.3,
    });
    expect(hudBorderBeamFor("rate_limited")).toBeNull();
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

  it("maps every HUD color variant to a border-beam-supported palette plus optional hue rotation", () => {
    const supported = new Set(["colorful", "mono", "ocean", "sunset"]);
    for (const variant of HUD_COLOR_VARIANTS) {
      const beam = hudBorderBeamFor("processing", { colorVariant: variant.id, processingStyle: "brilliant_sweep" });
      expect(beam).not.toBeNull();
      expect(supported.has(beam!.colorVariant)).toBe(true);
      expect(beam!.brightness).toBe(HUD_PROCESSING_STYLES.find((item) => item.id === "brilliant_sweep")!.config.brightness);
      if (["colorful", "ocean", "sunset", "mono"].includes(variant.id)) {
        expect(hudProcessingHueRotateDeg(variant.id)).toBe(0);
      } else {
        expect(hudProcessingHueRotateDeg(variant.id)).not.toBe(0);
      }
    }
  });
});

describe("hudVoiceDriveLevel & studio helpers", () => {
  it("tracks the latest sample rather than pinning inter-word valleys to an older peak", () => {
    const now = 10_000;
    const falling = [0.82, 0.76, 0.08];
    const peak = [0.2, 0.45, 0.82];
    expect(perceptualLevel(falling)).toBeCloseTo(Math.pow(0.82, 0.58), 5);
    const valleyDrive = hudVoiceDriveLevel(falling, now - 50, now);
    const peakDrive = hudVoiceDriveLevel(peak, now - 50, now);
    expect(valleyDrive).toBeLessThan(0.05);
    expect(peakDrive).toBeGreaterThan(0.65);
    expect(hudVoiceDriveLevel([0.015], now - 50, now)).toBe(0);
    expect(hudVoiceDriveLevel([0.85], now - 800, now)).toBe(0);
    expect(hudVoiceDriveLevel([0.45], now - 50, now, 1.5)).toBeGreaterThan(hudVoiceDriveLevel([0.45], now - 50, now, 0.5));
  });

  it("generates deterministic demo speech crests and inter-phrase pauses", () => {
    expect(sampleDemoVoiceLevel(792)).toBeGreaterThan(0.8);
    expect(sampleDemoVoiceLevel(2340)).toBeGreaterThan(0.85);
    expect(sampleDemoVoiceLevel(1800)).toBe(0);
    expect(sampleDemoVoiceLevel(3400)).toBe(0);
  });

  it("normalizes unknown HUD studio values back to defaults and exposes all presets", () => {
    expect(HUD_COLOR_VARIANTS).toHaveLength(8);
    expect(HUD_VOICE_STYLES).toHaveLength(4);
    expect(HUD_PROCESSING_STYLES).toHaveLength(3);
    expect(normalizeHudVisuals({
      hud_color_variant: "unknown",
      hud_processing_color_variant: "invalid",
      hud_voice_style: "invalid",
      hud_processing_style: "invalid",
      hud_voice_intensity: 5,
    })).toEqual({
      colorVariant: "colorful",
      processingColorVariant: "colorful",
      voiceStyle: "ripple",
      processingStyle: "flow_line",
      voiceIntensity: 1.5,
    });
    expect(normalizeHudVisuals({
      colorVariant: "ocean",
      processingColorVariant: "forest",
      hud_color_variant: "mono",
      hud_processing_color_variant: "gold",
    })).toEqual({
      colorVariant: "mono",
      processingColorVariant: "gold",
      voiceStyle: "ripple",
      processingStyle: "flow_line",
      voiceIntensity: 1,
    });
    expect(resolveVoiceBeamConfig({ colorVariant: "mono" }).bandColors).toEqual({
      core: "#ffffff",
      above: "#e5e5e5",
      mid: "#a3a3a3",
      below: "#737373",
    });
    expect(resolveVoiceBeamConfig({ colorVariant: "colorful" }).bandColors).toBeUndefined();
    expect(resolveProcessingBeamConfig({ colorVariant: "mono", processingColorVariant: "forest" }).colorVariant).toBe("ocean");
    expect(normalizeHudVisuals({
      colorVariant: "aurora" as never,
      voiceIntensity: Number.NaN,
    })).toEqual(DEFAULT_HUD_VISUALS);
  });
});
