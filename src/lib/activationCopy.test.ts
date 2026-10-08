import { describe, expect, it } from "vitest";
import { ACTIVATION_MODES, finishHints, recordingInstruction, tryItHint } from "./activationCopy";
describe("activationCopy", () => {
  it("offers only tap and pure hold", () => {
    expect(ACTIVATION_MODES.map((mode) => mode.id)).toEqual(["tap", "hold_to_talk"]);
  });
  it.each(["Fn", "⌘⌥Space"])("uses the same instruction for settings and trial with %s", (hotkey) => {
    expect(tryItHint("tap", hotkey)).toBe(`${recordingInstruction("tap", hotkey)} Esc 取消。`);
    expect(recordingInstruction("tap", hotkey)).toBe(`按 ${hotkey} 开始，再按一次结束并转成文字。`);
    expect(recordingInstruction("hold_to_talk", hotkey)).toBe(`按住 ${hotkey} 说话，松开后结束并转成文字。`);
    expect(finishHints("hold_to_talk", hotkey)).toEqual([`按住 ${hotkey} 说话`, "松开后结束并转成文字", "按 Esc 取消"]);
  });
});
