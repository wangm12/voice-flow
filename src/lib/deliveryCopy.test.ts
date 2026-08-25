import { describe, expect, it } from "vitest";
import { deliveryReasonMessage } from "./deliveryCopy";

describe("deliveryCopy", () => {
  it("maps known fallback reasons to friendly copy", () => {
    expect(deliveryReasonMessage("target_changed", (source) => source)).toBe(
      "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
    );
  });

  it("tells the user to press Cmd+V after an unverified insert", () => {
    expect(deliveryReasonMessage("paste_unverified", (source) => source)).toBe("已复制，请按 ⌘V");
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
});
