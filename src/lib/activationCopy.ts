export type ActivationMode = "tap" | "double_tap";
type Translate = (source: string) => string;

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
];

export function tryItHint(activationMode: string, hotkeyDisplay: string, translate: Translate = identity): string {
  switch (activationMode) {
    case "double_tap":
      return translate("双击功能键（如 ⌘ ⌃ ⌥ ⇧ fn）开始录音，再双击结束；按 Esc 可取消。");
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
    default:
      return [
        translate("按 {hotkey} 开始录音").replace("{hotkey}", hotkeyDisplay),
        translate("再按一次结束并转换成文字"),
        translate("按 Esc 取消"),
      ];
  }
}
