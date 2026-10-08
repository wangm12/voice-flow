import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { WhatsNewDialog, type WhatsNewPayload } from "../WhatsNewDialog";
import { useI18n } from "../../lib/i18n";
import { secondaryButtonClass, inputClass } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = inputClass;

export function DebugSettings({
  settings,
  save,
}: {
  settings: Settings;
  save: SaveSettings;
}) {
  const { t } = useI18n();
  const [whatsNewPreview, setWhatsNewPreview] = useState<WhatsNewPayload | null>(null);

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("调试设置")} description={t("录音缓冲、语音检测和诊断预览。")} />
      <SettingsGroup title={t("录音")}>
        <SettingsRow title={t("句尾缓冲")} description={t("说完后多录一会儿，避免吞掉句尾。")}>
          <BufferNumberField
            label={t("句尾缓冲")}
            value={settings.extra_recording_buffer_ms ?? 250}
            onCommit={(extra_recording_buffer_ms) => save({ extra_recording_buffer_ms })}
          />
          <span className="text-xs text-secondary">{t("毫秒")}</span>
        </SettingsRow>
        <SettingsRow title={t("语音检测")} description={t("过滤最终批次中的静音；实时或预取音频可能已发送，不会自动结束录音。")}>
          <Toggle
            checked={settings.vad_enabled ?? false}
            onChange={(checked) => save({ vad_enabled: checked })}
            label={t("语音检测")}
          />
        </SettingsRow>
        <SettingsRow title={t("常开麦克风")} description={t("保持麦克风预热，缩短开始录音的等待。")}>
          <Toggle
            checked={settings.always_on_microphone ?? false}
            onChange={(checked) => save({ always_on_microphone: checked })}
            label={t("常开麦克风")}
          />
        </SettingsRow>
        <SettingsRow title={t("模糊词典")} description={t("尝试匹配同一行内相近的英文词语；代码和受保护文本不受影响。")}>
          <Toggle checked={settings.fuzzy_dictionary_enabled ?? false} onChange={(checked) => save({ fuzzy_dictionary_enabled: checked })} label={t("模糊词典")} />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("诊断预览")}>
        <SettingsRow title={t("更新说明")} description={t("即使已经看过，也可以再次打开当前版本说明。")}>
          <button
            type="button"
            className={secondaryButtonClass}
            onClick={() => {
              void invoke<{ version: string; notes: string | null }>("preview_whats_new")
                .then((status) => setWhatsNewPreview({ version: status.version, notes: status.notes }))
                .catch(() => undefined);
            }}
          >
            {t("预览更新说明")}
          </button>
        </SettingsRow>
      </SettingsGroup>
      {whatsNewPreview && (
        <WhatsNewDialog payload={whatsNewPreview} onDismiss={() => setWhatsNewPreview(null)} />
      )}
    </SettingsShell>
  );
}

function BufferNumberField({
  label,
  value,
  onCommit,
}: {
  label: string;
  value: number;
  onCommit: (next: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);

  const commit = () => {
    const parsed = Number(draft);
    if (!Number.isFinite(parsed)) {
      setDraft(String(value));
      return;
    }
    const clamped = Math.min(2000, Math.max(0, Math.round(parsed)));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      aria-label={label}
      type="number"
      min={0}
      max={2000}
      step={1}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${controlClass} w-28 text-right`}
    />
  );
}
