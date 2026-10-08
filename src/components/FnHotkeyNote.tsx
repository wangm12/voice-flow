import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isFnOnlyHotkey } from "../lib/hotkeyFormat";
import { useI18n } from "../lib/i18n";
import { ghostButtonClass } from "../lib/theme";

export function FnHotkeyNote({ hotkey }: { hotkey: string }) {
  const { t } = useI18n();
  const [error, setError] = useState<string | null>(null);
  if (!isFnOnlyHotkey(hotkey)) return null;
  return <div role="note" className="mt-2 text-xs leading-5 text-secondary">
    <p>{t("Fn / 🌐 可能用于切换输入法、表情或系统听写。若有冲突，请在 macOS 键盘设置中调整；VoiceFlow 不会修改系统设置。")}</p>
    <button type="button" className={`mt-1 ${ghostButtonClass}`} onClick={() => {
      void invoke("open_privacy_settings", { pane: "keyboard" }).catch(() => setError(t("无法打开键盘设置，请从系统设置中打开。")));
    }}>{t("打开键盘设置")}</button>
    {error && <p role="alert" className="text-error-ink">{error}</p>}
  </div>;
}
