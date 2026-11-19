export type ActivationMode = "tap" | "hold_to_talk" | "hybrid_hold_lock";
type Translate = (source: string) => string;
const identity: Translate = (source) => source;

export const ACTIVATION_MODES: { id: ActivationMode; label: string }[] = [
  { id: "tap", label: "点按切换" },
  { id: "hold_to_talk", label: "按住说话" },
  { id: "hybrid_hold_lock", label: "按住 / 双击锁定" },
];

export function recordingInstruction(mode: string, hotkey: string, t: Translate = identity): string {
  const source = mode === "hold_to_talk"
    ? "按住 {hotkey} 说话，松开后结束并转成文字。"
    : mode === "hybrid_hold_lock"
      ? "按住 {hotkey} 至少 220ms 后松开结束；短按两次（320ms 内）锁定免提，再按一次结束。"
      : "按 {hotkey} 开始，再按一次结束并转成文字。";
  return t(source).replace(/\{hotkey\}/g, hotkey);
}

export function tryItHint(mode: string, hotkey: string, t: Translate = identity): string {
  return `${recordingInstruction(mode, hotkey, t)} ${t("Esc 取消。")}`;
}

export function finishHints(mode: string, hotkey: string, t: Translate = identity): string[] {
  if (mode === "hybrid_hold_lock") {
    return [
      t("按住 {hotkey} 至少 220ms 后松开结束").replace(/\{hotkey\}/g, hotkey),
      t("短按两次（320ms 内）锁定免提录音"),
      t("再次按 {hotkey} 结束").replace(/\{hotkey\}/g, hotkey),
      t("按 Esc 取消"),
    ];
  }
  return [
    t(mode === "hold_to_talk" ? "按住 {hotkey} 说话" : "按 {hotkey} 开始录音").replace("{hotkey}", hotkey),
    t(mode === "hold_to_talk" ? "松开后结束并转成文字" : "再按一次结束并转换成文字"),
    t("按 Esc 取消"),
  ];
}
