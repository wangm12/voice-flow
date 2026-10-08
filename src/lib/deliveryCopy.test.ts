import { describe, expect, it } from "vitest";
import { deliveryReasonHudMessage, deliveryReasonMessage } from "./deliveryCopy";

describe("deliveryCopy", () => {
  it("maps known fallback reasons to friendly copy", () => {
    expect(deliveryReasonMessage("target_changed", (source) => source)).toBe(
      "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
    );
  });

  it("asks the user to inspect History before retrying an unverified insert", () => {
    expect(deliveryReasonMessage("paste_unverified", (source) => source)).toBe(
      "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
    );
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

  it.each(["paste_unverified", "paste_mutation_uncertain", "clipboard_changed", "clipboard_ownership_unverified", "keyboard_paste_failed"])(
    "keeps inspection before recovery for %s on the HUD", (reason) => {
      expect(deliveryReasonHudMessage(reason, (source) => source)).toBe(
        "先检查输入框；需要时从历史记录复制",
      );
    },
  );

  it("keeps the full History diagnosis while the HUD shows the next step", () => {
    expect(deliveryReasonHudMessage("target_changed", (source) => source)).toBe("已复制，请手动粘贴");
    expect(deliveryReasonMessage("target_changed", (source) => source)).toBe(
      "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
    );
    expect(deliveryReasonHudMessage("clipboard_write_failed", (source) => source)).toBe(
      "文字已保存，请从历史记录复制",
    );
    expect(deliveryReasonHudMessage(undefined, (source) => source)).toBeNull();
    expect(deliveryReasonHudMessage("unknown", (source) => source)).toBe("处理失败，请检查结果或重试");
  });
});
