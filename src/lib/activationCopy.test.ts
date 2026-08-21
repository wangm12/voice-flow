import { describe, expect, it } from "vitest";
import { ACTIVATION_MODES, finishHints, resolveCapturedActivationMode, tryItHint } from "./activationCopy";

describe("activationCopy", () => {
  it("offers hybrid as a combo-key activation mode", () => {
    expect(ACTIVATION_MODES.map((mode) => mode.id)).toEqual(["tap", "double_tap", "hybrid"]);
  });

  it("explains hybrid short press and hold", () => {
    const hint = tryItHint("hybrid", "⌘⇧Space");
    expect(hint).toContain("⌘⇧Space");
    expect(hint).toMatch(/短按/);
    expect(hint).toMatch(/按住/);
  });

  it("keeps tap and double-tap hints unchanged", () => {
    expect(tryItHint("tap", "⌘⇧Space")).toContain("按 ⌘⇧Space 开始录音");
    expect(tryItHint("double_tap", "fn")).toMatch(/双击功能键/);
  });

  it("keeps hybrid when combo recapture reports tap", () => {
    expect(resolveCapturedActivationMode("hybrid", "Command+Shift+Space", "tap")).toBe("hybrid");
    expect(resolveCapturedActivationMode("tap", "Command+Shift+Space", "tap")).toBe("tap");
    expect(resolveCapturedActivationMode("hybrid", "Fn", "double_tap")).toBe("double_tap");
  });

  it("includes hold-to-talk in hybrid finish hints", () => {
    const hints = finishHints("hybrid", "⌘⇧Space");
    expect(hints.join(" ")).toMatch(/按住/);
    expect(hints.join(" ")).toContain("⌘⇧Space");
  });
});
