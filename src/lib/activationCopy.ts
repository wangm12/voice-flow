export type ActivationMode = "tap" | "hold_to_talk";
type Translate = (source: string) => string;
const identity: Translate = (source) => source;

export const ACTIVATION_MODES: { id: ActivationMode; label: string }[] = [
  { id: "tap", label: "点按切换" },
  { id: "hold_to_talk", label: "按住说话" },
];

export function recordingInstruction(mode: string, hotkey: string, t: Translate = identity): string {
  return t(mode === "hold_to_talk"
    ? "按住 {hotkey} 说话，松开后结束并转成文字。"
    : "按 {hotkey} 开始，再按一次结束并转成文字。"
  ).replace("{hotkey}", hotkey);
}

export function tryItHint(mode: string, hotkey: string, t: Translate = identity): string {
  return `${recordingInstruction(mode, hotkey, t)} ${t("Esc 取消。")}`;
}

export function finishHints(mode: string, hotkey: string, t: Translate = identity): string[] {
  return [
    t(mode === "hold_to_talk" ? "按住 {hotkey} 说话" : "按 {hotkey} 开始录音").replace("{hotkey}", hotkey),
    t(mode === "hold_to_talk" ? "松开后结束并转成文字" : "再按一次结束并转换成文字"),
    t("按 Esc 取消"),
  ];
}
