import type { Dispatch, RefObject, SetStateAction } from "react";
import { AppWindowMac, FolderOpen, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Autocomplete } from "../Autocomplete";
import { IconButton } from "../IconButton";
import { Select } from "../Select";
import { Toggle } from "../Toggle";
import { SettingsAlert, SettingsDisclosure, SettingsGroup, SettingsRow } from "../SettingsLayout";
import { ContextLesson, SceneLesson } from "../Onboarding/ContextualLessons";
import { WritingPreferenceReview, type WritingPreferenceCandidate, type WritingPreferenceDecisionItem, type WritingPreferenceDecisionOutcome } from "../WritingPreferenceReview";
import { useI18n } from "../../lib/i18n";
import { buttonClass, colors, dangerButtonClass, focusRingClass, secondaryButtonClass } from "../../lib/theme";
import {
  type ApplicationOption,
  type ContextMapping,
  type ContextPrivacyPreset,
  type ContextSourcePermissions,
  type FocusKind,
  type PreferenceReviewConflict,
  type WritingMode,
  type WritingPreferenceDimension,
  type WritingPreferenceDraft,
  type WritingPreferenceStatus,
  type WritingPreferenceValue,
  type WritingTaskId,
  familyLabels,
  fieldClass,
  focusFieldOptions,
  hasCloudSupportedActivePreference,
  matchingSelectors,
  preferencesFromDraft,
  privacyPresetOptions,
  websiteOptions,
  writingTaskOptions,
} from "./contextShared";

