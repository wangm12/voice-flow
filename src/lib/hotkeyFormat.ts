import { formatForDisplay } from "@tanstack/react-hotkeys";

export const DEFAULT_DICTATION_HOTKEY = "CmdOrControl+Alt+Space";
export const DEFAULT_SELECTED_ACTION_HOTKEY = "CmdOrControl+Alt+Slash";

const TAURI_PRIMARY = new Set(["Mod", "CmdOrControl", "CommandOrControl", "CmdOrCtrl", "Command", "Meta", "Super"]);
const MODIFIER_ONLY = new Set([
  "CmdOrControl",
  "CommandOrControl",
  "CmdOrCtrl",
  "Super",
  "Meta",
  "Command",
  "Alt",
  "Option",
  "Control",
  "Ctrl",
  "Shift",
  "Fn",
  "Function",
  "Globe",
]);

const MODIFIER_ORDER = ["CmdOrControl", "Control", "Alt", "Shift"] as const;

const DOM_MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "OS"]);

export function isModifierOnlyHotkey(hotkey: string): boolean {
  const trimmed = hotkey.trim();
  return !trimmed.includes("+") && MODIFIER_ONLY.has(trimmed);
}

const FN_KEYS = new Set(["Fn", "Function", "Globe"]);

export function isFnOnlyHotkey(hotkey: string): boolean {
  const trimmed = hotkey.trim();
  return isModifierOnlyHotkey(trimmed) && FN_KEYS.has(trimmed);
}

export function toTauriHotkey(tanstack: string): string {
  const parts = tanstack.split("+").map((part) => part.trim()).filter(Boolean);
  if (parts.length === 0) return tanstack;

  if (parts.length === 1) {
    const only = parts[0]!;
    if (only === "Mod" || only === "Meta") return "CmdOrControl";
    if (only === "Alt" || only === "Option") return "Alt";
    if (only === "Control" || only === "Ctrl") return "Control";
    if (only === "Shift") return "Shift";
    if (only === "Fn" || only === "Function" || only === "Globe") return "Fn";
  }

  const key = parts[parts.length - 1]!;
  const tauriKey = key === "/" ? "Slash" : key === "\\" ? "Backslash" : key;
  const modifiers = new Set<string>();

  for (const part of parts.slice(0, -1)) {
    if (TAURI_PRIMARY.has(part)) {
      modifiers.add("CmdOrControl");
    } else if (part === "Control" || part === "Ctrl") {
      modifiers.add("Control");
    } else if (part === "Alt" || part === "Option") {
      modifiers.add("Alt");
    } else if (part === "Shift") {
      modifiers.add("Shift");
    }
  }

  const ordered = MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier));
  return [...ordered, tauriKey].join("+");
}

export function toTanStackHotkey(tauri: string): string {
  const parts = tauri.split("+").map((part) => part.trim()).filter(Boolean);
  if (parts.length === 0) return tauri;

  if (isModifierOnlyHotkey(tauri)) {
    const only = parts[0]!;
    if (TAURI_PRIMARY.has(only)) return "Mod";
    return only;
  }

  const key = parts[parts.length - 1]!;
  const displayKey = key === "Slash" ? "/" : key === "Backslash" ? "\\" : key;
  const modifiers: string[] = [];

  for (const part of parts.slice(0, -1)) {
    if (TAURI_PRIMARY.has(part)) {
      if (!modifiers.includes("Mod")) modifiers.push("Mod");
    } else if (part === "Control" || part === "Ctrl") {
      modifiers.push("Control");
    } else if (part === "Alt" || part === "Option") {
      modifiers.push("Alt");
    } else if (part === "Shift") {
      modifiers.push("Shift");
    }
  }

  return [...modifiers, displayKey].join("+");
}

function formatModifierOnlyDisplay(hotkey: string): string {
  const only = hotkey.split("+")[0]?.trim() ?? hotkey;
  if (TAURI_PRIMARY.has(only)) return "⌘";
  if (only === "Alt" || only === "Option") return "⌥";
  if (only === "Shift") return "⇧";
  if (only === "Control" || only === "Ctrl") return "⌃";
  if (only === "Fn" || only === "Function" || only === "Globe") return "fn";
  return only;
}

