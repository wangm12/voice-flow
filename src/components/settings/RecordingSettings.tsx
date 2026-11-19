import { audioRetentionOptions as retentionOptions, historyRetentionOptions } from "../../lib/retention";
import { invoke } from "@tauri-apps/api/core";
import { Select } from "../Select";
import { FnHotkeyNote } from "../FnHotkeyNote";
import { useCallback, useState } from "react";
import { ClampedNumberInput } from "./ClampedNumberInput";
import { HotkeyRecorder } from "../HotkeyRecorder";
import { SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, type SettingsSaveFailure } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { ContextLesson, SelectedLesson } from "../Onboarding/ContextualLessons";
import { translationLanguageOptions } from "../../lib/translationLanguages";
import { ACTIVATION_MODES, recordingInstruction } from "../../lib/activationCopy";
import { useI18n } from "../../lib/i18n";
import { formatHotkeyDisplay, isFnOnlyHotkey } from "../../lib/hotkeyFormat";
import { asrLanguageDescription, providerOf } from "../../lib/engineWizard";
import { asrModelProfile } from "../../lib/providers";
import { MAX_PREFERRED_LANGUAGES, PREFERRED_LANGUAGE_OPTIONS, SUPPORTED_LANGUAGES, languageSupportedByModel, modelLanguageSupport, normalizePreferredLanguages } from "../../lib/languages";
import { compactButtonClass, inputClass } from "../../lib/theme";
import { friendlySettingsError } from "../../lib/settingsError";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = inputClass;
const selectClass = `${controlClass} w-44`;

