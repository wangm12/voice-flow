import { describe, expect, it } from "vitest";
import { pillCaption, voicePillWidthForState } from "./voicePillTokens";
import { waveformScaleForLevel } from "./VoiceWaveform";

describe("voice pill state tokens", () => {
  it("surfaces the recording limit without relying on hover", () => {
    expect(pillCaption("recording_limited")).toBe("已达上限 · 按热键结束");
    expect(voicePillWidthForState("recording_limited")).toBeGreaterThan(104);
  });

  it("keeps error guidance available as persistent state copy", () => {
    expect(pillCaption("error")).toBe("语音输入失败，请重试");
  });

  it("does not surface clipboard delivery as HUD copy", () => {
    expect(pillCaption("copied")).toBeNull();
  });

  it("appends a retry countdown to the rate-limit caption", () => {
    expect(pillCaption("rate_limited", undefined, 8)).toContain("8");
    expect(pillCaption("rate_limited")).toBe("处理时间比平时长…");
  });

  it("maps a louder microphone level to a taller waveform bar", () => {
    expect(waveformScaleForLevel(0.8, 1)).toBeGreaterThan(waveformScaleForLevel(0.05, 1));
  });

  it("keeps normal speech in a visible dynamic range", () => {
    expect(waveformScaleForLevel(0.2, 1)).toBeGreaterThan(0.45);
    expect(waveformScaleForLevel(0, 1)).toBeGreaterThan(0.1);
  });
});
