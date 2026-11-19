import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isFnOnlyHotkey } from "../lib/hotkeyFormat";
import { useI18n } from "../lib/i18n";
import { ghostButtonClass } from "../lib/theme";

export function FnHotkeyNote({ hotkey }: { hotkey: string }) {
  const { t } = useI18n();
  const [inputMonitoringError, setInputMonitoringError] = useState<string | null>(null);
  const [keyboardError, setKeyboardError] = useState<string | null>(null);
  if (!isFnOnlyHotkey(hotkey)) return null;
  return <div role="note" className="mt-2 text-xs leading-5 text-secondary">
    <p>{t("Fn / 🌐 可能用于切换输入法、表情或系统听写。若有冲突，请在 macOS 键盘设置中调整；VoiceFlow 不会修改系统设置。")}</p>
    <p className="mt-2">{t("Fn 全局快捷键需要输入监控权限。VoiceFlow 只观察按键和修饰键状态，不读取输入内容。")}</p>
    <button type="button" className={`mt-1 ${ghostButtonClass}`} onClick={() => {
      void invoke("open_privacy_settings", { pane: "input_monitoring" }).catch(() => setInputMonitoringError(t("无法打开输入监控设置，请从系统设置的隐私与安全性中打开。")));
    }}>{t("打开输入监控设置")}</button>
    <button type="button" className={`mt-1 ${ghostButtonClass}`} onClick={() => {
      void invoke("open_privacy_settings", { pane: "keyboard" }).catch(() => setKeyboardError(t("无法打开键盘设置，请从系统设置中打开。")));
    }}>{t("打开键盘设置")}</button>
    {inputMonitoringError && <p role="alert" className="text-error-ink">{inputMonitoringError}</p>}
    {keyboardError && <p role="alert" className="text-error-ink">{keyboardError}</p>}
  </div>;
}
