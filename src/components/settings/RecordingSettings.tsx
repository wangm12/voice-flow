import { Select } from "../Select";
import { invoke } from "@tauri-apps/api/core";
import { FnHotkeyNote } from "../FnHotkeyNote";
import { useCallback, useEffect, useState } from "react";
import { ActivationModeSelector } from "../ActivationModeSelector";
import { HotkeyRecorder } from "../HotkeyRecorder";
import { SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, type SettingsSaveFailure } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { translationLanguageOptions } from "../../lib/translationLanguages";
import { useI18n } from "../../lib/i18n";
import { isFnOnlyHotkey } from "../../lib/hotkeyFormat";
import { asrLanguageDescription, providerOf } from "../../lib/engineWizard";
import { asrModelProfile } from "../../lib/providers";
import { inputClass } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = inputClass;
const selectClass = `${controlClass} w-40`;
const retentionOptions = [
  { value: 0, label: "立即清理" },
  { value: 1, label: "1 天" },
  { value: 7, label: "1 周" },
  { value: 30, label: "1 月" },
  { value: 365, label: "1 年" },
] as const;
const historyRetentionOptions = [
  { value: 7, label: "1 周" },
  { value: 30, label: "1 月" },
  { value: 90, label: "3 月" },
  { value: 365, label: "1 年" },
  { value: 3650, label: "10 年" },
  { value: 0, label: "永久" },
] as const;

