import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { AppWindowMac, FolderOpen, Globe, Plus, RefreshCw, ScanSearch, Trash2 } from "lucide-react";
import { colors, radius, buttonClass, secondaryButtonClass } from "../lib/theme";
import { useI18n } from "../lib/i18n";
import { IconButton } from "./IconButton";
import { Toggle } from "./Toggle";
import { ConfirmDialog } from "./ConfirmDialog";
import { SettingsAlert, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "./SettingsLayout";

export type ContextFamily =
  | "email"
  | "browser_search"
  | "work_chat"
  | "personal_chat"
  | "document"
  | "project_management"
  | "calendar_task"
  | "developer_collaboration"
  | "prompt_or_code"
  | "terminal"
  | "form_filling"
  | "notes_journaling"
  | "social_media"
  | "customer_support"
  | "general";

type ContextSnapshot = {
  profile: { id: string; family: ContextFamily; writing_mode_id?: string | null; app_label: string; icon_key: string; source: string; confidence: number };
  browser_access_status: "not_applicable" | "disabled" | "needs_permission" | "granted" | "unavailable";
};

type ContextMapping = {
  id: string;
  label: string;
  family: ContextFamily;
  mode_id?: string | null;
  bundle_id?: string | null;
  executable?: string | null;
  browser_host?: string | null;
  style_example_input?: string | null;
  style_example_output?: string | null;
  enabled: boolean;
};

type ApplicationOption = {
  bundle_id: string;
  label: string;
};

export type WritingMode = {
  id: string;
  label: string;
  family: ContextFamily;
  prompt: string;
  builtin: boolean;
};

function mergeApplicationOptions(...groups: ApplicationOption[][]): ApplicationOption[] {
  const options = new Map<string, ApplicationOption>();
  for (const group of groups) {
    for (const application of group) {
      if (!options.has(application.bundle_id)) options.set(application.bundle_id, application);
    }
  }
  return [...options.values()].sort((left, right) => left.label.localeCompare(right.label));
}

function applicationsFromMappings(mappings: ContextMapping[]): ApplicationOption[] {
  return mappings.flatMap((mapping) => mapping.bundle_id ? [{ bundle_id: mapping.bundle_id, label: mapping.label }] : []);
}

const familyLabels: Record<ContextFamily, string> = {
  email: "邮件",
  browser_search: "浏览器搜索 / 研究",
  work_chat: "工作聊天",
  personal_chat: "个人聊天",
  document: "文档",
  project_management: "项目管理",
  calendar_task: "日历 / 任务",
  developer_collaboration: "开发协作",
  prompt_or_code: "Prompt / 代码",
  terminal: "终端 / 命令行",
  form_filling: "填写表单",
  notes_journaling: "笔记 / 日记",
  social_media: "社交媒体",
  customer_support: "客户支持",
  general: "通用",
};

const outputModeOptions = [
  { value: "auto", label: "自动整理" },
  { value: "email", label: "邮件" },
  { value: "bullets", label: "要点 / 步骤" },
  { value: "meeting_notes", label: "会议记录" },
  { value: "code", label: "代码 / 终端" },
  { value: "translation", label: "翻译" },
] as const;

const translationLanguageOptions = [
  { value: "en", label: "English" },
  { value: "zh", label: "中文" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "es", label: "Español" },
  { value: "fr", label: "Français" },
  { value: "de", label: "Deutsch" },
] as const;

const defaultWritingModes: WritingMode[] = Object.entries(familyLabels).map(([id, label]) => ({
  id,
  label,
  family: id as ContextFamily,
  prompt: "Use the smallest useful cleanup and preserve the original structure.",
  builtin: true,
}));

const ADD_CUSTOM_MODE_OPTION = "__add_custom_mode__";

function familyModeId(family: ContextFamily): string {
  return family;
}

const browserBundleIds = new Set([
  "com.google.Chrome",
  "com.google.Chrome.canary",
  "com.apple.Safari",
  "company.thebrowser.Browser",
  "com.brave.Browser",
  "com.microsoft.edgemac",
  "org.mozilla.firefox",
  "com.vivaldi.Vivaldi",
  "com.operasoftware.Opera",
  "com.kagi.kagimacOS",
]);

const websiteOptions = [
  { host: "mail.google.com", label: "Gmail" },
  { host: "outlook.office.com", label: "Outlook" },
  { host: "app.slack.com", label: "Slack" },
  { host: "teams.microsoft.com", label: "Teams" },
  { host: "notion.so", label: "Notion" },
  { host: "docs.google.com", label: "Google Docs" },
  { host: "github.com", label: "GitHub" },
  { host: "linear.app", label: "Linear" },
] as const;

const websiteLabels = Object.fromEntries(websiteOptions.map((option) => [option.host, option.label])) as Record<string, string>;

export function ContextSettings({
  writingModes: managedWritingModes = defaultWritingModes,
  onWritingModesChange,
  automationOnly = false,
  combined = false,
  outputMode,
  translationTargetLanguage,
  onOutputModeChange,
  onTranslationTargetLanguageChange,
}: {
  writingModes?: WritingMode[];
  onWritingModesChange?: (writingModes: WritingMode[]) => void;
  automationOnly?: boolean;
  combined?: boolean;
  outputMode?: string;
  translationTargetLanguage?: string;
  onOutputModeChange?: (outputMode: string) => void;
  onTranslationTargetLanguageChange?: (language: string) => void;
}) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<ContextSnapshot | null>(null);
  const [mappings, setMappings] = useState<ContextMapping[]>([]);
  const [applications, setApplications] = useState<ApplicationOption[]>([]);
  const [selectedApplicationId, setSelectedApplicationId] = useState("");
  const [selectedWebsite, setSelectedWebsite] = useState("");
  const [selectedModeId, setSelectedModeId] = useState("general");
  const [styleExampleInput, setStyleExampleInput] = useState("");
  const [styleExampleOutput, setStyleExampleOutput] = useState("");
  const [writingModes, setWritingModes] = useState<WritingMode[]>(managedWritingModes);
  const [draftModeId, setDraftModeId] = useState<string | null>(null);
  const [editingModeId, setEditingModeId] = useState("general");
  const [modeLabelDraft, setModeLabelDraft] = useState("");
  const [modePromptDraft, setModePromptDraft] = useState("");
  const [applicationsLoading, setApplicationsLoading] = useState(true);
  const [enabled, setEnabled] = useState(true);
  const [overrideFamily, setOverrideFamily] = useState<ContextFamily | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pendingConfirm, setPendingConfirm] = useState<{
    title: string;
    description: string;
    confirmLabel: string;
    action: () => void;
  } | null>(null);

  useEffect(() => {
    setWritingModes(managedWritingModes);
    setDraftModeId(null);
  }, [managedWritingModes]);

  const savedWritingModes = draftModeId ? writingModes.filter((mode) => mode.id !== draftModeId) : writingModes;
  const selectedMappingMode = savedWritingModes.find((mode) => mode.id === selectedModeId) ?? savedWritingModes[0];
  const editingMode = writingModes.find((mode) => mode.id === editingModeId) ?? writingModes[0];
  const editingModeLabel = editingMode?.builtin ? editingMode.label : modeLabelDraft;
  const modeIsDirty = Boolean(editingMode && (editingModeLabel !== editingMode.label || modePromptDraft !== editingMode.prompt));

  useEffect(() => {
    if (!editingMode) return;
    setEditingModeId(editingMode.id);
    setModeLabelDraft(editingMode.label);
    setModePromptDraft(editingMode.prompt);
  }, [editingMode]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      invoke<ContextSnapshot>("get_context_snapshot"),
      invoke<ContextMapping[]>("get_context_mappings"),
      invoke<{ context_enabled: boolean }>("get_settings"),
      invoke<ContextFamily | null>("get_context_override"),
    ])
      .then(([nextSnapshot, nextMappings, settings, nextOverride]) => {
        if (cancelled) return;
        setSnapshot(nextSnapshot);
        setMappings(nextMappings);
        setApplications((current) => mergeApplicationOptions(current, applicationsFromMappings(nextMappings)));
        setEnabled(settings.context_enabled);
        setOverrideFamily(nextOverride);
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)));

    void invoke<ApplicationOption[]>("get_available_applications")
      .then((nextApplications) => {
        if (!cancelled) setApplications((current) => mergeApplicationOptions(nextApplications, current));
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (!cancelled) setApplicationsLoading(false);
      });

    let cancelledListener = false;
    let unlisten: (() => void) | undefined;
    void listen<ContextSnapshot>("context://changed", (event) => setSnapshot(event.payload))
      .then((cleanup) => {
        if (cancelledListener) {
          cleanup();
        } else {
          unlisten = cleanup;
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      cancelled = true;
      cancelledListener = true;
      unlisten?.();
    };
  }, []);

  const refreshApplications = async () => {
    setApplicationsLoading(true);
    setError(null);
    try {
      const nextApplications = await invoke<ApplicationOption[]>("get_available_applications");
      setApplications((current) => mergeApplicationOptions(nextApplications, current));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setApplicationsLoading(false);
    }
  };

  const addApplicationFromDisk = async () => {
    setBusy(true);
    setError(null);
    try {
      const selectedPath = await open({
        title: t("选择应用程序"),
        defaultPath: "/Applications",
        multiple: false,
        directory: false,
        filters: [{ name: t("应用程序"), extensions: ["app"] }],
      });
      if (typeof selectedPath !== "string") return;

      const application = await invoke<ApplicationOption>("get_application_from_path", { path: selectedPath });
      setApplications((current) => mergeApplicationOptions([application], current));
      selectApplication(application.bundle_id);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const publishWritingModes = (nextWritingModes: WritingMode[]) => {
    setWritingModes(nextWritingModes);
    onWritingModesChange?.(nextWritingModes);
  };

  const applySelectEditingMode = (modeId: string) => {
    const nextWritingModes = draftModeId && draftModeId !== modeId
      ? writingModes.filter((mode) => mode.id !== draftModeId)
      : writingModes;
    if (nextWritingModes.length !== writingModes.length) {
      setWritingModes(nextWritingModes);
      setDraftModeId(null);
    }
    const mode = nextWritingModes.find((item) => item.id === modeId);
    if (!mode) return;
    setEditingModeId(mode.id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
  };

  const selectEditingMode = (modeId: string) => {
    if (modeIsDirty) {
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前模式有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: () => applySelectEditingMode(modeId),
      });
      return;
    }
    applySelectEditingMode(modeId);
  };

  const applyAddCustomMode = () => {
    const id = `custom.${Date.now()}.${Math.random().toString(36).slice(2, 8)}`;
    const mode: WritingMode = {
      id,
      label: t("自定义模式"),
      family: "general",
      prompt: t("根据我说的内容整理文字，保留事实、语气和具体信息，不要添加我没有说过的内容。"),
      builtin: false,
    };
    const baseWritingModes = draftModeId ? writingModes.filter((item) => item.id !== draftModeId) : writingModes;
    const next = [...baseWritingModes, mode];
    setWritingModes(next);
    setDraftModeId(id);
    setEditingModeId(id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
  };

  const addCustomMode = () => {
    if (modeIsDirty) {
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前模式有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: applyAddCustomMode,
      });
      return;
    }
    applyAddCustomMode();
  };

  const saveEditingMode = () => {
    if (!editingMode) return;
    const label = editingMode.builtin ? editingMode.label : modeLabelDraft.trim();
    const prompt = modePromptDraft.trim();
    if (!label) {
      setError(t("请填写模式名称"));
      return;
    }
    if (!prompt) {
      setError(t("请填写 Prompt"));
      return;
    }
    publishWritingModes(writingModes.map((mode) => mode.id === editingMode.id
      ? { ...mode, label, prompt }
      : mode));
    setDraftModeId(null);
    setModeLabelDraft(label);
    setModePromptDraft(prompt);
    setError(null);
  };

  const deleteEditingMode = () => {
    if (!editingMode || editingMode.builtin) return;
    if (mappings.some((mapping) => mapping.mode_id === editingMode.id)) {
      setError(t("请先删除使用这个模式的 App 映射"));
      return;
    }
    setPendingConfirm({
      title: t("删除自定义模式"),
      description: t("确定删除“{name}”吗？").replace("{name}", editingMode.label),
      confirmLabel: t("删除"),
      action: () => {
        const next = writingModes.filter((mode) => mode.id !== editingMode.id);
        if (editingMode.id === draftModeId) {
          setWritingModes(next);
          setDraftModeId(null);
        } else {
          publishWritingModes(next);
        }
        const fallback = next[0];
        if (fallback) {
          setEditingModeId(fallback.id);
          setModeLabelDraft(fallback.label);
          setModePromptDraft(fallback.prompt);
        }
        if (selectedModeId === editingMode.id) setSelectedModeId("general");
        setError(null);
      },
    });
  };

  const selectApplication = (bundleId: string) => {
    setSelectedApplicationId(bundleId);
    setSelectedWebsite("");
    const existing = mappings.find((mapping) => mapping.bundle_id === bundleId);
    setSelectedModeId(existing?.mode_id ?? familyModeId(existing?.family ?? "general"));
    setStyleExampleInput(existing?.style_example_input ?? "");
    setStyleExampleOutput(existing?.style_example_output ?? "");
    setError(null);
  };

  const selectWebsite = (host: string) => {
    setSelectedWebsite(host);
    const existing = mappings.find((mapping) => mapping.browser_host === host);
    if (existing) setSelectedModeId(existing.mode_id ?? familyModeId(existing.family));
    setStyleExampleInput(existing?.style_example_input ?? "");
    setStyleExampleOutput(existing?.style_example_output ?? "");
    setError(null);
  };

  const toggle = async (nextEnabled: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_context_enabled", { enabled: nextEnabled });
      setEnabled(nextEnabled);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const requestBrowserAccess = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("request_browser_access");
      const next = await invoke<ContextSnapshot>("get_context_snapshot");
      setSnapshot(next);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const setOverride = async (value: string) => {
    const family = value === "auto" ? null : value as ContextFamily;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<ContextSnapshot>("set_context_override", { family });
      setSnapshot(next);
      setOverrideFamily(family);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const saveMapping = async () => {
    const application = applications.find((item) => item.bundle_id === selectedApplicationId);
    const mode = savedWritingModes.find((item) => item.id === selectedModeId);
    if (!application) {
      setError(t("请先选择一个 App"));
      return;
    }
    if (!mode) {
      setError(t("请先选择一个写作模式"));
      return;
    }
    const browserHost = selectedWebsite || null;
    const existing = mappings.find((mapping) => browserHost
      ? mapping.browser_host === browserHost
      : mapping.bundle_id === application.bundle_id && !mapping.browser_host);
    const mappingId = existing?.id ?? (browserHost ? `${application.bundle_id}:${browserHost}` : application.bundle_id);
    const mappingLabel = browserHost ? `${application.label} · ${websiteLabels[browserHost] ?? browserHost}` : application.label;
    setBusy(true);
    setError(null);
    try {
      const mapping: ContextMapping = {
          id: mappingId,
          label: mappingLabel,
          family: mode.family,
          mode_id: mode.id,
          bundle_id: browserHost ? null : application.bundle_id,
          executable: null,
          browser_host: browserHost,
          enabled: true,
        };
      const styleInput = styleExampleInput.trim();
      const styleOutput = styleExampleOutput.trim();
      if (styleInput) mapping.style_example_input = styleInput;
      if (styleOutput) mapping.style_example_output = styleOutput;
      const next = await invoke<ContextMapping[]>("save_context_mapping", { mapping });
      setMappings(next);
      setSelectedApplicationId("");
      setSelectedWebsite("");
      setSelectedModeId("general");
      setStyleExampleInput("");
      setStyleExampleOutput("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const deleteMapping = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      setMappings(await invoke<ContextMapping[]>("delete_context_mapping", { id }));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const selectedApplication = applications.find((application) => application.bundle_id === selectedApplicationId);
  const selectedApplicationIsBrowser = selectedApplication ? browserBundleIds.has(selectedApplication.bundle_id) : false;
  const selectedExistingMapping = mappings.find((mapping) => selectedWebsite
    ? mapping.browser_host === selectedWebsite
    : mapping.bundle_id === selectedApplicationId && !mapping.browser_host);
  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t(automationOnly || combined ? "智能整理" : "写作模式")}
        description={t(combined ? "根据当前 App、输入框和你说的内容整理文字，也可以编辑 Prompt 和 App / 网站规则。" : automationOnly ? "根据当前 App、输入框和你说的内容，自动选择合适的 Prompt 和输出格式。" : "编辑写作 Prompt，并为 App 或网站配置固定模式。")}
      />
      {(automationOnly || combined) && (
        <>
          <SettingsGroup title={t("App 上下文")} description={t("决定 VoiceFlow 是否根据当前 App 和输入框自动适配。")}>
            <SettingsRow title={t("App 上下文适配")} description={t("根据你当前使用的 App 和输入框自动选择合适的 Prompt。")} icon={<ScanSearch size={17} strokeWidth={1.7} aria-hidden="true" />}>
              <Toggle checked={enabled} onChange={(nextEnabled) => void toggle(nextEnabled)} disabled={busy} label={t("App 上下文适配")} />
            </SettingsRow>
            {snapshot && (
              <SettingsRow
                title={t("当前上下文")}
                description={`${snapshot.profile.app_label} · ${t(familyLabels[snapshot.profile.family])}`}
              />
            )}
            <SettingsRow title={t("临时覆盖")} description={t("只对当前运行有效，重启或切换 App 后会恢复自动适配。")}>
              <select id="context-temporary-override" aria-label={t("临时覆盖")} value={overrideFamily ?? "auto"} onChange={(event) => void setOverride(event.target.value)} disabled={busy} className={`w-52 max-w-full ${fieldClass}`}>
                <option value="auto">{t("自动适配")}</option>
                {Object.entries(familyLabels).map(([value, label]) => <option key={value} value={value}>{t(label)}</option>)}
              </select>
            </SettingsRow>
            {snapshot?.browser_access_status !== "not_applicable" && snapshot?.browser_access_status !== "granted" && (
              <SettingsRow title={t("浏览器站点检测")} description={t("Chrome / Safari 需要额外授权，VoiceFlow 只保存站点标识。")} icon={<Globe size={17} className="text-warning" aria-hidden="true" />}>
                <button type="button" onClick={() => void requestBrowserAccess()} disabled={busy} className={secondaryButtonClass}>{t("启用访问")}</button>
              </SettingsRow>
            )}
          </SettingsGroup>
          <SettingsGroup title={t("输出格式")} description={t("自动整理会综合 App 上下文和你说的内容；手动模式只影响当前录音。")}>
            <SettingsRow title={t("输出模式")} description={t("自动根据当前 App、输入框和你说的内容选择整理方式；手动模式会覆盖自动判断。")}>
              <select aria-label={t("输出模式")} value={outputMode ?? "auto"} onChange={(event) => onOutputModeChange?.(event.target.value)} className={`w-40 max-w-full ${fieldClass}`}>
                {outputModeOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
              </select>
            </SettingsRow>
            {outputMode === "translation" && <SettingsRow title={t("目标语言")} description={t("只控制输出语言；语音识别仍然自动检测。")}>
              <select aria-label={t("翻译目标语言")} value={translationTargetLanguage ?? "en"} onChange={(event) => onTranslationTargetLanguageChange?.(event.target.value)} className={`w-40 max-w-full ${fieldClass}`}>
                {translationLanguageOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
              </select>
            </SettingsRow>}
          </SettingsGroup>
        </>
      )}

      {(!automationOnly || combined) && <>
      <SettingsGroup title={t("编辑 Prompt")} description={t("编辑 Prompt，或创建只属于你的模式。")}> 
        <div className="px-4 py-4 sm:px-5">
          <p className="max-w-xl text-xs leading-5 text-tertiary">{t("Prompt 只作为写作指导，不会覆盖 VoiceFlow 的事实保护规则。")} </p>
        </div>
        <div className="border-t border-border px-4 py-4 sm:px-5">
          <label className="block text-xs text-secondary">
            {t("编辑模式")}
            <select aria-label={t("编辑写作模式")} value={editingMode?.id ?? ""} onChange={(event) => event.target.value === ADD_CUSTOM_MODE_OPTION ? addCustomMode() : selectEditingMode(event.target.value)} disabled={busy || !editingMode} className={`mt-1 w-full ${fieldClass}`}>
              {writingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}
              <option value={ADD_CUSTOM_MODE_OPTION}>＋ {t("添加自定义模式")}</option>
            </select>
          </label>
          {!editingMode?.builtin && <label className="mt-4 block text-xs text-secondary">{t("模式名称")}<input aria-label={t("自定义模式名称")} value={modeLabelDraft} onChange={(event) => setModeLabelDraft(event.target.value)} disabled={busy} maxLength={64} className={`mt-1 w-full ${fieldClass}`} /></label>}
          <label className="mt-4 block text-xs text-secondary">
            {t("Prompt")}
            <textarea aria-label={t("写作模式 Prompt")} value={modePromptDraft} onChange={(event) => setModePromptDraft(event.target.value)} disabled={busy} maxLength={8_000} rows={6} className={`mt-1 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2.5 text-sm leading-6 outline-none transition-[border-color,box-shadow] focus:border-accent focus:ring-2 focus:ring-accent/10`} />
            <span className="mt-1 block text-xs text-tertiary">{t("可以用中文或英文描述希望保留什么、如何组织，以及明确禁止添加什么。")} </span>
          </label>
          <div className="mt-4 rounded-lg bg-elevated/60 px-3 py-2.5 text-xs text-secondary">
            <p className="font-medium text-primary">{t("预览")}{modeIsDirty ? ` · ${t("未保存")}` : ""}</p>
            <p className="mt-1 leading-5">{t("去掉口头禅和重复，保留事实与技术词；语气、标点和结构会按当前模式处理。")}</p>
          </div>
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <button type="button" onClick={saveEditingMode} disabled={busy || !editingMode} className={buttonClass}>{t("保存模式")}</button>
            {!editingMode?.builtin && <button type="button" onClick={deleteEditingMode} disabled={busy} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{t("删除自定义模式")}</button>}
          </div>
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("App / 网站映射")} description={t("为某个 App 选择固定写作模式；也可以细分到浏览器网站。")}>
        <div className="space-y-4 px-4 py-4 sm:px-5">
          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-0 flex-1 text-xs text-secondary">{t("选择 App")}
              <select aria-label={t("选择 App")} value={selectedApplicationId} onChange={(event) => selectApplication(event.target.value)} disabled={applicationsLoading || busy || applications.length === 0} className={`mt-1 w-full ${fieldClass}`}>
                <option value="">{applicationsLoading ? t("正在加载 App…") : applications.length === 0 ? t("没有找到 App") : t("选择一个 App…")}</option>
                {applications.map((application) => <option key={application.bundle_id} value={application.bundle_id}>{application.label}</option>)}
              </select>
            </label>
            <button type="button" onClick={() => void addApplicationFromDisk()} disabled={applicationsLoading || busy} className={secondaryButtonClass}><FolderOpen size={15} aria-hidden="true" />{t("从应用程序中选择")}</button>
            <IconButton size="md" label={t("刷新 App 列表")} icon={<RefreshCw size={16} aria-hidden="true" />} onClick={() => void refreshApplications()} disabled={applicationsLoading || busy} />
          </div>
          <p className="flex items-center gap-1.5 text-xs text-tertiary"><AppWindowMac size={14} aria-hidden="true" />{t("运行中的 App 会随刷新更新；手动添加的 App 即使未打开也能生效。")} </p>
          {selectedApplicationIsBrowser && <label className="block text-xs text-secondary">{t("网站（可选）")}<select aria-label={t("应用映射网站")} value={selectedWebsite} onChange={(event) => selectWebsite(event.target.value)} disabled={busy} className={`mt-1 w-full ${fieldClass}`}><option value="">{t("整个浏览器")}</option>{websiteOptions.map((website) => <option key={website.host} value={website.host}>{t(website.label)}</option>)}</select><span className="mt-1 block text-xs text-tertiary">{t("选择网站后，只在这个网站使用此模式；不选择则对整个浏览器生效。")} </span></label>}
          <label className="block text-xs text-secondary">{t("写作模式")}<select aria-label={t("应用映射写作模式")} value={selectedMappingMode?.id ?? ""} onChange={(event) => setSelectedModeId(event.target.value)} disabled={busy || savedWritingModes.length === 0} className={`mt-1 w-full ${fieldClass}`}>{savedWritingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}</select></label>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block text-xs text-secondary">{t("示例输入")}
              <textarea aria-label={t("App 风格示例输入")} value={styleExampleInput} onChange={(event) => setStyleExampleInput(event.target.value)} maxLength={2_000} rows={3} disabled={busy || !selectedApplicationId} placeholder={t("说一句典型的话…")} className={`mt-1 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent`} />
            </label>
            <label className="block text-xs text-secondary">{t("期望输出")}
              <textarea aria-label={t("App 风格期望输出")} value={styleExampleOutput} onChange={(event) => setStyleExampleOutput(event.target.value)} maxLength={2_000} rows={3} disabled={busy || !selectedApplicationId} placeholder={t("希望 VoiceFlow 输出的样子…")} className={`mt-1 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent`} />
            </label>
          </div>
          <p className="text-xs text-tertiary">{t("保存后只作为这个 App 的本地整理参考，不会自动从历史记录学习。")} {t("确认后的 App 风格样例会发送给当前配置的 LLM 服务。")} </p>
          {selectedExistingMapping && <p className="text-xs text-secondary">{t("这个目标已有设置；保存后会更新它的写作模式。")} </p>}
          <button type="button" onClick={() => void saveMapping()} disabled={busy || applicationsLoading || !selectedApplicationId} className={buttonClass}><Plus size={16} aria-hidden="true" />{t("保存 App 设置")}</button>
        </div>
        {mappings.length > 0 && <div className="border-t border-border px-4 sm:px-5"><p className="py-3 text-xs font-medium text-tertiary">{t("已保存的 App 设置")}</p>{mappings.map((mapping) => <div key={mapping.id} className="flex items-center gap-3 border-t border-border py-3"><span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-elevated text-xs font-semibold text-primary">{mapping.label.slice(0, 1)}</span><div className="min-w-0 flex-1"><p className="truncate text-sm font-medium text-primary">{mapping.label}</p><p className="mt-0.5 truncate text-xs text-tertiary">{t(writingModes.find((mode) => mode.id === mapping.mode_id)?.label ?? familyLabels[mapping.family])} · {mapping.browser_host ? t("按网站匹配") : t("按 App 匹配")}</p></div><IconButton size="sm" label={t("删除映射")} aria-label={`${t("删除应用映射")} ${mapping.label}`} tone="danger" icon={<Trash2 size={15} aria-hidden="true" />} onClick={() => setPendingConfirm({ title: t("删除映射"), description: t("确定删除“{name}”吗？").replace("{name}", mapping.label), confirmLabel: t("删除"), action: () => void deleteMapping(mapping.id) })} disabled={busy} /></div>)}</div>}
      </SettingsGroup>
      </>}
      {error && <SettingsAlert>{error}</SettingsAlert>}
      <ConfirmDialog
        open={pendingConfirm != null}
        title={pendingConfirm?.title ?? ""}
        description={pendingConfirm?.description ?? ""}
        confirmLabel={pendingConfirm?.confirmLabel ?? t("确定")}
        cancelLabel={t("取消")}
        onCancel={() => setPendingConfirm(null)}
        onConfirm={() => {
          const action = pendingConfirm?.action;
          setPendingConfirm(null);
          action?.();
        }}
      />
    </SettingsShell>
  );
}

const fieldClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-[border-color,box-shadow] focus:border-accent focus:ring-2 focus:ring-accent/10`;
