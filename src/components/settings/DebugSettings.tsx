import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { WhatsNewDialog, type WhatsNewPayload } from "../WhatsNewDialog";
import { ClampedNumberInput } from "./ClampedNumberInput";
import { useI18n } from "../../lib/i18n";
import { secondaryButtonClass } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

export function DebugSettings({
  settings,
  save,
}: {
  settings: Settings;
  save: SaveSettings;
}) {
  const { t } = useI18n();
  const [whatsNewPreview, setWhatsNewPreview] = useState<WhatsNewPayload | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("调试设置")} description={t("录音缓冲、语音检测和诊断预览。")} saveHint={t("更改自动保存")} />
      <SettingsGroup title={t("录音")}>
        <SettingsRow title={t("句尾缓冲")} description={t("说完后多录一会儿，避免吞掉句尾。")}>
          <ClampedNumberInput
            label={t("句尾缓冲")}
            value={settings.extra_recording_buffer_ms ?? 250}
            min={0}
            max={2000}
            widthClassName="w-28"
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
            disabled={previewBusy}
            aria-busy={previewBusy}
            onClick={() => {
              setPreviewBusy(true); setPreviewError(null);
              void invoke<{ version: string; notes: string | null }>("preview_whats_new")
                .then((status) => setWhatsNewPreview({ version: status.version, notes: status.notes }))
                .catch(() => setPreviewError(t("无法读取更新说明，请重试。")))
                .finally(() => setPreviewBusy(false));
            }}
          >
            {t(previewBusy ? "正在读取…" : "预览更新说明")}
          </button>
          {previewError && <p role="alert" className="w-full text-xs text-error-ink">{previewError}</p>}
        </SettingsRow>
      </SettingsGroup>
      {whatsNewPreview && (
        <WhatsNewDialog payload={whatsNewPreview} onDismiss={() => setWhatsNewPreview(null)} />
      )}
    </SettingsShell>
  );
}
