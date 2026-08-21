import { describe, expect, it } from "vitest";
import {
  formatHotkeyDisplay,
  hotkeyDisplayParts,
  isFnOnlyHotkey,
  isModifierOnlyHotkey,
  toTauriHotkey,
  toTanStackHotkey,
} from "./hotkeyFormat";

describe("hotkey formatting", () => {
  it("round-trips common modifier combinations", () => {
    expect(toTauriHotkey("Mod+Shift+Space")).toBe("CmdOrControl+Shift+Space");
    expect(toTanStackHotkey("CmdOrControl+Shift+Space")).toBe("Mod+Shift+Space");
    const display = formatHotkeyDisplay("CmdOrControl+Shift+Space");
    expect(display).toMatch(/⌘|Ctrl/);
    expect(display).toMatch(/⇧|Shift/);
    expect(display).toContain("Space");
  });

  it("keeps modifier-only bindings distinct", () => {
    expect(isModifierOnlyHotkey("Fn")).toBe(true);
    expect(formatHotkeyDisplay("Fn")).toBe("fn");
    expect(isModifierOnlyHotkey("CmdOrControl+Shift")).toBe(false);
  });

  it("treats Fn, Function, and Globe as the Fn hint hotkey", () => {
    expect(isFnOnlyHotkey("Fn")).toBe(true);
    expect(isFnOnlyHotkey("Function")).toBe(true);
    expect(isFnOnlyHotkey("Globe")).toBe(true);
    expect(isFnOnlyHotkey("Command")).toBe(false);
    expect(isFnOnlyHotkey("CmdOrControl+Shift+Space")).toBe(false);
  });

  it("keeps the slash shortcut canonical internally but symbolic in the UI", () => {
    expect(toTauriHotkey("Mod+Shift+/"))
      .toBe("CmdOrControl+Shift+Slash");
    expect(toTanStackHotkey("CmdOrControl+Shift+Slash")).toBe("Mod+Shift+/");
    expect(formatHotkeyDisplay("CmdOrControl+Shift+Slash")).toContain("/");
    expect(formatHotkeyDisplay("CmdOrControl+Shift+Slash")).not.toContain("Slash");
  });

  it("renders every registered key as its own display part, including Space", () => {
    const parts = hotkeyDisplayParts("CmdOrControl+Shift+Space");
    expect(parts).toHaveLength(3);
    expect(parts[parts.length - 1]).toBe("Space");
  });
});
