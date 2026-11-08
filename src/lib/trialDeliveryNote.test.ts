import { describe, expect, it } from "vitest";
import { shouldClearTrialDeliveryNote, trialDeliveryNote } from "./trialDeliveryNote";

describe("trialDeliveryNote", () => {
  const translate = (source: string) => source;

  it("uses the copied terminal state to give truthful paste guidance", () => {
    expect(trialDeliveryNote("copied", "target_unavailable", translate)).toBe(
      "已复制到剪贴板，请手动粘贴",
    );
    expect(shouldClearTrialDeliveryNote("idle")).toBe(false);
    expect(shouldClearTrialDeliveryNote("copied")).toBe(false);
  });

  it("keeps an unverified target failure cause-only", () => {
    expect(trialDeliveryNote("unverified", "target_unavailable", translate)).toBe(
      "无法确认输入目标",
    );
  });

  it("clears the trial note only when a new recording starts", () => {
    expect(shouldClearTrialDeliveryNote("recording")).toBe(true);
    expect(shouldClearTrialDeliveryNote("starting")).toBe(true);
    expect(shouldClearTrialDeliveryNote("processing")).toBe(false);
  });

  it("uses a readable default when copied has no fallback reason", () => {
    expect(trialDeliveryNote("copied", null, translate)).toBe("已复制到剪贴板，请手动粘贴");
  });
});
