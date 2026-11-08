import { describe, expect, it } from "vitest";
import { hudPresentation } from "./hudPresentation";

describe("HUD presentation from real app outcomes", () => {
  it("shows the raw-ASR notice for critical-memory cleanup fallback", () => {
    expect(hudPresentation({ state: "degraded", fallbackReason: "critical_memory_pressure" })).toMatchObject({
      title: "已保留 ASR 原稿",
      subtitle: "系统内存压力过高，未运行本地整理",
      action: { destination: "history", label: "检查历史记录" },
    });
  });

  it.each([
    ["microphone_required", "需要麦克风权限", "microphone"],
    ["closed_lid_builtin_input_unavailable", "内置麦克风不可用", "microphone"],
    ["api_key_required", "语音服务暂不可用", "service"],
    ["local_native_model_missing", "本地模型暂不可用", "models"],
    ["processing_timeout", "处理超时", "history"],
    ["local_processing_timeout", "本地处理超时", "history"],
    ["onboarding_required", "还需要完成设置", "service"],
  ])("maps %s to a truthful title and a supported destination", (reason, title, destination) => {
    const result = hudPresentation({ state: "error", fallbackReason: reason });
    expect(result.title).toBe(title);
    expect(result.action?.destination).toBe(destination);
  });

  it("explains the closed-lid microphone recovery action", () => {
    const result = hudPresentation({ state: "error", fallbackReason: "closed_lid_builtin_input_unavailable" });
    expect(result.subtitle).toBe("打开显示屏或选择外接麦克风");
    expect(result.action?.label).toBe("检查麦克风");
  });

  it("maps the actual first-frame wait source without claiming audio arrived", () => {
    const result = hudPresentation({ state: "recording", captureStatus: "waiting_first_frame" });
    expect(result.title).toBe("等待麦克风");
    expect(result.subtitle).toBe("尚未收到音频");
  });

  it("does not let an old microphone status label active processing", () => {
    const result = hudPresentation({ state: "processing", captureStatus: "no_recent_audio" });
    expect(result.title).toBeNull();
    expect(result.action).toBeNull();
  });

  it("does not infer that an unverified delivery was saved or copied", () => {
    const result = hudPresentation({ state: "unverified", fallbackReason: "paste_unverified" });
    expect(result.title).toBe("已尝试输入");
    expect(result.subtitle).toBe("先检查输入框");
    expect(result.title).not.toMatch(/保存|复制/);
    expect(result.action).toEqual({ destination: "history", label: "检查历史记录" });
  });

  it("keeps an uncertain paste attempt to one short input-check hint", () => {
    const result = hudPresentation({
      state: "unverified",
      deliveryMethod: "paste_unverified",
      fallbackReason: "paste_mutation_uncertain",
    });
    expect(result).toMatchObject({
      title: "已尝试输入",
      subtitle: "先检查输入框",
      action: null,
    });
  });

  it.each(["preparing_rewrite", "preview_ready"])("does not direct unverified %s text actions to History", (selectedActionState) => {
    const result = hudPresentation({
      state: "unverified",
      deliveryMethod: "paste_unverified",
      fallbackReason: "paste_unverified",
      selectedActionState,
    });
    expect(result.title).toBe("已尝试输入");
    expect(result.subtitle).toBe("先检查输入框");
    expect(result.action).toBeNull();
  });

  it("keeps the unverified-delivery warning while exposing the critical-cleanup notice", () => {
    const result = hudPresentation({
      state: "unverified",
      fallbackReason: "paste_unverified",
      deliveryMethod: "paste_unverified",
      cleanupStatus: "critical_memory_pressure",
    });
    expect(result.title).toBe("已尝试输入");
    expect(result.subtitle).toBe("先检查输入框");
    expect(result.detail).toContain("系统内存压力过高");
  });

  it("keeps the unverified-delivery warning while exposing the neutral memory-budget notice", () => {
    const result = hudPresentation({
      state: "unverified",
      fallbackReason: "paste_unverified",
      deliveryMethod: "paste_unverified",
      cleanupStatus: "local_memory_unavailable",
    });
    expect(result.title).toBe("已尝试输入");
    expect(result.subtitle).toBe("先检查输入框");
    expect(result.detail).toBe("本机内存预算不足，未运行本地整理；已保留 ASR 原稿");
    expect(result.detail).not.toContain("压力过高");
  });

  it("distinguishes a known clipboard fallback from a result retained in History", () => {
    const copied = hudPresentation({
      state: "copied",
      deliveryMethod: "clipboard",
      fallbackReason: "target_changed",
    });
    const history = hudPresentation({
      state: "history",
      deliveryMethod: "history",
      fallbackReason: "target_changed",
    });

    expect(copied.title).toBe("输入目标已变化");
    expect(copied.subtitle).toBe("文字已复制");
    expect(copied.icon).toBe("clipboard");
    expect(history.title).toBe("输入目标已变化");
    expect(history.subtitle).toBe("结果已存历史");
    expect(history.icon).toBe("archive");
  });

  it("keeps the actual delivery method in a degraded result", () => {
    expect(hudPresentation({ state: "degraded", deliveryMethod: "clipboard" })).toMatchObject({
      title: "部分结果已复制",
      icon: "clipboard",
    });
    expect(hudPresentation({ state: "degraded", deliveryMethod: "history" })).toMatchObject({
      title: "部分结果已保存到历史",
      icon: "archive",
    });
  });
});
