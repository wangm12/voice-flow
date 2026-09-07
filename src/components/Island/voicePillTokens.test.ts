import { describe, expect, it } from "vitest";
import {
  CONTEXT_LABEL_VISIBLE_MS,
  pillCaption,
  selectedActionCaption,
  visibleContextLabel,
  voicePillCaptionMaxWidthForPartial,
  voicePillCaptionNeedsWide,
  voicePillWidthForState,
  voicePillWindowWidth,
  voicePillWindowWidthForPartial,
  voicePillWindowHeight,
  voicePillHeight,
  voicePillCaptionHeight,
  voicePillCaptionGap,
  voicePillStagePaddingTop,
} from "./voicePillTokens";
import { waveformScaleForLevel } from "./VoiceWaveform";

describe("voice pill state tokens", () => {
  it("surfaces the recording limit without relying on hover", () => {
    expect(pillCaption("recording_limited")).toBe("已达上限 · 按热键结束");
    expect(voicePillWidthForState("recording_limited")).toBeGreaterThan(104);
    expect(voicePillWidthForState("recording")).toBe(132);
  });

  it("keeps error guidance available as persistent state copy", () => {
    expect(pillCaption("error")).toBe("语音输入失败，请重试");
  });

  it("surfaces clipboard delivery so the user can read what happened", () => {
    expect(pillCaption("copied")).toBe("已复制到剪贴板，请手动粘贴");
  });

  it("appends a retry countdown to the rate-limit caption", () => {
    expect(pillCaption("rate_limited", undefined, 8)).toContain("8");
    expect(pillCaption("rate_limited")).toBe("处理时间比平时长…");
  });

  it("surfaces selected-action captions while idle", () => {
    expect(
      pillCaption("idle", undefined, undefined, { selectedActionState: "waiting_for_selection" }),
    ).toBe("等待选中文本");
    expect(selectedActionCaption("accessibility_required")).toBe("需要辅助功能权限");
  });

  it("surfaces listening copy during recording without replacing waveform state", () => {
    expect(
      pillCaption("recording", undefined, undefined, { selectedActionState: "listening" }),
    ).toBe("正在听取操作");
    expect(pillCaption("recording")).toBeNull();
  });

  it("keeps the template caption visible for a couple of seconds then dismisses it", () => {
    expect(CONTEXT_LABEL_VISIBLE_MS).toBeGreaterThanOrEqual(2_000);
    expect(CONTEXT_LABEL_VISIBLE_MS).toBeLessThanOrEqual(3_000);
    expect(visibleContextLabel("Chrome · 通用", 1_000, 1_000 + 2_499)).toBe("Chrome · 通用");
    expect(visibleContextLabel("Chrome · 通用", 1_000, 1_000 + CONTEXT_LABEL_VISIBLE_MS)).toBeNull();
    expect(visibleContextLabel("Chrome · 通用", null, 1_000)).toBeNull();
  });

  it("surfaces the context label while recording, starting, or processing", () => {
    expect(
      pillCaption("recording", undefined, undefined, { contextLabel: "WeChat · 口语" }),
    ).toBe("WeChat · 口语");
    expect(
      pillCaption("starting", undefined, undefined, { contextLabel: "Slack · 工作短讯" }),
    ).toBe("Slack · 工作短讯");
    expect(
      pillCaption("processing", undefined, undefined, { contextLabel: "Chrome Canary · General" }),
    ).toBe("Chrome Canary · General");
  });

  it("keeps the recording-limit caption and still surfaces the context label", () => {
    const caption = pillCaption("recording_limited", undefined, undefined, {
      contextLabel: "Chrome Canary · General",
    });
    expect(caption).toContain("Chrome Canary · General");
    expect(caption).toContain("已达上限 · 按热键结束");
  });

  it("prefers error and degraded captions over the context label", () => {
    expect(
      pillCaption("error", undefined, undefined, { contextLabel: "WeChat · 口语" }),
    ).toBe("语音输入失败，请重试");
    expect(
      pillCaption("degraded", undefined, undefined, { contextLabel: "Slack · 工作短讯" }),
    ).toBe("部分结果已保存，请检查后再使用");
  });

  it("keeps prefetch transcripts off the HUD caption", () => {
    expect(
      pillCaption("recording", undefined, undefined, {
        contextLabel: "WeChat · 口语",
        partialText: "你好世界",
      }),
    ).toBe("WeChat · 口语");
    expect(
      pillCaption("recording", undefined, undefined, { partialText: "hello there" }),
    ).toBeNull();
  });

  it("allows a wider caption than 164px when in-progress words are present", () => {
    expect(voicePillCaptionMaxWidthForPartial(true)).toBeGreaterThanOrEqual(360);
    expect(voicePillCaptionMaxWidthForPartial(true)).toBeGreaterThan(164);
    expect(voicePillCaptionMaxWidthForPartial(false)).toBe(164);
  });

  it("widens the island caption for paste failures and other long status copy", () => {
    expect(voicePillCaptionNeedsWide("copied", { fallbackReason: "paste_failed" })).toBe(true);
    expect(voicePillCaptionNeedsWide("degraded", { fallbackReason: "target_changed" })).toBe(true);
    expect(voicePillCaptionNeedsWide("error")).toBe(true);
    expect(voicePillCaptionNeedsWide("copied")).toBe(true);
    expect(voicePillCaptionNeedsWide("recording")).toBe(false);
    expect(voicePillCaptionNeedsWide("recording", { partialText: "你好" })).toBe(false);
  });

  it("uses a wider island window while in-progress words are present", () => {
    expect(voicePillWindowWidthForPartial(false)).toBe(voicePillWindowWidth);
    expect(voicePillWindowWidthForPartial(false)).toBe(172);
    expect(voicePillWindowWidthForPartial(true)).toBe(400);
  });

  it("reserves a downward caption band so the pill does not shift upward", () => {
    expect(voicePillStagePaddingTop).toBe(6);
    expect(
      voicePillStagePaddingTop + voicePillHeight + voicePillCaptionGap + voicePillCaptionHeight,
    ).toBe(voicePillWindowHeight);
  });

  it("keeps processing on the context label instead of prefetch or chunk counts", () => {
    expect(
      pillCaption("processing", undefined, undefined, {
        contextLabel: "Slack · 工作短讯",
        partialText: "in progress words",
        chunkProgress: { completed: 3, total: 8 },
      }),
    ).toBe("Slack · 工作短讯");
  });

  it("prefers error and degraded captions over in-progress words", () => {
    expect(
      pillCaption("error", undefined, undefined, {
        contextLabel: "WeChat · 口语",
        partialText: "should not appear",
      }),
    ).toBe("语音输入失败，请重试");
    expect(
      pillCaption("degraded", undefined, undefined, { partialText: "should not appear" }),
    ).toBe("部分结果已保存，请检查后再使用");
  });

  it("surfaces preparing rewrite copy during processing", () => {
    expect(
      pillCaption("processing", undefined, undefined, { selectedActionState: "preparing_rewrite" }),
    ).toBe("正在准备改写");
  });

  it("surfaces cascade accurate status from the processing phase", () => {
    expect(
      pillCaption("processing", undefined, undefined, { phase: "cascade_accurate" }),
    ).toBe("精确重打中");
    expect(
      pillCaption("processing", undefined, undefined, {
        phase: "cascade_accurate",
        contextLabel: "微信 · 重",
      }),
    ).toBe("微信 · 重 · 精确重打中");
    expect(
      pillCaption("processing", undefined, undefined, { phase: "asr" }),
    ).toBeNull();
  });

  it("does not surface chunk counts or truncation on the HUD", () => {
    expect(
      pillCaption("processing", undefined, undefined, {
        chunkProgress: { completed: 3, total: 8 },
      }),
    ).toBeNull();
    expect(
      pillCaption("processing", undefined, undefined, {
        chunkProgress: { completed: 9, total: 8 },
      }),
    ).toBeNull();
    expect(
      pillCaption("recording", undefined, undefined, { partialText: "hello…".repeat(40) }),
    ).toBeNull();
  });

  it("surfaces delivery fallback reasons in the caption chip", () => {
    expect(
      pillCaption("done", undefined, undefined, { fallbackReason: "target_changed" }),
    ).toBe("输入目标已变化，文字已复制到剪贴板，请手动粘贴");
  });

  it("surfaces terminal selected-action captions on copied state", () => {
    expect(
      pillCaption("copied", undefined, undefined, { selectedActionState: "copied_instead" }),
    ).toBe("目标变化，结果已复制");
    expect(
      pillCaption("copied", undefined, undefined, {
        fallbackReason: "selected_action_clipboard_fallback",
      }),
    ).toBe("目标变化，结果已复制");
    expect(pillCaption("copied")).toBe("已复制到剪贴板，请手动粘贴");
  });

  it("surfaces replaced caption on done state", () => {
    expect(
      pillCaption("done", undefined, undefined, { selectedActionState: "replaced" }),
    ).toBe("已替换选中文本");
  });

  it("prefers actionable fallback copy over generic degraded caption", () => {
    expect(
      pillCaption("degraded", undefined, undefined, { fallbackReason: "target_changed" }),
    ).toBe("输入目标已变化，文字已复制到剪贴板，请手动粘贴");
    expect(pillCaption("degraded")).toBe("部分结果已保存，请检查后再使用");
    expect(
      pillCaption("degraded", undefined, undefined, { fallbackReason: "some_unknown_reason" }),
    ).toBe("处理失败，请检查结果或重试");
  });

  it("maps a louder microphone level to a taller waveform bar", () => {
    expect(waveformScaleForLevel(0.8, 1)).toBeGreaterThan(waveformScaleForLevel(0.05, 1));
  });

  it("keeps normal speech in a visible dynamic range", () => {
    expect(waveformScaleForLevel(0.2, 1)).toBeGreaterThan(0.45);
    expect(waveformScaleForLevel(0, 1)).toBeGreaterThan(0.1);
  });
});
