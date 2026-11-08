/* Adapted from Handy src/components/settings/MicrophoneSelector.tsx (MIT),
 * commit 14f6f0d31cb22a4268acfbddba3052820fd0a06e.
 * Copyright (c) 2025 CJ Pais. Full notice: ./HANDY-LICENSE.txt. */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Select } from "../Select";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass, inputClass } from "../../lib/theme";
import type { AudioInputDevice } from "../../types/settings";

export function MicrophoneSelector({ value, onChange }: { value: string; onChange: (value: string) => Promise<void> }) {
  const { t } = useI18n();
  const [devices, setDevices] = useState<AudioInputDevice[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(false);
  const sequence = useRef(0);
  const refresh = async () => {
    const request = ++sequence.current;
    setLoading(true);
    try {
      const next = await invoke<AudioInputDevice[]>("get_audio_input_devices");
      if (!Array.isArray(next)) throw new Error("invalid_device_list");
      if (active.current && request === sequence.current) { setDevices(next); setError(null); }
    } catch { if (active.current && request === sequence.current) setError(t("无法读取麦克风列表，请刷新或检查系统设置。")); }
    finally { if (active.current && request === sequence.current) setLoading(false); }
  };
  useEffect(() => { active.current = true; void refresh(); return () => { active.current = false; sequence.current += 1; }; }, []);
  const select = async (next: string) => {
    setSaving(true); setError(null);
    try { await onChange(next); }
    catch { if (active.current) setError(t("设备设置保存失败，原选择已保留。")); }
    finally { if (active.current) setSaving(false); }
  };
  const available = !value || devices.some((device) => device.name === value);
  return <div className="mt-5 space-y-2">
    <label htmlFor="onboarding-microphone" className="block text-sm font-medium">{t("选定麦克风")}</label>
    <div className="flex flex-wrap gap-2"><Select id="onboarding-microphone" value={value} disabled={loading || saving} onValueChange={(next) => void select(next)} className={`${inputClass} min-w-0 flex-1`}>
      <option value="">{t("默认（跟随系统）")}</option>
      {value && !available && <option value={value}>{value} · {t("设备不可用")}</option>}
      {devices.map((device) => <option key={device.name} value={device.name}>{device.name}</option>)}
    </Select><button type="button" disabled={loading || saving} className={compactButtonClass} onClick={() => void refresh()}>{t("刷新")}</button><button type="button" disabled={loading || saving || !value} className={compactButtonClass} onClick={() => void select("")}>{t("恢复默认")}</button></div>
    <p className="text-xs leading-5 text-secondary">{t("设备列表只表示可选择的设备。实际使用的麦克风与音频采集结果由下面的本机自检确认。")}</p>
    {error && <p role="alert" className="text-xs text-error-ink">{error}</p>}
  </div>;
}
