import { describe, expect, it } from "vitest";
import {
  closedLidMicrophoneErrorMessage,
  deliveryReasonHudMessage,
  deliveryReasonMessage,
} from "./deliveryCopy";

describe("deliveryCopy", () => {
  it("explains critical-memory fallback without implying cleanup ran", () => {
    expect(deliveryReasonMessage("critical_memory_pressure", (source) => source)).toBe(
      "系统内存压力过高，未运行本地整理；已保留 ASR 原稿",
    );
    expect(deliveryReasonHudMessage("critical_memory_pressure", (source) => source)).toBe(
      "内存压力过高；已保留 ASR 原稿",
    );
  });

  it("explains a denied local-memory budget without labeling it critical pressure", () => {
    expect(deliveryReasonMessage("local_memory_unavailable", (source) => source)).toBe(
      "本机内存预算不足，未运行本地整理；已保留 ASR 原稿",
    );
    expect(deliveryReasonHudMessage("local_memory_unavailable", (source) => source)).toBe(
      "内存预算不足；已保留 ASR 原稿",
    );
  });

  it("maps known fallback reasons to friendly copy", () => {
    expect(deliveryReasonMessage("target_changed", (source) => source)).toBe(
      "目标已变化",
    );
  });

  it("explains microphone and no-speech failures with an actionable next step", () => {
    expect(deliveryReasonMessage("no_speech", (source) => source)).toBe(
      "未检测到声音，请检查麦克风后重试",
    );
    expect(deliveryReasonHudMessage("no_speech", (source) => source)).toBe(
      "未检测到声音；检查麦克风",
    );
    expect(deliveryReasonMessage("audio_error", (source) => source)).toBe(
      "麦克风输入中断，请检查所选设备后重试",
    );
    expect(deliveryReasonMessage("closed_lid_builtin_input_unavailable", (source) => source)).toBe(
      "合上显示屏时内置麦克风不可用，请打开显示屏或选择外接麦克风。",
    );
    expect(deliveryReasonHudMessage("closed_lid_builtin_input_unavailable", (source) => source)).toBe(
      "打开显示屏或选择外接麦克风",
    );
  });

  it("localizes only the recognized closed-lid built-in microphone error", () => {
    expect(closedLidMicrophoneErrorMessage(
      "microphone device error: selected input device is unavailable while the lid is closed; open the display or choose an external microphone",
      (source) => source,
    )).toBe("合上显示屏时内置麦克风不可用，请打开显示屏或选择外接麦克风。");
    expect(closedLidMicrophoneErrorMessage("input device unavailable", (source) => source)).toBeNull();
  });

  it("asks the user to inspect History before retrying an unverified insert", () => {
    expect(deliveryReasonMessage("paste_unverified", (source) => source)).toBe(
      "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
    );
  });

  it("does not report a History save when the History write failed", () => {
    const full = deliveryReasonMessage("history_save_failed", (source) => source);
    const hud = deliveryReasonHudMessage("history_save_failed", (source) => source);
    expect(full).toBe("无法保存到历史记录，请检查存储空间后重试");
    expect(hud).toBe("历史记录暂不可用");
    expect(full).not.toContain("已保存");
    expect(hud).not.toContain("已保存");
  });

  it.each([
    ["paste_shortcut_outcome_unknown", "已尝试触发粘贴，但系统未返回可确认结果；请检查输入框和历史记录，未重复粘贴"],
    ["paste_expected_splice_unavailable", "已尝试粘贴，但无法构造输入框核对结果；请检查输入框和历史记录，未重复粘贴"],
    ["paste_target_unconfirmed_after_attempt", "已尝试粘贴，但无法确认目标应用在回读时的状态；请检查输入框和历史记录，未重复粘贴"],
    ["paste_focus_unconfirmed_after_attempt", "已尝试粘贴，但无法确认输入框焦点在回读时的状态；请检查输入框和历史记录，未重复粘贴"],
    ["paste_target_changed_after_attempt", "已尝试粘贴，但无法确认目标应用在回读时的状态；请检查输入框和历史记录，未重复粘贴"],
    ["paste_focus_changed_after_attempt", "已尝试粘贴，但无法确认输入框焦点在回读时的状态；请检查输入框和历史记录，未重复粘贴"],
    ["paste_readback_unavailable", "已尝试粘贴，但无法读取输入框结果；请检查输入框和历史记录，未重复粘贴"],
    ["paste_readback_mismatch", "已尝试粘贴，但本次输入框读回结果与预期不同；请检查输入框和历史记录，未重复粘贴"],
  ])("describes typed paste fact %s without claiming failure or success", (code, message) => {
    expect(deliveryReasonMessage(code, (source) => source)).toBe(message);
    expect(deliveryReasonHudMessage(code, (source) => source)).toBe("先检查输入框");
  });

  it("describes History retry copy outcomes without claiming insertion", () => {
    expect(deliveryReasonMessage("retry_clipboard_only", (source) => source)).toBe(
      "历史重试已完成，结果已复制到剪贴板（仅复制）",
    );
    expect(deliveryReasonMessage("retry_clipboard_changed", (source) => source)).toBe(
      "剪贴板在重试期间已更新，保留了当前内容；转写结果已保存在历史记录，可手动复制",
    );
    expect(deliveryReasonMessage("retry_clipboard_ownership_unverified", (source) => source)).toBe(
      "无法确认剪贴板在重试期间是否变化；转写结果已保存在历史记录，可手动复制",
    );
    expect(deliveryReasonMessage("retry_clipboard_write_failed", (source) => source)).toBe(
      "复制状态未能确认；转写结果已保存在历史记录，请检查剪贴板或从历史记录复制",
    );
    expect(deliveryReasonHudMessage("retry_clipboard_changed", (source) => source)).toBe(
      "剪贴板有变化；先检查输入框",
    );
    expect(deliveryReasonHudMessage("retry_clipboard_changed", (source) => source)).not.toContain("已保存");
  });

  it("maps selected-action clipboard fallback to friendly copy", () => {
    expect(deliveryReasonMessage("selected_action_clipboard_fallback", (source) => source)).toBe(
      "目标变化，结果已复制",
    );
  });

  it("never exposes unknown enum values", () => {
    expect(deliveryReasonMessage("some_unknown_reason", (source) => source)).toBe(
      "处理失败，请检查结果或重试",
    );
  });

  it.each([
    ["paste_unverified", "先检查输入框"],
    ["paste_mutation_uncertain", "先检查输入框；需要时从历史记录复制"],
    ["clipboard_changed", "剪贴板有变化；先检查输入框"],
    ["clipboard_ownership_unverified", "复制状态未确认；先检查输入框"],
    ["keyboard_paste_failed", "先检查输入框；需要时从历史记录复制"],
  ])("keeps inspection before recovery for %s on the HUD", (reason, expected) => {
      expect(deliveryReasonHudMessage(reason, (source) => source)).toBe(expected);
    },
  );

  it("keeps unproven delivery reasons cause-only", () => {
    const reasonOnly = [
      "browser_permission_required",
      "secure_input",
      "input_changed",
      "target_changed",
      "target_unavailable",
      "paste_failed",
      "clipboard_unavailable",
      "clipboard_write_failed",
      "keyboard_paste_failed",
    ];
    for (const reason of reasonOnly) {
      const full = deliveryReasonMessage(reason, (source) => source) ?? "";
      const hud = deliveryReasonHudMessage(reason, (source) => source) ?? "";
      expect(full).not.toContain("手动粘贴");
      expect(full).not.toContain("⌘V");
      expect(hud).not.toContain("手动粘贴");
      expect(hud).not.toContain("⌘V");
      expect(full).not.toContain("已复制");
      expect(full).not.toContain("已保存");
      expect(hud).not.toContain("已复制");
      expect(hud).not.toContain("已保存");
    }
    expect(deliveryReasonHudMessage("target_changed", (source) => source)).toBe("目标已变化");
    const targetChanged = deliveryReasonMessage("target_changed", (source) => source);
    expect(targetChanged).toBe("目标已变化");
    expect(targetChanged).not.toContain("已复制");
    expect(targetChanged).not.toContain("已保存");
    expect(deliveryReasonHudMessage(undefined, (source) => source)).toBeNull();
    expect(deliveryReasonHudMessage("unknown", (source) => source)).toBe("处理失败，请检查结果或重试");
  });
});
