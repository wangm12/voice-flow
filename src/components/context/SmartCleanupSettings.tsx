import { Globe, ScanSearch } from "lucide-react";
import { Select } from "../Select";
import { Toggle } from "../Toggle";
import { SettingsDisclosure, SettingsGroup, SettingsRow } from "../SettingsLayout";
import { ContextLesson } from "../Onboarding/ContextualLessons";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass, secondaryButtonClass } from "../../lib/theme";
import { translationLanguageOptions } from "../../lib/translationLanguages";
import { providerById, type ProviderId } from "../../lib/providers";
import {
  type ApplicationOption,
  type ContextFamily,
  type ContextMapping,
  type ContextSnapshot,
  familyLabels,
  fieldClass,
  matchingSelectors,
  outputModeOptions,
  sourceLabel,
} from "./contextShared";

export type SmartCleanupSettingsProps = {
  combined?: boolean;
  busy: boolean;
  busyAction: "context" | "browser" | "override" | "applications" | "mapping" | "delete_mapping" | null;
  enabled: boolean;
  onToggleEnabled: (nextEnabled: boolean) => void;
  snapshot: ContextSnapshot | null;
  mappings: ContextMapping[];
  applications: ApplicationOption[];
  cleanupIntensity: "auto" | "off" | "light" | "standard" | "heavy";
  onCleanupIntensityChange?: (intensity: "auto" | "off" | "light" | "standard" | "heavy") => void;
  windowOcrEnabled: boolean;
  onWindowOcrEnabledChange?: (enabled: boolean) => void;
  onRequestBrowserAccess: () => void;
  outputMode?: string;
  onOutputModeChange?: (outputMode: string) => void;
  translationTargetLanguage?: string;
  onTranslationTargetLanguageChange?: (language: string) => void;
  onOpenPermissions?: () => void;
  onOpenMappings?: () => void;
  advancedError: string | null;
  overrideFamily: ContextFamily | null;
  onSetOverride: (value: string) => void;
  accurateAsrProvider: ProviderId;
  onAccurateAsrProviderChange?: (provider: ProviderId) => void;
  accurateAsrModel: string;
  onAccurateAsrModelChange?: (model: string) => void;
  accurateAsrBaseUrl: string;
  onAccurateAsrBaseUrlChange?: (url: string) => void;
  visionProvider: string;
  onVisionProviderChange?: (provider: string) => void;
  visionModel: string;
  onVisionModelChange?: (model: string) => void;
  contextError: string | null;
  browserError: string | null;
};