export function RecordingSettings({ settings, save, saveFailure, dictationBusy = false, onOpenPermissions, onOpenMappings }: { settings: Settings; save: SaveSettings; saveFailure?: SettingsSaveFailure | null; dictationBusy?: boolean; onOpenPermissions?: () => void; onOpenMappings?: () => void }) {
  const { t } = useI18n();
  const [captureBusy, setCaptureBusy] = useState<Record<string, boolean>>({});
  const [captureErrors, setCaptureErrors] = useState<Record<string, string | null>>({});
  const [permissionNoticeError, setPermissionNoticeError] = useState<string | null>(null);
  const updateBusy = useCallback((target: string, busy: boolean) => {
    setCaptureBusy((previous) => previous[target] === busy ? previous : { ...previous, [target]: busy });
  }, []);
  const updateError = useCallback((target: string, error: string | null) => {
    setCaptureErrors((previous) => previous[target] === error ? previous : { ...previous, [target]: error });
  }, []);
  const otherCaptureBusy = (target: string) => Object.entries(captureBusy).some(([key, busy]) => key !== target && busy);
  const extraShortcutsBusy = Object.entries(captureBusy).some(([key, busy]) => key !== "dictation" && busy);
  const modeError = saveFailure?.fields.includes("activation_mode") ? saveFailure.message : null;
  const languageError = saveFailure?.fields.some((field) => field === "language" || field === "preferred_languages") ? saveFailure.message : null;
  const preferredLanguages = normalizePreferredLanguages(settings.preferred_languages);
  const advancedSaveError = saveFailure?.fields.some((field) => ["input_gain", "chunk_threshold_secs", "chunk_length_secs"].includes(field)) ? saveFailure.message : null;
  const shortcutSaveError = saveFailure?.fields.some((field) => ["selected_action_hotkey", "selected_actions_enabled", "translation_hotkey", "screen_action_hotkey", "screen_actions_enabled", "verbatim_hotkey"].includes(field)) ? saveFailure.message : null;
  const extraShortcutsError = Object.entries(captureErrors).find(([key, error]) => key !== "dictation" && error)?.[1];
  const configuredShortcuts = [settings.selected_action_hotkey, settings.translation_hotkey, settings.screen_action_hotkey, settings.verbatim_hotkey].filter((hotkey) => hotkey?.trim()).length;
  const hasEnabledUnboundShortcut = Boolean(settings.selected_actions_enabled && !settings.selected_action_hotkey?.trim());
  const hotkeyErrorMessage = settings.hotkey_error === "input_monitoring_required" || settings.hotkey_error === "modifier_event_tap_unavailable"
    ? friendlySettingsError(settings.hotkey_error, t)
    : settings.hotkey_error;
  const asrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const asrProfile = asrModelProfile(asrProvider, settings.asr_model);
  const langSupport = modelLanguageSupport(asrProvider, settings.asr_model);
  const languageAllowed = languageSupportedByModel(asrProvider, settings.asr_model, settings.language);
  const languageOptions = SUPPORTED_LANGUAGES.filter((item) =>
    langSupport.allowedCodes.includes(item.code) || item.code === settings.language,
  );
  return (
    <SettingsShell>
      <SettingsPageHeader title={t("录音与输出")} description={t("快捷键、识别语言、输出方式和本地存储。")} saveHint={t("更改自动保存")} />
      <SettingsGroup title={t("日常听写")}>
        <SettingsRow title={t("全局快捷键")} description={t("在任何 App 中开始听写，设置自动保存。") }>
          <HotkeyRecorder
            compact
            disabled={dictationBusy || otherCaptureBusy("dictation")}
            onCaptureBusyChange={(busy) => updateBusy("dictation", busy)}
            onCaptureErrorChange={(error) => updateError("dictation", error)}
            value={settings.hotkey}
            onChange={(hotkey, options) => save({ hotkey }, options)}
          />
        </SettingsRow>
        {hotkeyErrorMessage && <p role="alert" className="px-5 pb-4 text-xs leading-5 text-error-ink">{t("快捷键注册失败：")}{hotkeyErrorMessage}。{t("请重新设置一个快捷键。")}</p>}
        {settings.hotkey_notice === "combo_release_uses_carbon_fallback" && (
          <div role="note" className="px-5 pb-4 text-xs leading-5 text-secondary">
            <p>{t("修饰键提前释放检测当前不可用；组合快捷键仍可使用，但结束录音会等到整组按键松开。输入监控只读取按键状态，不读取输入内容。")}</p>
            <button type="button" className={`mt-1 ${compactButtonClass}`} onClick={() => {
              void invoke("open_privacy_settings", { pane: "input_monitoring" }).catch(() => setPermissionNoticeError(t("无法打开输入监控设置，请从系统设置的隐私与安全性中打开。")));
            }}>{t("打开输入监控设置")}</button>
            {permissionNoticeError && <p role="alert" className="mt-1 text-error-ink">{permissionNoticeError}</p>}
          </div>
        )}
        <SettingsRow
          title={t("录音方式")}
          description={(
            <>
              {recordingInstruction(settings.activation_mode, formatHotkeyDisplay(settings.hotkey), t)}
              <span className="mt-1 block text-xs text-tertiary">{t("Esc 取消。")}</span>
            </>
          )}
        >
          <Select
            aria-label={t("录音方式")}
            value={settings.activation_mode}
            disabled={dictationBusy || Object.values(captureBusy).some(Boolean)}
            onValueChange={(value) => {
              if (value !== "tap" && value !== "hold_to_talk" && value !== "hybrid_hold_lock") return;
              save({ activation_mode: value }, { immediate: true });
            }}
            className={`${controlClass} w-40 max-w-full`}
          >
            {ACTIVATION_MODES.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}</option>)}
          </Select>
        </SettingsRow>
        {modeError && <p role="alert" className="px-5 pb-4 text-xs text-error-ink">{modeError}</p>}
        {isFnOnlyHotkey(settings.hotkey) && <div className="px-5 pb-2"><FnHotkeyNote hotkey={settings.hotkey} /></div>}
        <SettingsRow title={t("识别语言")} description={asrLanguageDescription(asrProfile, settings.language, t)}>
          <Select aria-label={t("识别语言")} value={settings.language} onValueChange={(value) => save({ language: value }, { immediate: true })} className={selectClass}>
            {languageOptions.map((item) => {
              const supported = langSupport.allowedCodes.includes(item.code);
              const baseLabel = item.code === "auto"
                ? t("自动检测")
                : item.code === "zh"
                  ? t("中文（简体）")
                  : item.code === "zh-TW"
                    ? t("繁體中文")
                    : item.nativeName;
              return (
                <option key={item.code} value={item.code} disabled={!supported}>
                  {supported ? baseLabel : `${baseLabel} (${t("当前模型不支持")})`}
                </option>
              );
            })}
          </Select>
        </SettingsRow>
        {!languageAllowed && (
          <p role="alert" className="px-5 pb-3 text-xs text-error-ink">
            {t("当前所选识别语言不受当前转写模型支持，请改选支持列表中的语言。")}
          </p>
        )}
        {settings.language === "auto" && langSupport.hintsAllowed && (
          <div className="border-t border-border px-5 py-4">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div>
                <p className="text-[13px] font-medium text-primary">{t("常用口述语言（可选）")}</p>
                <p className="mt-0.5 text-xs leading-5 text-secondary">
                  {t("留空将对全部支持语言使用无偏向自动检测；需要中英或日英等双语混说时，最多可勾选 4 种常用语言作为候选提示。")}
                </p>
              </div>
              {preferredLanguages.length > 0 && (
                <button
                  type="button"
                  onClick={() => save({ preferred_languages: [] }, { immediate: true })}
                  className="text-xs text-secondary underline hover:text-primary"
                >
                  {t("清空（无偏向自动检测）")}
                </button>
              )}
            </div>
            <div className="mt-3 flex flex-wrap gap-1.5" role="group" aria-label={t("常用口述语言（可选）")}>
              {PREFERRED_LANGUAGE_OPTIONS.filter(
                (item) => langSupport.allowedCodes.includes(item.code) || preferredLanguages.includes(item.code),
              ).map((item) => {
                const selected = preferredLanguages.includes(item.code);
                const disabled = !selected && preferredLanguages.length >= MAX_PREFERRED_LANGUAGES;
                const label = item.code === "zh" ? t("中文（简体）") : item.code === "zh-TW" ? t("繁體中文") : item.nativeName;
                return (
                  <button
                    key={item.code}
                    type="button"
                    aria-pressed={selected}
                    disabled={disabled}
                    onClick={() => {
                      const next = selected
                        ? preferredLanguages.filter((code) => code !== item.code)
                        : [...preferredLanguages, item.code];
                      save({ preferred_languages: next }, { immediate: true });
                    }}
                    className={`rounded-md border px-2.5 py-1 text-xs transition ${
                      selected
                        ? "border-accent bg-accent/15 font-medium text-primary"
                        : "border-border bg-elevated text-secondary hover:text-primary disabled:opacity-45"
                    }`}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        )}
        {languageError && <p role="alert" className="px-5 pb-4 text-xs text-error-ink">{languageError}</p>}
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
            <ClampedNumberInput
              label={t("提示音音量")}
              value={settings.audio_feedback_volume ?? 0.6}
              min={0}
              max={1}
              step={0.1}
              decimals={1}
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
        title={(
          <span className="flex flex-wrap items-center gap-2">
            <span>{t("更多快捷键")}</span>
            {hasEnabledUnboundShortcut && <span className="rounded-full bg-warning/10 px-2 py-0.5 text-[11px] font-medium text-warning-ink">{t("有快捷键尚未绑定")}</span>}
          </span>
        )}
        description={t("为选中文本、翻译、看屏幕和跳过整理设置独立快捷键。")}
        summary={extraShortcutsBusy ? t("快捷键录制中") : t("已配置 {count} / 4").replace("{count}", String(configuredShortcuts))}
        locked={extraShortcutsBusy}
        error={extraShortcutsError ?? shortcutSaveError}
      >
      <SelectedLesson firstUse={false} />
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
              disabled={dictationBusy || otherCaptureBusy("selected_action")}
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
        <SettingsRow title={t("目标语言")} description={t("翻译目标统一在“写作与整理”的输出模式中设置；开始录音后固定本次目标语言。")}>
          <div className="flex items-center gap-2">
            <span className="text-sm text-primary">
              {translationLanguageOptions.find((option) => option.value === (settings.translation_target_language ?? "en"))?.label ?? settings.translation_target_language}
            </span>
            {onOpenMappings && <button type="button" onClick={onOpenMappings} className={compactButtonClass}>{t("更改")}</button>}
          </div>
        </SettingsRow>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("翻译快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。与全局快捷键使用相同的录音方式（点按切换 / 按住说话 / 按住或双击锁定）。")}</p>
          <p className="mt-2 text-xs leading-5 text-tertiary">{t("需要当前场景允许 AI 整理，并已配置整理服务；严格离线时只使用本机模型。")}</p>
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || otherCaptureBusy("translation_action")}
              onCaptureBusyChange={(busy) => updateBusy("translation_action", busy)}
              onCaptureErrorChange={(error) => updateError("translation_action", error)} value={settings.translation_hotkey ?? ""} captureTarget="translation_action" onChange={(hotkey, options) => save({ translation_hotkey: hotkey }, options)} />
          </div>
        </div>
      </SettingsGroup>
      <ContextLesson firstUse={false} onOpenPermissions={onOpenPermissions} onOpenMappings={onOpenMappings} />
      <SettingsGroup title={t("看屏幕")} description={t("独立快捷键，先截当前窗口再说话；结果只进预览，默认听写不会截屏。")}>
        <div className="px-5 py-4">
          <p className="text-sm font-medium text-primary">{t("看屏幕快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。需要屏幕录制、辅助功能和已配置的视觉模型。")}</p>
          {!settings.screen_action_hotkey?.trim() && (
            <p className="mt-2 text-xs text-tertiary">{t("未设置快捷键，看屏幕不会触发")}</p>
          )}
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || otherCaptureBusy("screen_action")}
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
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("空着就不注册。与全局快捷键使用相同的录音方式（点按切换 / 按住说话 / 按住或双击锁定）。")}</p>
          {!settings.verbatim_hotkey?.trim() && (
            <p className="mt-2 text-xs text-tertiary">{t("未设置跳过整理快捷键")}</p>
          )}
          <div className="mt-4">
            <HotkeyRecorder
              disabled={dictationBusy || otherCaptureBusy("verbatim_action")}
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
          <ClampedNumberInput
            label={t("输入增益")}
            value={settings.input_gain ?? 1}
            min={0.5}
            max={4}
            step={0.1}
            decimals={1}
            onCommit={(input_gain) => save({ input_gain })}
          />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("长录音")} description={t("较长的录音会自动分段处理，避免一次请求过大。")}>
        <SettingsRow title={t("开始分段（秒）")} description={t("超过这个时长后开始分段。默认 25 秒。范围 5–3600。")}>
          <ClampedNumberInput label={t("开始分段（秒）")} value={settings.chunk_threshold_secs} min={5} max={3600} onCommit={(chunk_threshold_secs) => save({ chunk_threshold_secs })} />
        </SettingsRow>
        <SettingsRow title={t("每段长度（秒）")} description={t("每个语音请求的目标长度。默认 35 秒。范围 15–60。")}>
          <ClampedNumberInput label={t("每段长度（秒）")} value={settings.chunk_length_secs} min={15} max={60} onCommit={(chunk_length_secs) => save({ chunk_length_secs })} />
        </SettingsRow>
      </SettingsGroup>
      </SettingsDisclosure>

    </SettingsShell>
  );
}
