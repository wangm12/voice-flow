import { describe, expect, it } from "vitest";
import {
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

describe("Beam v10 HUD tokens", () => {
  it("keeps the primary capsule, host, and secondary tray at their reviewed geometry", () => {
    expect([voicePillWidth, voicePillHeight, HUD_PILL_RADIUS_PX]).toEqual([220, 48, 24]);
    expect([voicePillWindowWidth, voicePillWindowHeight]).toEqual([244, 64]);
    expect([voicePillRecoveryWidth, voicePillRecoveryHeight]).toEqual([320, 116]);
    expect(voicePillLearnToastHeight).toBe(48);
  });

  it("keeps selected-action guidance deterministic", () => {
    expect(selectedActionCaption("waiting_for_selection")).toBe("等待选中文本");
    expect(selectedActionCaption("unknown-state")).toBeNull();
  });
});