export function RecordingSettings({ settings, save, saveFailure, dictationBusy = false }: { settings: Settings; save: SaveSettings; saveFailure?: SettingsSaveFailure | null; dictationBusy?: boolean }) {
  const { t } = useI18n();
  const [modeSaving, setModeSaving] = useState(false);
  const [modeError, setModeError] = useState<string | null>(null);
  const [captureBusy, setCaptureBusy] = useState<Record<string, boolean>>({});
  const [captureErrors, setCaptureErrors] = useState<Record<string, string | null>>({});
  const updateBusy = useCallback((target: string, busy: boolean) => {
    setCaptureBusy((previous) => previous[target] === busy ? previous : { ...previous, [target]: busy });
  }, []);
  const updateError = useCallback((target: string, error: string | null) => {
    setCaptureErrors((previous) => previous[target] === error ? previous : { ...previous, [target]: error });
  }, []);
  const otherCaptureBusy = (target: string) => Object.entries(captureBusy).some(([key, busy]) => key !== target && busy);
  const extraShortcutsBusy = Object.entries(captureBusy).some(([key, busy]) => key !== "dictation" && busy);
  const advancedSaveError = saveFailure?.fields.some((field) => ["input_gain", "chunk_threshold_secs", "chunk_length_secs"].includes(field)) ? saveFailure.message : null;
  const shortcutSaveError = saveFailure?.fields.some((field) => ["selected_action_hotkey", "selected_actions_enabled", "translation_hotkey", "translation_target_language", "screen_action_hotkey", "screen_actions_enabled", "verbatim_hotkey"].includes(field)) ? saveFailure.message : null;
  const extraShortcutsError = Object.entries(captureErrors).find(([key, error]) => key !== "dictation" && error)?.[1];
  const configuredShortcuts = [settings.selected_action_hotkey, settings.translation_hotkey, settings.screen_action_hotkey, settings.verbatim_hotkey].filter((hotkey) => hotkey?.trim()).length;
  const asrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const asrProfile = asrModelProfile(asrProvider, settings.asr_model);
  return (
    <SettingsShell>
      <SettingsPageHeader title={t("录音与输出")} description={t("快捷键、识别语言、输出方式和本地存储。")} />
      <SettingsGroup title={t("日常听写")}>
        <SettingsRow title={t("全局快捷键")} description={t("在任何 App 中开始听写，设置自动保存。") }>
          <HotkeyRecorder
            compact
            disabled={dictationBusy || modeSaving || otherCaptureBusy("dictation")}
            onCaptureBusyChange={(busy) => updateBusy("dictation", busy)}
            onCaptureErrorChange={(error) => updateError("dictation", error)}
            value={settings.hotkey}
            onChange={(hotkey, options) => save({ hotkey }, options)}
          />
        </SettingsRow>
        {settings.hotkey_error && <p role="alert" className="px-5 pb-4 text-xs leading-5 text-error-ink">{t("快捷键注册失败：")}{settings.hotkey_error}。{t("请重新设置一个快捷键。")}</p>}
        <SettingsRow title={t("录音方式")}>
          <ActivationModeSelector
            value={settings.activation_mode}
            hotkey={settings.hotkey}
            disabled={dictationBusy || modeSaving || Object.values(captureBusy).some(Boolean)}
            onChange={(activation_mode) => {
              setModeSaving(true);
              setModeError(null);
              void invoke("update_settings_patch", { patch: { activation_mode } })
                .then(() => save({ activation_mode }, { persist: false }))
                .catch((reason) => setModeError(String(reason)))
                .finally(() => setModeSaving(false));
            }}
          />
        </SettingsRow>
        {modeError && <p role="alert" className="px-5 pb-4 text-xs text-error-ink">{modeError}</p>}
        {isFnOnlyHotkey(settings.hotkey) && <div className="px-5 pb-2"><FnHotkeyNote hotkey={settings.hotkey} /></div>}
        <SettingsRow title={t("识别语言")} description={asrLanguageDescription(asrProfile, settings.language, t)}>
          <Select aria-label={t("识别语言")} value={settings.language} onValueChange={(value) => save({ language: value })} className={selectClass}><option value="auto">{t("自动检测")}</option><option value="zh">{t("中文")}</option><option value="en">{t("English")}</option></Select>
        </SettingsRow>
        <SettingsRow title={t("输出方式")} description={t("自动优先粘贴；目标未接收时，保留文字并复制到剪贴板。") }>
          <Select aria-label={t("输出方式")} value={settings.delivery_policy} onValueChange={(value) => save({ delivery_policy: value })} className={selectClass}>
            <option value="auto">{t("自动（优先粘贴）")}</option>
            <option value="paste_shortcut">{t("写入当前输入框")}</option>
            <option value="clipboard_only">{t("复制到剪贴板")}</option>
            <option value="history_only">{t("仅保存到历史")}</option>
          </Select>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("录音反馈")}>
        <SettingsRow title={t("录音提示音")} description={t("录音开始和结束时播放短提示。")}>
          <Toggle
            checked={settings.audio_feedback_enabled ?? false}
            onChange={(checked) => save({ audio_feedback_enabled: checked })}
            label={t("录音提示音")}
          />
        </SettingsRow>
        {(settings.audio_feedback_enabled ?? false) && (
          <SettingsRow title={t("提示音音量")} description={t("范围 0.0–1.0。")}>
            <VolumeNumberField
              label={t("提示音音量")}
              value={settings.audio_feedback_volume ?? 0.6}
              onCommit={(audio_feedback_volume) => save({ audio_feedback_volume })}
            />
          </SettingsRow>
        )}
      </SettingsGroup>
      <SettingsGroup title={t("本地保留")}>
        <div>
          <SettingsRow title={t("保留音频")} description={t("设置本机恢复音频的保留时间，过期后会自动清理。")}>
            <Select
              aria-label={t("音频缓存保留时间")}
              value={String(settings.keep_audio_days)}
              onValueChange={(value) => save({ keep_audio_days: Number(value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {retentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </Select>
          </SettingsRow>
          {settings.keep_audio_days === 365 && <p className="px-5 pb-4 text-xs text-warning-ink">{t("较长时间保留可能占用更多磁盘空间。")} </p>}
        </div>
        <div>
          <SettingsRow
            title={t("成功听写也保留音频")}
            description={t("打开后，成功听写也会在本机保存 wav，供以后本地训练。占用磁盘，含你说的话，不会上传。密码框和默认关学习的目标不会保留。")}
          >
            <Toggle
              checked={Boolean(settings.keep_success_audio)}
              onChange={(checked) => save({ keep_success_audio: checked })}
              label={t("成功听写也保留音频")}
            />
          </SettingsRow>
          {settings.keep_success_audio && settings.keep_audio_days === 7 && (
            <p role="note" className="px-5 pb-4 text-xs text-warning-ink">
              {t("训练建议把音频保留至少 90 天或 1 年。")}
            </p>
          )}
        </div>
        <div>
          <SettingsRow title={t("保留历史文字")} description={t("自动清理本机历史记录中的原始文字、整理结果和上下文策略。")}>
            <Select
              aria-label={t("历史文字保留时间")}
              value={String(settings.keep_history_days)}
              onValueChange={(value) => save({ keep_history_days: Number(value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {historyRetentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </Select>
          </SettingsRow>
          {(settings.keep_history_days === 0 || settings.keep_history_days >= 3650) && <p className="px-5 pb-4 text-xs text-warning-ink">{settings.keep_history_days === 0 ? t("历史记录会永久保留，除非你手动删除或清空。") : t("历史记录会长期保留，请定期清理。")} </p>}
        </div>
      </SettingsGroup>
      <SettingsDisclosure
        title={t("更多快捷键")}
        description={t("为选中文本、翻译、看屏幕和跳过整理设置独立快捷键。")}
        summary={extraShortcutsBusy ? t("快捷键录制中") : t("已配置 {count} / 4").replace("{count}", String(configuredShortcuts))}
        locked={extraShortcutsBusy}
        error={extraShortcutsError ?? shortcutSaveError}
      >
      <SettingsGroup title={t("选中文本操作")} description={t("先选中文本，再用独立快捷键说出改写、缩短、翻译或总结指令。")}>
        <SettingsRow
          title={t("启用选中文本操作")}
          description={(
            <>
              {t("默认开启；使用独立快捷键，不会自动保存原选中文本。")}
              {settings.selected_actions_enabled && !settings.selected_action_hotkey?.trim() && (
                <span role="status" className="mt-1 block text-warning-ink">{t("请先设置快捷键后才能触发")}</span>
              )}
            </>
          )}
        >
          <Toggle checked={Boolean(settings.selected_actions_enabled)} onChange={(checked) => save({ selected_actions_enabled: checked })} label={t("启用选中文本操作")} />
        </SettingsRow>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("选中文本快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("选中文本后按它开始录音，再按一次结束；目标或选区变化时只复制结果，不会替换文字。")}</p>
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || modeSaving || otherCaptureBusy("selected_action")}
              onCaptureBusyChange={(busy) => updateBusy("selected_action", busy)}
              onCaptureErrorChange={(error) => updateError("selected_action", error)}
              value={settings.selected_action_hotkey ?? ""}
              captureTarget="selected_action"
              onChange={(hotkey, options) => save({ selected_action_hotkey: hotkey, selected_actions_enabled: true }, options)}
            />
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("本次翻译")} description={t("用独立快捷键启动翻译听写，只影响本次录音，不改变日常输出模式。")}>
        <SettingsRow title={t("目标语言")} description={t("与输出模式中的翻译共用目标语言；开始录音后固定本次目标语言。")}>
          <Select aria-label={t("快捷翻译目标语言")} value={settings.translation_target_language ?? "en"} onValueChange={(value) => save({ translation_target_language: value })} className={selectClass}>
            {translationLanguageOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
          </Select>
        </SettingsRow>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("翻译快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。与全局快捷键使用相同的录音方式（点按切换 / 按住说话）。")}</p>
          <p className="mt-2 text-xs leading-5 text-tertiary">{t("需要当前场景允许 AI 整理，并已配置整理服务；严格离线时只使用本机模型。")}</p>
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || modeSaving || otherCaptureBusy("translation_action")}
              onCaptureBusyChange={(busy) => updateBusy("translation_action", busy)}
              onCaptureErrorChange={(error) => updateError("translation_action", error)} value={settings.translation_hotkey ?? ""} captureTarget="translation_action" onChange={(hotkey, options) => save({ translation_hotkey: hotkey }, options)} />
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("看屏幕")} description={t("独立快捷键，先截当前窗口再说话；结果只进预览，默认听写不会截屏。")}>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("看屏幕快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。需要屏幕录制、辅助功能和已配置的视觉模型。")}</p>
          {!settings.screen_action_hotkey?.trim() && (
            <p className="mt-2 text-xs text-tertiary">{t("未设置快捷键，看屏幕不会触发")}</p>
          )}
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || modeSaving || otherCaptureBusy("screen_action")}
              onCaptureBusyChange={(busy) => updateBusy("screen_action", busy)}
              onCaptureErrorChange={(error) => updateError("screen_action", error)}
              value={settings.screen_action_hotkey ?? ""}
              captureTarget="screen_action"
              onChange={(hotkey, options) => save({ screen_action_hotkey: hotkey }, options)}
            />
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("本次跳过 AI 整理")} description={t("独立快捷键，跳过 AI 整理，仍会做本地处理、使用所选语音服务。")}>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("跳过整理快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。与全局快捷键使用相同的录音方式（点按切换 / 按住说话）。")}</p>
          {!settings.verbatim_hotkey?.trim() && (
            <p className="mt-2 text-xs text-tertiary">{t("未设置跳过整理快捷键")}</p>
          )}
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || modeSaving || otherCaptureBusy("verbatim_action")}
              onCaptureBusyChange={(busy) => updateBusy("verbatim_action", busy)}
              onCaptureErrorChange={(error) => updateError("verbatim_action", error)}
              value={settings.verbatim_hotkey ?? ""}
              captureTarget="verbatim_action"
              onChange={(hotkey, options) => save({ verbatim_hotkey: hotkey }, options)}
            />
          </div>
        </div>
      </SettingsGroup>
      </SettingsDisclosure>
      <SettingsDisclosure title={t("高级录音设置")} description={t("调整麦克风增益和长录音分段。")} error={advancedSaveError}>
      <SettingsGroup title={t("麦克风增益")}>
        <SettingsRow title={t("输入增益")} description={<>{t("放大麦克风音量，适合说话较轻的情况。1.0 为原始音量。范围 0.5–4.0。")}{(settings.input_gain ?? 1) > 1 && <p role="note" className="mt-1 text-xs leading-5 text-secondary">{t("增益大于 1 时，过大的声音会被压限，避免削波。说话很轻再提高。")}</p>}</>}>
          <GainNumberField
            label={t("输入增益")}
            value={settings.input_gain ?? 1}
            min={0.5}
            max={4}
            onCommit={(input_gain) => save({ input_gain })}
          />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("长录音")} description={t("较长的录音会自动分段处理，避免一次请求过大。")}>
        <SettingsRow title={t("开始分段（秒）")} description={t("超过这个时长后开始分段。默认 25 秒。范围 5–3600。")}>
          <ChunkNumberField label={t("开始分段（秒）")} value={settings.chunk_threshold_secs} min={5} max={3600} onCommit={(chunk_threshold_secs) => save({ chunk_threshold_secs })} />
        </SettingsRow>
        <SettingsRow title={t("每段长度（秒）")} description={t("每个语音请求的目标长度。默认 35 秒。范围 15–60。")}>
          <ChunkNumberField label={t("每段长度（秒）")} value={settings.chunk_length_secs} min={15} max={60} onCommit={(chunk_length_secs) => save({ chunk_length_secs })} />
        </SettingsRow>
      </SettingsGroup>
      </SettingsDisclosure>

    </SettingsShell>
  );
}

function VolumeNumberField({
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
    const clamped = Math.min(1, Math.max(0, Math.round(parsed * 10) / 10));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      aria-label={label}
      type="number"
      min={0}
      max={1}
      step={0.1}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${controlClass} w-[88px] text-right`}
    />
  );
}

function GainNumberField({
  label,
  value,
  min,
  max,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
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
    const clamped = Math.min(max, Math.max(min, Math.round(parsed * 10) / 10));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      aria-label={label}
      type="number"
      min={min}
      max={max}
      step={0.1}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${controlClass} w-[88px] text-right`}
    />
  );
}

function ChunkNumberField({
  label,
  value,
  min,
  max,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
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
    const clamped = Math.min(max, Math.max(min, Math.round(parsed)));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      aria-label={label}
      type="number"
      min={min}
      max={max}
      step={1}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${controlClass} w-[88px] text-right`}
    />
  );
}
