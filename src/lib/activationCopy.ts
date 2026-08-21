import { isModifierOnlyHotkey } from "./hotkeyFormat";

export type ActivationMode = "tap" | "double_tap" | "hybrid";
type Translate = (source: string) => string;

/** Combo capture always reports tap (same keys as hybrid). Keep hybrid unless the new binding is modifier-only. */
export function resolveCapturedActivationMode(
  currentMode: string,
  hotkey: string,
  capturedMode?: ActivationMode,
): ActivationMode | undefined {
  if (!capturedMode) return undefined;
  if (isModifierOnlyHotkey(hotkey)) return "double_tap";
  if (currentMode === "hybrid") return "hybrid";
  return capturedMode;
}

const identity: Translate = (source) => source;

export const ACTIVATION_MODES: { id: ActivationMode; label: string; description: string }[] = [
  {
    id: "tap",
    label: "按一下切换",
    description: "按一下开始，再按一下结束并转换成文字；按 Esc 可取消",
  },
  {
    id: "double_tap",
    label: "双击开始",
    description: "双击功能键（如 ⌘ ⌃ ⌥ ⇧ fn）开始，再双击结束并转换成文字",
  },
  {
    id: "hybrid",
    label: "短按切换，按住说话",
    description: "短按开始，再按结束；按住则松开后转换成文字；按 Esc 可取消",
  },
];

export function tryItHint(activationMode: string, hotkeyDisplay: string, translate: Translate = identity): string {
  switch (activationMode) {
    case "double_tap":
      return translate("双击功能键（如 ⌘ ⌃ ⌥ ⇧ fn）开始录音，再双击结束；按 Esc 可取消。");
    case "hybrid":
      return translate("短按 {hotkey} 开始录音，再按一次结束；按住不放则松开后结束并转换成文字；按 Esc 可取消。").replace("{hotkey}", hotkeyDisplay);
    default:
      return translate("按 {hotkey} 开始录音，再按一次结束并转换成文字；按 Esc 可取消。").replace("{hotkey}", hotkeyDisplay);
  }
}

export function finishHints(activationMode: string, hotkeyDisplay: string, translate: Translate = identity): string[] {
  switch (activationMode) {
    case "double_tap":
      return [
        translate("双击功能键（如 ⌘ ⌃ ⌥ ⇧ fn）开始录音"),
        translate("再双击结束并转换成文字"),
        translate("按 Esc 取消"),
      ];
    case "hybrid":
      return [
        translate("短按 {hotkey} 开始录音").replace("{hotkey}", hotkeyDisplay),
        translate("再按一次结束，或按住后松开结束并转换成文字"),
        translate("按 Esc 取消"),
      ];
    default:
      return [
        translate("按 {hotkey} 开始录音").replace("{hotkey}", hotkeyDisplay),
        translate("再按一次结束并转换成文字"),
        translate("按 Esc 取消"),
      ];
  }
}
