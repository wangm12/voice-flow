import { useEffect, useState } from "react";
import { ActivationModeSelector } from "../ActivationModeSelector";
import { HotkeyRecorder } from "../HotkeyRecorder";
import { HotkeyUsageGuide } from "../HotkeyUsageGuide";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { resolveCapturedActivationMode } from "../../lib/activationCopy";
import { isFnOnlyHotkey, isModifierOnlyHotkey } from "../../lib/hotkeyFormat";
import { useI18n } from "../../lib/i18n";
import { colors, focusRingClass, radius } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;
const selectClass = `${controlClass} w-32`;
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

export function RecordingSettings({ settings, save }: { settings: Settings; save: SaveSettings }) {
  const { t } = useI18n();
  return (
    <SettingsShell>
      <SettingsPageHeader title={t("录音与输出")} description={t("快捷键、识别语言、输出方式和本地存储。")} />
      <SettingsGroup title={t("快捷键")}>
        <div className="px-4 py-4 sm:px-5">
          <p className="text-sm font-medium text-primary">{t("全局快捷键")}</p>
          <p className="mt-1 text-xs text-tertiary">{t("在 Cursor、浏览器、邮件等 App 中都能使用。")}</p>
          {settings.hotkey_error && <p role="alert" className="mt-3 rounded-lg bg-error/5 px-3 py-2 text-xs text-error">{t("快捷键注册失败：")}{settings.hotkey_error}。{t("请重新设置一个快捷键。")}</p>}
          <div className="mt-4"><HotkeyRecorder value={settings.hotkey} onChange={(hotkey, mode, options) => {
            const activation_mode = resolveCapturedActivationMode(settings.activation_mode, hotkey, mode);
            save(activation_mode ? { hotkey, activation_mode } : { hotkey }, options);
          }} /></div>
          <div className="mt-4">
            <ActivationModeSelector
              value={isModifierOnlyHotkey(settings.hotkey) ? "double_tap" : settings.activation_mode}
              modifierOnly={isModifierOnlyHotkey(settings.hotkey)}
              onChange={(activation_mode) => save({ activation_mode })}
            />
          </div>
          <HotkeyUsageGuide hotkey={settings.hotkey} activationMode={settings.activation_mode} />
          {isFnOnlyHotkey(settings.hotkey) && (
            <p role="note" className="mt-3 text-xs leading-5 text-tertiary">
              {t("微信 / 微信输入法可能会占用 Fn 键，VoiceFlow 可能收不到这个快捷键。")}
            </p>
          )}
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("选中文本操作")} description={t("先选中文本，再用独立快捷键说出改写、缩短、翻译或总结指令。")}>
        <SettingsRow
          title={t("启用选中文本操作")}
          description={(
            <>
              {t("默认开启；使用独立快捷键，不会自动保存原选中文本。")}
              {settings.selected_actions_enabled && !settings.selected_action_hotkey?.trim() && (
                <span role="status" className="mt-1 block text-warning">{t("请先设置快捷键后才能触发")}</span>
              )}
            </>
          )}
        >
          <Toggle checked={Boolean(settings.selected_actions_enabled)} onChange={(checked) => save({ selected_actions_enabled: checked })} label={t("启用选中文本操作")} />
        </SettingsRow>
        <div className="px-4 py-4 sm:px-5">
          <p className="text-sm font-medium text-primary">{t("选中文本快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("选中文本后按它开始录音，再按一次结束；目标或选区变化时只复制结果，不会替换文字。")}</p>
          <div className="mt-4">
            <HotkeyRecorder
              value={settings.selected_action_hotkey ?? ""}
              captureTarget="selected_action"
              onChange={(hotkey, _mode, options) => save({ selected_action_hotkey: hotkey, selected_actions_enabled: true }, options)}
            />
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("识别")}>
        <SettingsRow title={t("识别语言")} description={t("自动检测适合中文、English 和混合语音。只有在识别结果不稳定时，才建议手动指定。")}>
          <select aria-label={t("识别语言")} value={settings.language} onChange={(event) => save({ language: event.target.value })} className={selectClass}><option value="auto">{t("自动检测")}</option><option value="zh">{t("中文")}</option><option value="en">{t("English")}</option></select>
        </SettingsRow>
        <SettingsRow title={t("输入增益")} description={t("放大麦克风音量，适合说话较轻的情况。1.0 为原始音量。范围 0.5–4.0。")}>
          <GainNumberField
            label={t("输入增益")}
            value={settings.input_gain ?? 1}
            min={0.5}
            max={4}
            onCommit={(input_gain) => save({ input_gain })}
          />
        </SettingsRow>
        {(settings.input_gain ?? 1) > 1 && (
          <p role="note" className="px-4 pb-3 text-xs leading-5 text-tertiary sm:px-5">
            {t("增益大于 1 时，过大的声音会被压限，避免削波。说话很轻再提高。")}
          </p>
        )}
      </SettingsGroup>
      <SettingsGroup title={t("输出方式")} description={t("短录音和长录音都先尝试写入当前输入框；如果目标没有接收，会保留文字并复制到剪贴板。你也可以改成只复制或仅保存历史。")}>
        <SettingsRow title={t("默认行为")} description={t("自动会先粘贴；目标没有接收时保留文字并复制到剪贴板。")}>
          <select aria-label={t("输出方式")} value={settings.delivery_policy} onChange={(event) => save({ delivery_policy: event.target.value })} className={selectClass}>
            <option value="auto">{t("自动（优先粘贴）")}</option>
            <option value="paste_shortcut">{t("写入当前输入框")}</option>
            <option value="clipboard_only">{t("复制到剪贴板")}</option>
            <option value="history_only">{t("仅保存到历史")}</option>
          </select>
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
      <SettingsGroup title={t("本地存储")}>
        <div>
          <SettingsRow title={t("保留音频")} description={t("设置本机恢复音频的保留时间，过期后会自动清理。")}>
            <select
              aria-label={t("音频缓存保留时间")}
              value={String(settings.keep_audio_days)}
              onChange={(event) => save({ keep_audio_days: Number(event.target.value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {retentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </select>
          </SettingsRow>
          {settings.keep_audio_days === 365 && <p className="px-4 pb-4 text-xs text-warning sm:px-5">{t("较长时间保留可能占用更多磁盘空间。")} </p>}
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
            <p role="note" className="px-4 pb-4 text-xs text-warning sm:px-5">
              {t("训练建议把音频保留至少 90 天或 1 年。")}
            </p>
          )}
        </div>
        <div>
          <SettingsRow title={t("保留历史文字")} description={t("自动清理本机历史记录中的原始文字、整理结果和上下文策略。")}>
            <select
              aria-label={t("历史文字保留时间")}
              value={String(settings.keep_history_days)}
              onChange={(event) => save({ keep_history_days: Number(event.target.value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {historyRetentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </select>
          </SettingsRow>
          {(settings.keep_history_days === 0 || settings.keep_history_days >= 3650) && <p className="px-4 pb-4 text-xs text-warning sm:px-5">{settings.keep_history_days === 0 ? t("历史记录会永久保留，除非你手动删除或清空。") : t("历史记录会长期保留，请定期清理。")} </p>}
        </div>
      </SettingsGroup>
    </SettingsShell>
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
      className={`${controlClass} w-28 text-right`}
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
      className={`${controlClass} w-28 text-right`}
    />
  );
}