export function SmartCleanupSettings({
  combined,
  busy,
  busyAction,
  enabled,
  onToggleEnabled,
  snapshot,
  mappings,
  applications,
  cleanupIntensity,
  onCleanupIntensityChange,
  windowOcrEnabled,
  onWindowOcrEnabledChange,
  onRequestBrowserAccess,
  outputMode,
  onOutputModeChange,
  translationTargetLanguage,
  onTranslationTargetLanguageChange,
  onOpenPermissions,
  onOpenMappings,
  advancedError,
  overrideFamily,
  onSetOverride,
  accurateAsrProvider,
  onAccurateAsrProviderChange,
  accurateAsrModel,
  onAccurateAsrModelChange,
  accurateAsrBaseUrl,
  onAccurateAsrBaseUrlChange,
  visionProvider,
  onVisionProviderChange,
  visionModel,
  onVisionModelChange,
  contextError,
  browserError,
}: SmartCleanupSettingsProps) {
  const { t } = useI18n();
  const inlineError = (message: string | null) => message
    ? <p role="alert" className="mt-2 break-words text-[13px] leading-5 text-error-ink">{message}</p>
    : null;

  return (
    <>
      <SettingsGroup title={t("App 上下文")} description={t("决定 VoiceFlow 是否根据当前 App 和输入框自动适配。")}>
        <SettingsRow title={t("App 上下文适配")} description={<>{t("根据你当前使用的 App 和输入框自动选择合适的 Prompt。")}{inlineError(contextError)}</>} icon={<ScanSearch size={17} strokeWidth={1.7} aria-hidden="true" />}>
          <Toggle checked={enabled} onChange={onToggleEnabled} disabled={busy} label={t("App 上下文适配")} />
        </SettingsRow>
        {snapshot && (
          (() => {
            const matchedMappingId = snapshot.profile.id.startsWith("user.")
              ? snapshot.profile.id.slice("user.".length)
              : null;
            const matchedMapping = snapshot.profile.source === "user_mapping"
              ? mappings.find((mapping) => mapping.id === matchedMappingId)
              : undefined;
            const selectors = matchedMapping ? matchingSelectors(matchedMapping, applications, t) : [];
            return (
              <SettingsRow
                title={t("当前上下文")}
                description={(
                  <>
                    <span>{matchedMapping?.label ?? snapshot.profile.app_label} · {t(familyLabels[snapshot.profile.family])}</span>
                    <span className="block">{`${t("来源")}: ${sourceLabel(snapshot.profile.source, t)}`}</span>
                    {matchedMapping && selectors.length > 0 && (
                      <span className="block">{`${t("匹配条件")}: ${selectors.join(" · ")}`}</span>
                    )}
                  </>
                )}
              />
            );
          })()
        )}
        <SettingsRow title={t("整理强度")} description={t("自动按输入场景选择整理强度，也可以按 App 单独覆盖。")}>
          <Select
            aria-label={t("整理强度")}
            value={cleanupIntensity}
            onValueChange={(value) => onCleanupIntensityChange?.(value as "auto" | "off" | "light" | "standard" | "heavy")}
            disabled={busy}
            className={`w-52 max-w-full ${fieldClass}`}
          >
            <option value="auto">{t("自动")}</option>
            <option value="off">{t("关")}</option>
            <option value="light">{t("轻")}</option>
            <option value="standard">{t("中")}</option>
            <option value="heavy">{t("重")}</option>
          </Select>
        </SettingsRow>
        <SettingsRow title={t("窗口文字识别")} description={t("仅在已授权的 App 规则允许时，辅助功能文字不足可截取当前窗口并在本机识别。默认关闭，需要屏幕录制权限；云端视觉另需单独授权。")}>
          <Toggle checked={windowOcrEnabled} onChange={(next) => onWindowOcrEnabledChange?.(next)} disabled={busy || !onWindowOcrEnabledChange} label={t("窗口文字识别")} />
        </SettingsRow>
        {snapshot?.browser_access_status !== "not_applicable" && snapshot?.browser_access_status !== "granted" && (
          <SettingsRow title={t("浏览器站点检测")} description={<>{t("Chrome / Safari 需要额外授权，VoiceFlow 只保存站点标识。")}{inlineError(browserError)}</>} icon={<Globe size={17} className="text-warning-ink" aria-hidden="true" />}>
            <button type="button" onClick={onRequestBrowserAccess} aria-busy={busyAction === "browser"} disabled={busy} className={`${secondaryButtonClass} min-w-32`}>{t(busyAction === "browser" ? "处理中…" : "启用访问")}</button>
          </SettingsRow>
        )}
      </SettingsGroup>
      <SettingsGroup title={t("输出格式")} description={t("自动整理按场景决定格式；手动选择会作为默认模式持续生效，直到你再次更改。")}>
        <SettingsRow title={t("输出模式")} description={t("自动根据当前 App、输入框和你说的内容选择整理方式；手动模式会覆盖自动判断。")}>
          <Select aria-label={t("输出模式")} value={outputMode ?? "auto"} onValueChange={(value) => onOutputModeChange?.(value)} className={`w-40 max-w-full ${fieldClass}`}>
            {outputModeOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
          </Select>
        </SettingsRow>
        {(combined || outputMode === "translation") && <SettingsRow title={t("翻译目标语言")} description={t("在翻译输出模式和翻译快捷键中使用；语音识别仍然自动检测。")}>
          <Select aria-label={t("翻译目标语言")} value={translationTargetLanguage ?? "en"} onValueChange={(value) => onTranslationTargetLanguageChange?.(value)} className={`w-40 max-w-full ${fieldClass}`}>
            {translationLanguageOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
          </Select>
        </SettingsRow>}
      </SettingsGroup>
      <ContextLesson onOpenPermissions={onOpenPermissions} onOpenMappings={onOpenMappings} />
      <SettingsDisclosure title={t("高级整理设置")} description={t("临时场景、精确转写和视觉模型。授权选项保持独立。")} error={advancedError}>
        {inlineError(advancedError)}
        <div className="divide-y divide-border">
        <SettingsRow title={t("临时覆盖")} description={t("只对当前运行有效，重启或切换 App 后会恢复自动适配。")}>
          <Select id="context-temporary-override" aria-label={t("临时覆盖")} value={overrideFamily ?? "auto"} onValueChange={onSetOverride} disabled={busy} className={`w-52 max-w-full ${fieldClass}`}>
            <option value="auto">{t("自动适配")}</option>
            {Object.entries(familyLabels).map(([value, label]) => <option key={value} value={value}>{t(label)}</option>)}
          </Select>
        </SettingsRow>
        <SettingsRow title={t("精确转写")} description={t("中英混合、人名多、主转写失败或置信度低时才打第二枪；留空仍关闭。")}>
          <div className="flex w-full max-w-md flex-col gap-2">
            <div className="flex w-full flex-col gap-2 sm:flex-row">
              <Select
                aria-label={t("精确转写服务商")}
                value={accurateAsrProvider}
                onValueChange={(value) => onAccurateAsrProviderChange?.(value as ProviderId)}
                disabled={busy || !onAccurateAsrProviderChange}
                className={`w-full sm:w-40 ${fieldClass}`}
              >
                <option value="groq">Groq</option>
                <option value="openai">OpenAI</option>
                <option value="siliconflow">SiliconFlow</option>
                <option value="custom">{t("自定义")}</option>
              </Select>
              <input
                aria-label={t("精确转写模型")}
                value={accurateAsrModel}
                onChange={(event) => onAccurateAsrModelChange?.(event.target.value)}
                disabled={busy || !onAccurateAsrModelChange}
                placeholder={t("例如 whisper-large-v3 或 qwen3-asr-flash")}
                className={`w-full ${fieldClass}`}
              />
            </div>
            {accurateAsrProvider === "custom" && (
              <input
                aria-label={t("精确转写地址")}
                value={accurateAsrBaseUrl}
                onChange={(event) => onAccurateAsrBaseUrlChange?.(event.target.value)}
                disabled={busy || !onAccurateAsrBaseUrlChange}
                placeholder="https://api.example.com/v1"
                className={`w-full ${fieldClass}`}
              />
            )}
            <p className="text-xs leading-5 text-tertiary">
              {t("第二枪需要 {provider} 上已填的密钥。").replace(
                "{provider}",
                accurateAsrProvider === "custom"
                  ? t("自定义 / 兼容接口")
                  : providerById(accurateAsrProvider as ProviderId)?.label ?? accurateAsrProvider,
              )}
            </p>
            <button
              type="button"
              onClick={() => {
                onAccurateAsrProviderChange?.("custom");
                onAccurateAsrModelChange?.("qwen3-asr-flash");
                onAccurateAsrBaseUrlChange?.("https://dashscope.aliyuncs.com/compatible-mode/v1");
              }}
              className={`self-start ${compactButtonClass}`}
            >
              {t("用百炼 Qwen3-ASR 补一枪")}
            </button>
          </div>
        </SettingsRow>
        <SettingsRow title={t("视觉模型")} description={t("显式看屏幕操作，或已单独授权的 App 规则触发本机证据不足回退时，才会把当前窗口图发给此模型。默认听写不截屏；未配置则不发送。")}>
          <div className="flex w-full max-w-md flex-col gap-2 sm:flex-row">
            <Select
              aria-label={t("视觉服务商")}
              value={visionProvider}
              onValueChange={(value) => onVisionProviderChange?.(value)}
              disabled={busy || !onVisionProviderChange}
              className={`w-full sm:w-40 ${fieldClass}`}
            >
              <option value="">{t("未配置")}</option>
              <option value="openai">OpenAI</option>
              <option value="groq">Groq</option>
              <option value="ollama">Ollama</option>
              <option value="custom">{t("自定义")}</option>
            </Select>
            <input
              aria-label={t("视觉模型")}
              value={visionModel}
              onChange={(event) => onVisionModelChange?.(event.target.value)}
              disabled={busy || !onVisionModelChange}
              placeholder={t("例如 gpt-4o")}
              className={`w-full ${fieldClass}`}
            />
          </div>
        </SettingsRow>
        </div>
      </SettingsDisclosure>
    </>
  );
}
