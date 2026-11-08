import { describe, expect, it } from "vitest";
import {
  integerRetrySeconds,
  selectedActionCaption,
  voicePillHeight,
  voicePillLearnToastHeight,
  voicePillRecoveryHeight,
  voicePillRecoveryWidth,
  voicePillWidth,
  voicePillWindowHeight,
  voicePillWindowWidth,
} from "./voicePillTokens";
import { HUD_PILL_RADIUS_PX } from "./hudOrb";
import { waveformScaleForLevel } from "./VoiceWaveform";

describe("Beam v10 HUD tokens", () => {
  it("keeps the primary capsule, host, and secondary tray at their reviewed geometry", () => {
    expect([voicePillWidth, voicePillHeight, HUD_PILL_RADIUS_PX]).toEqual([220, 48, 24]);
    expect([voicePillWindowWidth, voicePillWindowHeight]).toEqual([244, 64]);
    expect([voicePillRecoveryWidth, voicePillRecoveryHeight]).toEqual([320, 116]);
    expect(voicePillLearnToastHeight).toBe(48);
  });

  it("keeps elapsed retries legible without exposing fractional seconds", () => {
    expect(integerRetrySeconds(2.1)).toBe(3);
    expect(integerRetrySeconds(0)).toBeNull();
    expect(integerRetrySeconds(Number.NaN)).toBeNull();
  });

  it("keeps selected-action guidance and level scaling deterministic", () => {
    expect(selectedActionCaption("waiting_for_selection")).toBe("等待选中文本");
    expect(selectedActionCaption("unknown-state")).toBeNull();
    expect(waveformScaleForLevel(0, 0)).toBeGreaterThan(0);
    expect(waveformScaleForLevel(1, 0)).toBeGreaterThan(waveformScaleForLevel(0, 0));
  });
});