function WritingPreferenceRow({
  dimension,
  label,
  value,
  status,
  valueOptions,
  preview,
  disabled,
  onValueChange,
  onStatusChange,
}: {
  dimension: WritingPreferenceDimension;
  label: string;
  value: WritingPreferenceValue | "";
  status: WritingPreferenceStatus | "";
  valueOptions: { value: WritingPreferenceValue; label: string }[];
  preview?: string;
  disabled: boolean;
  onValueChange: (value: WritingPreferenceValue | "") => void;
  onStatusChange: (status: WritingPreferenceStatus | "") => void;
}) {
  const { t } = useI18n();
  return (
    <div className="space-y-2">
      <label className="block text-xs text-secondary">{label}
        <Select aria-label={label} value={value} onValueChange={(next) => onValueChange(next as WritingPreferenceValue | "")} disabled={disabled} className={`mt-2 w-full ${fieldClass}`}>
          <option value="">{t("不保存偏好")}</option>
          {valueOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </Select>
      </label>
      <label className="block text-xs text-secondary">{t("偏好状态")}
        <Select aria-label={`${label} · ${t("偏好状态")}`} value={status} onValueChange={(next) => onStatusChange(next as WritingPreferenceStatus | "")} disabled={disabled || !value} className={`mt-2 w-full ${fieldClass}`}>
          <option value="">{t("未保存")}</option>
          <option value="active">{t("保存并应用")}</option>
          <option value="paused">{t("暂停")}</option>
          <option value="forgotten">{t("忘记此偏好")}</option>
        </Select>
      </label>
      {preview && (
        <p className="rounded-md bg-elevated/50 px-2.5 py-2 text-xs leading-5 text-secondary">
          <span className="block text-[11px] font-medium text-tertiary">{t("固定示例，不会调用服务商或修改原文")}</span>
          <span className="mt-1 block whitespace-pre-wrap">{preview}</span>
        </p>
      )}
      <span className="sr-only" data-preference-dimension={dimension}>{dimension}</span>
    </div>
  );
}

export type AppMappingEditorProps = {
  combined?: boolean;
  busy: boolean;
  busyAction: "context" | "browser" | "override" | "applications" | "mapping" | "delete_mapping" | null;
  draftKey: string;
  onOpenDictionary?: () => void;
  onOpenPermissions?: () => void;
  preferenceScopeMigrationNotice: boolean;
  selectedApplicationId: string;
  onSelectApplication: (value: string) => void;
  applicationsLoading: boolean;
  applications: ApplicationOption[];
  onAddApplicationFromDisk: () => void;
  onRefreshApplications: () => void;
  applicationsError: string | null;
  savedWritingModes: WritingMode[];
  selectedMappingMode: WritingMode | undefined;
  onSelectedModeIdChange: (modeId: string) => void;
  selectedWritingTaskId: WritingTaskId | "";
  onSelectWritingTask: (taskId: WritingTaskId | "") => void;
  writingPreferenceDraft: WritingPreferenceDraft;
  onPreferenceValueChange: (dimension: WritingPreferenceDimension, value: WritingPreferenceValue | "") => void;
  onPreferenceStatusChange: (dimension: WritingPreferenceDimension, status: WritingPreferenceStatus | "") => void;
  mappingDetailsRef: RefObject<HTMLDetailsElement | null>;
  selectedExecutable: string;
  onSelectedExecutableChange: (value: string) => void;
  selectedWebsite: string;
  onSelectedWebsiteChange: (value: string) => void;
  browserPathPrefix: string;
  onBrowserPathPrefixChange: (value: string) => void;
  focusedField: FocusKind | "";
  onFocusedFieldChange: (value: FocusKind | "") => void;
  hasStyleExamples: boolean;
  styleExampleInput: string;
  onStyleExampleInputChange: (value: string) => void;
  styleExampleOutput: string;
  onStyleExampleOutputChange: (value: string) => void;
  mappingIntensity: "inherit" | "auto" | "off" | "light" | "standard" | "heavy";
  onMappingIntensityChange: (value: "inherit" | "auto" | "off" | "light" | "standard" | "heavy") => void;
  mappingCleanupEnabled: boolean;
  onMappingCleanupEnabledChange: (value: boolean) => void;
  mappingLearnEnabled: boolean;
  onMappingLearnEnabledChange: (value: boolean) => void;
  privacyPreset: ContextPrivacyPreset;
  onApplyPrivacyPreset: (preset: Exclude<ContextPrivacyPreset, "custom">) => void;
  sourcePermissions: ContextSourcePermissions;
  onSourcePermissionsChange: Dispatch<SetStateAction<ContextSourcePermissions>>;
  styleExamplesApproved: boolean;
  onStyleExamplesApprovedChange: (approved: boolean) => void;
  preferenceGuidanceProviderApproved: boolean;
  onPreferenceGuidanceProviderApprovedChange: (approved: boolean) => void;
  retainedStylePairs: { input: string; output: string }[];
  removeStyleExamples: boolean;
  onRemoveAllStyleExamples: () => void;
  invalidCloudVisionGrant: boolean;
  mappingError: string | null;
  selectedExistingMapping: ContextMapping | undefined;
  editingMappingId: string | null;
  editingMappingOwnerMissing: boolean;
  onResetToNewMapping: () => void;
  preferenceRevisionBaselineMismatch: boolean;
  preferenceReviewConflicts: PreferenceReviewConflict[];
  onReloadMappingAfterPreferenceReview: (mappingId: string) => void;
  onSaveMapping: () => void;
  mappings: ContextMapping[];
  writingModes: WritingMode[];
  onLoadMapping: (mapping: ContextMapping) => void;
  onRequestDeleteMapping: (mapping: ContextMapping) => void;
  onLoadWritingPreferenceDrafts: () => Promise<WritingPreferenceCandidate[]>;
  onDecideWritingPreferenceDrafts: (decisions: WritingPreferenceDecisionItem[], candidates: WritingPreferenceCandidate[]) => Promise<WritingPreferenceDecisionOutcome[]>;
  onRefreshMappingsAfterPreferenceReview: (approvedCandidates: WritingPreferenceCandidate[], outcomes: WritingPreferenceDecisionOutcome[]) => Promise<void>;
};

export function AppMappingEditor({
  combined,
  busy,
  busyAction,
  draftKey,
  onOpenDictionary,
  onOpenPermissions,
  preferenceScopeMigrationNotice,
  selectedApplicationId,
  onSelectApplication,
  applicationsLoading,
  applications,
  onAddApplicationFromDisk,
  onRefreshApplications,
  applicationsError,
  savedWritingModes,
  selectedMappingMode,
  onSelectedModeIdChange,
  selectedWritingTaskId,
  onSelectWritingTask,
  writingPreferenceDraft,
  onPreferenceValueChange,
  onPreferenceStatusChange,
  mappingDetailsRef,
  selectedExecutable,
  onSelectedExecutableChange,
  selectedWebsite,
  onSelectedWebsiteChange,
  browserPathPrefix,
  onBrowserPathPrefixChange,
  focusedField,
  onFocusedFieldChange,
  hasStyleExamples,
  styleExampleInput,
  onStyleExampleInputChange,
  styleExampleOutput,
  onStyleExampleOutputChange,
  mappingIntensity,
  onMappingIntensityChange,
  mappingCleanupEnabled,
  onMappingCleanupEnabledChange,
  mappingLearnEnabled,
  onMappingLearnEnabledChange,
  privacyPreset,
  onApplyPrivacyPreset,
  sourcePermissions,
  onSourcePermissionsChange,
  styleExamplesApproved,
  onStyleExamplesApprovedChange,
  preferenceGuidanceProviderApproved,
  onPreferenceGuidanceProviderApprovedChange,
  retainedStylePairs,
  removeStyleExamples,
  onRemoveAllStyleExamples,
  invalidCloudVisionGrant,
  mappingError,
  selectedExistingMapping,
  editingMappingId,
  editingMappingOwnerMissing,
  onResetToNewMapping,
  preferenceRevisionBaselineMismatch,
  preferenceReviewConflicts,
  onReloadMappingAfterPreferenceReview,
  onSaveMapping,
  mappings,
  writingModes,
  onLoadMapping,
  onRequestDeleteMapping,
  onLoadWritingPreferenceDrafts,
  onDecideWritingPreferenceDrafts,
  onRefreshMappingsAfterPreferenceReview,
}: AppMappingEditorProps) {
  const { t } = useI18n();
  const inlineError = (message: string | null) => message
    ? <p role="alert" className="mt-2 break-words text-[13px] leading-5 text-error-ink">{message}</p>
    : null;

  return (
    <>
      <SceneLesson onOpenDictionary={onOpenDictionary} />
      <SettingsGroup title={t("App / 网站映射")} description={t("可组合 App、可执行文件、网站、路径和输入框条件；所有已选条件必须同时匹配。")}>
        {preferenceScopeMigrationNotice && (
          <p role="status" className="px-5 pt-3 text-xs leading-5 text-warning-ink">
            {t("范围已保存；该映射的所有任务偏好和云端批准已清除。本次填写的偏好没有保存；请在当前表单重新选择偏好并再次保存。")}
          </p>
        )}
        <div className="vf-mapping-form space-y-4 px-5 py-4">
          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-0 flex-1 text-xs text-secondary">{t("选择 App（可选）")}
              <Select aria-label={t("选择 App")} value={selectedApplicationId} onValueChange={(value) => { if (value !== selectedApplicationId) onStyleExamplesApprovedChange(false); onSelectApplication(value); }} disabled={applicationsLoading || busy || applications.length === 0} className={`mt-2 w-full ${fieldClass}`}>
                <option value="">{applicationsLoading ? t("正在加载 App…") : applications.length === 0 ? t("没有找到 App") : t("不限定 App")}</option>
                {applications.map((application) => <option key={application.bundle_id} value={application.bundle_id}>{application.label}</option>)}
              </Select>
            </label>
            <button type="button" onClick={onAddApplicationFromDisk} disabled={applicationsLoading || busy} className={secondaryButtonClass}><FolderOpen size={15} aria-hidden="true" />{t("从应用程序中选择")}</button>
            <IconButton size="md" label={t("刷新 App 列表")} icon={<RefreshCw size={16} aria-hidden="true" />} onClick={onRefreshApplications} disabled={applicationsLoading || busy} />
          </div>
          {inlineError(applicationsError)}
          <p className="flex items-center gap-1.5 text-xs text-tertiary"><AppWindowMac size={14} aria-hidden="true" />{t("运行中的 App 会随刷新更新；手动添加的 App 即使未打开也能生效。")} </p>
          <label className="block text-xs text-secondary">{t("语气")}<Select aria-label={t("应用映射语气")} value={selectedMappingMode?.id ?? ""} onValueChange={onSelectedModeIdChange} disabled={busy || savedWritingModes.length === 0} className={`mt-2 w-full ${fieldClass}`}>{savedWritingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}</Select></label>
          <SettingsDisclosure
            variant="plain"
            title={t("任务级写作偏好")}
            description={t("显式指定此映射对应的写作任务；认可的原生 App 可按 bundle ID 绑定，未知 App 与浏览器仍需可信输入框或网站路径。")}
            summary={t(selectedWritingTaskId ? writingTaskOptions.find((option) => option.value === selectedWritingTaskId)?.label ?? "已指定任务" : "未指定任务")}
          >
            <div className="space-y-4">
              <label className="block text-xs text-secondary">{t("写作任务 ID")}
                <Select aria-label={t("写作任务 ID")} value={selectedWritingTaskId} onValueChange={(value) => onSelectWritingTask(value as WritingTaskId | "")} disabled={busy} className={`mt-2 w-full ${fieldClass}`}>
                  <option value="">{t("未知或不限定任务")}</option>
                  {writingTaskOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
                </Select>
                <span className="mt-1 block text-xs text-tertiary">{t("任务 ID 由你明确选择，不从 App、窗口标题或写作语气推断。认可的原生 App 使用已知 bundle ID；未知 App 与浏览器需可信输入框或网站路径。切换任务时会加载对应已保存偏好，并重置云端批准。")}</span>
              </label>
              <p className="text-xs leading-5 text-tertiary">{t("列表只在本机整理且原话有可逐项核对的项目边界时应用。称呼只调整原文已有且边界明确的你好 / 您好；标点只控制无歧义的单句句末句号。不会从观察自动推断。")}</p>
              <div className="grid gap-3 rounded-lg border border-border p-3 sm:grid-cols-2">
                <WritingPreferenceRow
                  dimension="list_structure"
                  label={t("列表结构")}
                  value={writingPreferenceDraft.list_structure.value}
                  status={writingPreferenceDraft.list_structure.status}
                  valueOptions={[{ value: "bullets", label: t("符合来源证据时使用要点") }]}
                  preview={writingPreferenceDraft.list_structure.value ? t("三项任务是：更新 API；修复测试；审阅文档。") : undefined}
                  disabled={busy || !selectedWritingTaskId}
                  onValueChange={(value) => onPreferenceValueChange("list_structure", value)}
                  onStatusChange={(status) => onPreferenceStatusChange("list_structure", status)}
                />
                <WritingPreferenceRow
                  dimension="greeting_tone"
                  label={t("短称呼形式")}
                  value={writingPreferenceDraft.greeting_tone.value}
                  status={writingPreferenceDraft.greeting_tone.status}
                  valueOptions={[
                    { value: "conversational_greeting", label: t("你好（仅替换原有称呼）") },
                    { value: "respectful_greeting", label: t("您好（仅替换原有称呼）") },
                  ]}
                  preview={writingPreferenceDraft.greeting_tone.value === "respectful_greeting"
                    ? t("你好，Alex。 → 您好，Alex。")
                    : writingPreferenceDraft.greeting_tone.value === "conversational_greeting"
                      ? t("您好，Alex。 → 你好，Alex。")
                      : undefined}
                  disabled={busy || !selectedWritingTaskId}
                  onValueChange={(value) => onPreferenceValueChange("greeting_tone", value)}
                  onStatusChange={(status) => onPreferenceStatusChange("greeting_tone", status)}
                />
                <div className="sm:col-span-2">
                  <WritingPreferenceRow
                    dimension="terminal_punctuation"
                    label={t("陈述句句末标点")}
                    value={writingPreferenceDraft.terminal_punctuation.value}
                    status={writingPreferenceDraft.terminal_punctuation.status}
                    valueOptions={[
                      { value: "terminal_period", label: t("保留一个句号") },
                      { value: "no_terminal_period", label: t("聊天句末省略句号") },
                    ]}
                    preview={writingPreferenceDraft.terminal_punctuation.value === "terminal_period"
                      ? t("好的 → 好的。")
                      : writingPreferenceDraft.terminal_punctuation.value === "no_terminal_period"
                        ? t("好的。 → 好的")
                        : undefined}
                    disabled={busy || !selectedWritingTaskId}
                    onValueChange={(value) => onPreferenceValueChange("terminal_punctuation", value)}
                    onStatusChange={(status) => onPreferenceStatusChange("terminal_punctuation", status)}
                  />
                </div>
              </div>
              <p className="text-xs leading-5 text-tertiary">{t("保存、暂停或忘记状态需点击下方“保存映射”或“更新映射”后生效。忘记会保留撤销标记。")}</p>
            </div>
          </SettingsDisclosure>
          <details ref={mappingDetailsRef} className="vf-inline-disclosure border-y border-border py-3 text-xs text-secondary">
            <summary className={`cursor-pointer rounded-md font-medium text-primary ${focusRingClass}`}>
              {t("高级映射选项")}{selectedExecutable || selectedWebsite || browserPathPrefix || focusedField || hasStyleExamples || mappingIntensity !== "inherit" || !mappingCleanupEnabled || !mappingLearnEnabled ? ` · ${t("已设置")}` : ""}
            </summary>
            <p className="mt-2 leading-5 text-tertiary">{t("可选条件必须同时匹配；收起不会清除已设置的选项。")}</p>
            <div className="mt-3 space-y-4">
              <label className="block text-xs text-secondary">
                {t("可执行文件（可选）")}
                <input aria-label={t("可执行文件名")} value={selectedExecutable} onChange={(event) => { if (event.target.value !== selectedExecutable) onStyleExamplesApprovedChange(false); onSelectedExecutableChange(event.target.value); }} disabled={busy} maxLength={255} placeholder={t("例如 Cursor")} className={`mt-2 w-full ${fieldClass}`} />
                <span className="mt-1 block text-xs text-tertiary">{t("可与其他条件组合；不会显示在听写 HUD。")}</span>
              </label>
              <label className="block text-xs text-secondary">
                {t("网站或域名（可选）")}
                <Autocomplete
                  aria-label={t("应用映射网站")}
                  options={websiteOptions.map((website) => ({ value: website.host, label: website.label }))}
                  value={selectedWebsite}
                  onValueChange={(value) => { if (value !== selectedWebsite) onStyleExamplesApprovedChange(false); onSelectedWebsiteChange(value); }}
                  disabled={busy}
                  maxLength={253}
                  placeholder="mail.google.com"
                  className={`mt-2 w-full ${fieldClass}`}
                />
                <span className="mt-1 block text-xs text-tertiary">{t("只输入主机名；不保存完整网址、路径、查询参数或片段。")}</span>
              </label>
              <label className="block text-xs text-secondary">
                {t("网站路径前缀（可选）")}
                <input
                  aria-label={t("网站路径前缀")}
                  value={browserPathPrefix}
                  onChange={(event) => { if (event.target.value !== browserPathPrefix) onStyleExamplesApprovedChange(false); onBrowserPathPrefixChange(event.target.value); }}
                  disabled={busy}
                  maxLength={512}
                  placeholder="/issues"
                  className={`mt-2 w-full ${fieldClass}`}
                />
                <span className="mt-1 block text-xs text-tertiary">{t("路径前缀需以 / 开头，只匹配此路径及其子路径；必须同时填写网站主机名。")}</span>
              </label>
              <label className="block text-xs text-secondary">
                {t("输入框类型（可选）")}
                <Select
                  aria-label={t("输入框类型")}
                  value={focusedField}
                  onValueChange={(value) => { if (value !== focusedField) onStyleExamplesApprovedChange(false); onFocusedFieldChange(value as FocusKind | ""); }}
                  disabled={busy}
                  className={`mt-2 w-full ${fieldClass}`}
                >
                  <option value="">{t("不限定输入框")}</option>
                  {focusFieldOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
                </Select>
                <span className="mt-1 block text-xs text-tertiary">{t("选中的条件会与 App、网站和路径一起全部满足时才匹配。")} </span>
              </label>
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="block text-xs text-secondary">{t("示例输入")}
                  <textarea aria-label={t("App 风格示例输入")} value={styleExampleInput} onChange={(event) => { onStyleExampleInputChange(event.target.value); onStyleExamplesApprovedChange(false); }} maxLength={2_000} rows={3} disabled={busy} placeholder={t("贴一条你平时微信怎么打")} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent ${focusRingClass}`} />
                </label>
                <label className="block text-xs text-secondary">{t("期望输出")}
                  <textarea aria-label={t("App 风格期望输出")} value={styleExampleOutput} onChange={(event) => { onStyleExampleOutputChange(event.target.value); onStyleExamplesApprovedChange(false); }} maxLength={2_000} rows={3} disabled={busy} placeholder={t("希望 VoiceFlow 输出的样子…")} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent ${focusRingClass}`} />
                </label>
              </div>
              <label className="block text-xs text-secondary">{t("整理强度")}
                <Select aria-label={t("整理强度")} value={mappingIntensity} onValueChange={(value) => onMappingIntensityChange(value as "inherit" | "auto" | "off" | "light" | "standard" | "heavy")} disabled={busy} className={`mt-2 w-full ${fieldClass}`}>
                  <option value="inherit">{t("跟随全局")}</option>
                  <option value="auto">{t("自动按场景")}</option>
                  <option value="off">{t("关")}</option>
                  <option value="light">{t("轻")}</option>
                  <option value="standard">{t("中")}</option>
                  <option value="heavy">{t("重")}</option>
                </Select>
              </label>
              <SettingsRow title={t("这个 App 使用 AI 整理")} description={t("关闭后仍会去掉 um / 嗯，但不请求整理服务。")}>
                <Toggle checked={mappingCleanupEnabled} onChange={onMappingCleanupEnabledChange} disabled={busy} label={t("这个 App 使用 AI 整理")} />
              </SettingsRow>
              <SettingsRow title={t("在这个 App 学习词条")} description={t("关闭后不会从该 App 的输入框学习纠正。")}>
                <Toggle checked={mappingLearnEnabled} onChange={onMappingLearnEnabledChange} disabled={busy} label={t("在这个 App 学习词条")} />
              </SettingsRow>
            </div>
          </details>
          <fieldset className="rounded-xl border border-border bg-elevated/30 p-4">
            <legend className="px-1 text-sm font-medium text-primary">{t("上下文隐私预设")}</legend>
            <p className="mb-3 text-xs leading-5 text-secondary">{t("预设只修改当前草稿；点击保存规则后生效。高级权限可以逐项自定义。")}</p>
            <div className="grid gap-2 lg:grid-cols-3">
              {privacyPresetOptions.map((option) => (
                <label
                  key={option.value}
                  className={`flex cursor-pointer items-start gap-2.5 rounded-lg border p-3 transition-colors ${focusRingClass} ${privacyPreset === option.value ? "border-accent/50 bg-accent-soft" : "border-border bg-card hover:bg-elevated/60"}`}
                >
                  <input
                    type="radio"
                    name={`context-privacy-preset-${draftKey}`}
                    value={option.value}
                    checked={privacyPreset === option.value}
                    onChange={() => onApplyPrivacyPreset(option.value)}
                    disabled={busy}
                    className="mt-0.5 h-4 w-4 shrink-0 accent-accent"
                  />
                  <span className="min-w-0">
                    <span className="block text-xs font-medium text-primary">{t(option.label)}</span>
                    <span className="mt-1 block text-[11px] leading-4 text-secondary">{t(option.description)}</span>
                  </span>
                </label>
              ))}
            </div>
            {privacyPreset === "custom" && (
              <p role="status" className="mt-3 text-xs leading-5 text-warning-ink">{t("当前为自定义授权；请在下方高级权限中检查各项来源和发送批准。")}</p>
            )}
          </fieldset>
          <SettingsDisclosure variant="plain" title={t("上下文与样例授权")} description={t("高级数据来源、视觉回退与独立发送授权。保存规则后生效；收起不会撤销授权。") } summary={t("{count} 项已开启").replace("{count}", String(Object.values(sourcePermissions).filter(Boolean).length + Number(styleExamplesApproved) + Number(preferenceGuidanceProviderApproved)))} error={mappingError}>
            <SettingsRow
              title={t("允许结构化偏好发送给云端整理服务商")}
              description={t("云端默认关闭。启用后，只有当前任务已保存的称呼或标点偏好可进入云端整理请求；列表偏好只用于本机整理。不发送样例、候选历史或证据计数。本机路线不需要此授权。此权限独立于风格示例授权。")}
            >
              <Toggle
                checked={preferenceGuidanceProviderApproved}
                onChange={onPreferenceGuidanceProviderApprovedChange}
                disabled={busy || !selectedWritingTaskId || !hasCloudSupportedActivePreference(preferencesFromDraft(writingPreferenceDraft, selectedWritingTaskId))}
                label={t("允许结构化偏好发送给云端整理服务商")}
              />
            </SettingsRow>
            <SettingsRow
              title={t("允许风格示例发送给整理服务商")}
              description={t("启用后，此映射保留的输入、期望输出和已学习示例对可能随整理请求发送给已配置的整理服务商。关闭时仍保留这些示例，但不发送。此权限独立于“允许文字发送给服务商”。")}
            >
              <Toggle
                checked={styleExamplesApproved}
                onChange={onStyleExamplesApprovedChange}
                disabled={busy}
                label={t("允许风格示例发送给整理服务商")}
              />
            </SettingsRow>
            {retainedStylePairs.length > 0 && (
              <details className="rounded-lg border border-border px-3 py-2.5 text-xs text-secondary">
                <summary className="cursor-pointer font-medium text-primary">
                  {t("查看已保存的风格配对")} ({retainedStylePairs.length})
                </summary>
                <div className="mt-3 max-h-72 space-y-3 overflow-y-auto">
                  {retainedStylePairs.map((pair, index) => (
                    <div key={`${index}-${pair.input.slice(0, 16)}`} className="rounded-md bg-elevated/60 p-2.5">
                      <p className="font-medium">{t("示例输入")}</p>
                      <p className="mt-1 whitespace-pre-wrap break-words">{pair.input}</p>
                      <p className="mt-2 font-medium">{t("期望输出")}</p>
                      <p className="mt-1 whitespace-pre-wrap break-words">{pair.output}</p>
                    </div>
                  ))}
                </div>
              </details>
            )}
            {hasStyleExamples && <button type="button" disabled={busy} className={dangerButtonClass} onClick={onRemoveAllStyleExamples}>{t("移除全部样例")}</button>}
            {removeStyleExamples && <p role="status" className="text-xs text-secondary">{t("保存规则后移除样例，并撤销样例发送批准。")}</p>}
            {hasStyleExamples && !styleExamplesApproved && (
              <p className="text-xs leading-5 text-warning-ink">{t("风格示例会保留，但在你单独批准前不会进入整理服务请求。")}</p>
            )}
            {!combined && <ContextLesson onOpenPermissions={onOpenPermissions} />}
            <div className="vf-mapping-permissions">
              <div className="py-3">
                <p className="text-sm font-medium text-primary">{t("自动文字来源权限")}</p>
                <p className="mt-1 max-w-2xl text-xs leading-5 text-tertiary">{t("新规则和缺少来源授权的旧规则默认关闭；迁移时只会按之前明确开启的全局 OCR 选择恢复本机 OCR。全局上下文开关仍会统一关闭自动读取。")}</p>
              </div>
              <SettingsRow title={t("允许读取辅助功能文字")} description={t("允许为此匹配目标自动读取有限的当前输入框和附近文字。")}>
                <Toggle
                  checked={sourcePermissions.ax_text}
                  onChange={(ax_text) => onSourcePermissionsChange((current) => ({ ...current, ax_text }))}
                  disabled={busy}
                  label={t("允许读取辅助功能文字")}
                />
              </SettingsRow>
              <SettingsRow
                title={t("允许本机 OCR")}
                description={t("仅在全局窗口文字识别已开启、辅助功能文字不足时，允许在本机识别当前窗口。")}
              >
                <Toggle
                  checked={sourcePermissions.local_ocr}
                  onChange={(local_ocr) => onSourcePermissionsChange((current) => ({ ...current, local_ocr }))}
                  disabled={busy}
                  label={t("允许本机 OCR")}
                />
              </SettingsRow>
              <SettingsRow
                title={t("允许自动云端视觉")}
                description={t("仅当规则含有具体原生 App 或可执行文件选择器时生效；只在本机证据不足时把同一张当前窗口图发送给已配置的视觉服务商。")}
              >
                <Toggle
                  checked={sourcePermissions.cloud_vision}
                  onChange={(cloud_vision) => onSourcePermissionsChange((current) => ({ ...current, cloud_vision }))}
                  disabled={busy}
                  label={t("允许自动云端视觉")}
                />
              </SettingsRow>
              {invalidCloudVisionGrant && (
                <p className="px-4 pb-3 text-xs leading-5 text-warning-ink">{t("此规则没有具体原生 App 或可执行文件选择器，云端视觉权限不会生效；关闭权限后才能保存。")}</p>
              )}
              <SettingsRow
                title={t("允许文字发送给服务商")}
                description={t("启用后，辅助功能或 OCR 派生的文字可能离开这台 Mac，发送给已配置的转写和整理服务商。关闭后，这些文字不会进入服务商请求。")}
              >
                <Toggle
                  checked={sourcePermissions.context_text_to_providers}
                  onChange={(context_text_to_providers) => onSourcePermissionsChange((current) => ({ ...current, context_text_to_providers }))}
                  disabled={busy}
                  label={t("允许文字发送给服务商")}
                />
              </SettingsRow>
            </div>
            <p className="text-xs text-tertiary">{t("示例不会自动从历史记录学习。")}</p>
          </SettingsDisclosure>
          {selectedExistingMapping && (
            <p className="text-xs text-secondary">
              {t(editingMappingId
                ? "这个目标已有设置；保存后会更新它的语气。"
                : "此目标已有规则；使用编辑按钮查看并保留现有权限。")}
            </p>
          )}
          {editingMappingOwnerMissing && (
            <SettingsAlert onRetry={onResetToNewMapping} retryLabel={t("放弃草稿并新建规则")}>
              {t("当前编辑的映射已删除。放弃此草稿会清空旧规则的偏好和授权；之后请明确选择新目标。")}
            </SettingsAlert>
          )}
          {editingMappingId && preferenceRevisionBaselineMismatch
            && !preferenceReviewConflicts.some((conflict) => conflict.mappingId === editingMappingId)
            && <SettingsAlert onRetry={() => onReloadMappingAfterPreferenceReview(editingMappingId)} retryLabel={t("重新加载获批的 App 规则")}>
              {t("此 App 规则在当前草稿建立后已更新。草稿仍保留；请重新加载最新规则后再保存，避免覆盖已批准偏好。")}
            </SettingsAlert>}
          {editingMappingId && preferenceReviewConflicts
            .filter((conflict) => conflict.mappingId === editingMappingId)
            .map((conflict) => (
              <SettingsAlert key={conflict.candidateId} onRetry={() => onReloadMappingAfterPreferenceReview(conflict.mappingId)} retryLabel={t("重新加载获批的 App 规则")}>
                {t(conflict.reason === "scope_changed"
                  ? "批量复核期间此 App 规则的范围或任务已改变。获批偏好未写入当前表单；草稿仍保留。重新加载最新规则后再继续。"
                  : conflict.reason === "preference_draft_changed"
                    ? "此偏好维度有未保存草稿，获批值未覆盖它。草稿仍保留；请重新加载最新规则以核对获批值后再保存。"
                    : "云端偏好授权有未保存草稿，复核未覆盖它。草稿仍保留；请重新加载最新规则以核对授权状态后再保存。")}
              </SettingsAlert>
            ))}
          {inlineError(mappingError)}
          <button type="button" onClick={onSaveMapping} aria-busy={busyAction === "mapping"} disabled={busy} className={`${buttonClass} min-w-36`}><Plus size={16} aria-hidden="true" />{t(busyAction === "mapping" ? "保存中…" : editingMappingId ? "更新映射" : "保存映射")}</button>
        </div>
        {mappings.length > 0 && (
          <div className="px-5">
            <p className="py-3 text-xs font-medium text-tertiary">{t("已保存的目标规则")}</p>
            {mappings.map((mapping) => (
              <div key={mapping.id} className="flex items-center gap-3 border-t border-border py-3">
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-elevated text-xs font-semibold text-primary">{mapping.label.slice(0, 1)}</span>
                <div className="min-w-0 flex-1">
                  <p className="break-words text-sm font-medium text-primary">{mapping.label} <span className="font-normal text-secondary">→ {t(writingModes.find((mode) => mode.id === mapping.mode_id)?.label ?? familyLabels[mapping.family])}{mapping.writing_task_id ? ` · ${t(writingTaskOptions.find((option) => option.value === mapping.writing_task_id)?.label ?? "已指定任务")}` : ""}</span></p>
                  <p className="mt-1 break-words text-xs leading-[18px] text-secondary">
                    {matchingSelectors(mapping, applications, t).length > 0
                      ? matchingSelectors(mapping, applications, t).join(" · ")
                      : t("按 App 匹配")}
                  </p>
                </div>
                <IconButton size="sm" label={t("编辑映射")} icon={<Pencil size={15} aria-hidden="true" />} onClick={() => onLoadMapping(mapping)} disabled={busy} />
                <IconButton size="sm" label={t("删除映射")} aria-label={`${t("删除应用映射")} ${mapping.label}`} tone="danger" icon={<Trash2 size={15} aria-hidden="true" />} onClick={() => onRequestDeleteMapping(mapping)} disabled={busy} />
              </div>
            ))}
          </div>
        )}
      </SettingsGroup>
      <WritingPreferenceReview
        onLoadCandidates={onLoadWritingPreferenceDrafts}
        onDecideCandidates={onDecideWritingPreferenceDrafts}
        onRefreshApprovedMappings={onRefreshMappingsAfterPreferenceReview}
      />
    </>
  );
}