export function formatHotkeyDisplay(tauriHotkey: string): string {
  const normalized = sanitizeTauriHotkey(tauriHotkey);
  if (isModifierOnlyHotkey(normalized)) {
    return formatModifierOnlyDisplay(normalized);
  }
  return formatForDisplay(toTanStackHotkey(normalized)).replace("␣", "Space");
}

export function hotkeyDisplayParts(tauriHotkey: string): string[] {
  const normalized = sanitizeTauriHotkey(tauriHotkey);
  const display = formatHotkeyDisplay(normalized);
  if (isModifierOnlyHotkey(normalized)) return [display];
  return display
    .split(/\s*\+\s*|\s+/)
    .map((part) => part.trim())
    .filter(Boolean);
}

function codeToKeyName(code: string): string | null {
  if (["Minus", "Equal", "BracketLeft", "BracketRight", "Semicolon", "Quote", "Backquote", "Comma", "Period", "Home", "End", "PageUp", "PageDown", "Insert", "NumpadAdd", "NumpadSubtract", "NumpadMultiply", "NumpadDivide", "NumpadDecimal"].includes(code)) return code;
  if (/^Numpad[0-9]$/.test(code)) return code;
  if (code === "Space") return "Space";
  if (code === "Slash") return "Slash";
  if (code === "Backslash") return "Backslash";
  if (code === "Enter" || code === "NumpadEnter") return "Enter";
  if (code === "Tab") return "Tab";
  if (code === "Backspace") return "Backspace";
  if (code === "Escape") return "Escape";
  if (code === "Delete") return "Delete";
  if (/^Key([A-Z])$/.test(code)) return code.slice(3);
  if (/^Digit([0-9])$/.test(code)) return code.slice(5);
  if (/^F\d+$/.test(code)) return code;
  if (code.startsWith("Arrow")) return code;
  return null;
}

function normalizeMainKey(event: KeyboardEvent): string {
  // Register the physical key, not Option-generated characters or shifted punctuation.
  const physical = codeToKeyName(event.code);
  if (physical) return physical;
  if (event.key === " ") return "Space";
  if (event.key.length === 1) return event.key.toUpperCase();
  return event.key;
}

export function sanitizeTauriHotkey(hotkey: string): string {
  const parts = hotkey.split("+").map((part) => part.trim()).filter(Boolean);
  return parts
    .map((part, index) => {
      const trimmed = part.trim();
      if (trimmed === "Dead") return "Space";
      if (index === parts.length - 1 && (trimmed === "/" || trimmed === "÷")) return "Slash";
      if (index === parts.length - 1 && trimmed === "\\") return "Backslash";
      return trimmed;
    })
    .join("+");
}

export function hotkeyFromKeyboardEvent(event: KeyboardEvent): string | null {
  if (event.key === "Escape") return null;

  const pressedModifiers = new Set<string>();
  if (event.metaKey) pressedModifiers.add("CmdOrControl");
  if (event.ctrlKey) pressedModifiers.add("Control");
  if (event.altKey) pressedModifiers.add("Alt");
  if (event.shiftKey) pressedModifiers.add("Shift");

  if (DOM_MODIFIER_KEYS.has(event.key)) {
    return null;
  }

  const ordered = MODIFIER_ORDER.filter((modifier) => pressedModifiers.has(modifier));
  return [...ordered, normalizeMainKey(event)].join("+");
}

/** New global bindings must not take over ordinary typing or UI navigation. */
export function isSafeCapturedHotkey(hotkey: string): boolean {
  const parts = sanitizeTauriHotkey(hotkey).split("+");
  const key = parts[parts.length - 1] ?? "";
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(key)) return true;
  return parts.slice(0, -1).some((part) => TAURI_PRIMARY.has(part) || ["Control", "Ctrl", "Alt", "Option"].includes(part));
}

export function isDomModifierKey(key: string): boolean {
  return DOM_MODIFIER_KEYS.has(key);
}

export function previewFromKeyboardEvent(event: KeyboardEvent): string | null {
  const parts: string[] = [];
  if (event.metaKey) parts.push("CmdOrControl");
  if (event.ctrlKey) parts.push("Control");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");

  if (isDomModifierKey(event.key)) {
    return parts.length > 0 ? parts.join("+") : null;
  }

  return hotkeyFromKeyboardEvent(event);
}

export function isModifierKeyCode(code: string): boolean {
  return /^(Shift|Control|Alt|Meta|OS|Fn)(Left|Right)?$/.test(code);
}
