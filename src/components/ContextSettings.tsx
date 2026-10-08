import { Autocomplete } from "./Autocomplete";
import { Select } from "./Select";
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { AppWindowMac, FolderOpen, Globe, Pencil, Plus, RefreshCw, ScanSearch, Trash2 } from "lucide-react";
import { colors, buttonClass, secondaryButtonClass, compactButtonClass, dangerButtonClass, inputClass, focusRingClass } from "../lib/theme";
import { useI18n } from "../lib/i18n";
import { IconButton } from "./IconButton";
import { Toggle } from "./Toggle";
import { ConfirmDialog } from "./ConfirmDialog";
import { WritingPreview } from "./WritingPreview";
import { SettingsAlert, SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, type SettingsSaveFailure } from "./SettingsLayout";
import { translationLanguageOptions } from "../lib/translationLanguages";
import type { ProviderId } from "../lib/providers";

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

export type FocusKind =
  | "chat"
  | "code"
  | "terminal"
  | "email"
  | "document"
  | "form"
  | "search"
  | "editable"
  | "secure"
  | "unknown"
  | "coding_prompt";

export type ContextSourcePermissions = {
  ax_text: boolean;
  local_ocr: boolean;
  cloud_vision: boolean;
  context_text_to_providers: boolean;
};

type ContextSnapshot = {
  profile: { id: string; family: ContextFamily; writing_mode_id?: string | null; app_label: string; icon_key: string; source: string; confidence: number };
  policy?: { input_kind?: FocusKind };
  browser_access_status: "not_applicable" | "disabled" | "needs_permission" | "granted" | "unavailable";
};

export type ContextMapping = {
  id: string;
  label: string;
  family: ContextFamily;
  mode_id?: string | null;
  bundle_id?: string | null;
  executable?: string | null;
  browser_host?: string | null;
  browser_path_prefix?: string | null;
  focused_field?: FocusKind | null;
  source_permissions?: Partial<ContextSourcePermissions> | null;
  style_example_input?: string | null;
  style_example_output?: string | null;
  style_example_pairs?: { input: string; output: string }[];
  style_examples_approved?: boolean;
  enabled: boolean;
  cleanup_effort?: "light" | "standard" | null;
  cleanup_intensity?: "auto" | "off" | "light" | "standard" | "heavy" | null;
  cleanup_enabled?: boolean;
  dictionary_learn_enabled?: boolean;
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

function sameWritingMode(left: WritingMode, right: WritingMode): boolean {
  return left.id === right.id && left.label === right.label && left.family === right.family
    && left.prompt === right.prompt && left.builtin === right.builtin;
}

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

function mappingIntensityFrom(
  mapping?: ContextMapping,
): "inherit" | "auto" | "off" | "light" | "standard" | "heavy" {
  if (mapping?.cleanup_intensity) return mapping.cleanup_intensity;
  if (mapping?.cleanup_effort === "light" || mapping?.cleanup_effort === "standard") {
    return mapping.cleanup_effort;
  }
  return "inherit";
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

const focusFieldOptions: { value: FocusKind; label: string }[] = [
  { value: "chat", label: "聊天输入框" },
  { value: "code", label: "代码编辑器" },
  { value: "terminal", label: "终端输入框" },
  { value: "email", label: "邮件输入框" },
  { value: "document", label: "文档编辑器" },
  { value: "form", label: "表单输入框" },
  { value: "search", label: "搜索框" },
  { value: "editable", label: "其他可编辑输入框" },
  { value: "secure", label: "安全输入框" },
  { value: "unknown", label: "未知输入框" },
  { value: "coding_prompt", label: "代码提示输入框" },
];

function sourcePermissionValue(
  permissions: ContextMapping["source_permissions"],
  key: keyof ContextSourcePermissions,
): boolean {
  return permissions?.[key] === true;
}

function sourceLabel(source: string, translate: (source: string) => string): string {
  const labels: Record<string, string> = {
    user_mapping: "用户映射",
    browser_domain: "浏览器域名",
    native_process: "原生应用",
    window_title: "窗口标题",
    focused_input: "输入框",
    manual_override: "手动覆盖",
    fallback: "自动回退",
  };
  return translate(labels[source] ?? "未知来源");
}

function focusFieldLabel(kind: FocusKind, translate: (source: string) => string): string {
  return translate(focusFieldOptions.find((option) => option.value === kind)?.label ?? "未知输入框");
}

function matchingSelectors(
  mapping: ContextMapping,
  applications: ApplicationOption[],
  translate: (source: string) => string,
): string[] {
  const selectors: string[] = [];
  if (mapping.bundle_id) {
    const app = applications.find((option) => option.bundle_id === mapping.bundle_id);
    selectors.push(`${translate("App")}: ${app?.label ?? mapping.label}`);
  }
  if (mapping.executable) selectors.push(`${translate("可执行文件")}: ${mapping.executable}`);
  if (mapping.browser_host) {
    selectors.push(`${translate("网站")}: ${translate(websiteLabels[mapping.browser_host] ?? mapping.browser_host)}`);
  }
  if (mapping.browser_path_prefix) selectors.push(`${translate("路径前缀")}: ${mapping.browser_path_prefix}`);
  if (mapping.focused_field) selectors.push(`${translate("输入框")}: ${focusFieldLabel(mapping.focused_field, translate)}`);
  return selectors;
}

function normalizeBrowserHost(value: string): string | null {
  const candidate = value.trim();
  if (!candidate) return null;
  // The rule stores only a host. Reject complete URLs (which can carry paths,
  // credentials, queries, or fragments) instead of accidentally persisting one.
  if (/[/:?#@\s]/.test(candidate)) return null;
  try {
    const host = new URL(`https://${candidate}`).hostname.toLowerCase().replace(/\.$/, "");
    const labels = host.split(".");
    return host && host.length <= 253 && labels.every((label) => label.length > 0 && label.length <= 63)
      ? host
      : null;
  } catch {
    return null;
  }
}

function selectorsEqual(left: ContextMapping, right: Pick<ContextMapping, "bundle_id" | "executable" | "browser_host" | "browser_path_prefix" | "focused_field">): boolean {
  return (left.bundle_id ?? null) === (right.bundle_id ?? null)
    && (left.executable?.trim() || null) === (right.executable?.trim() || null)
    && (left.browser_host?.toLowerCase() || null) === (right.browser_host?.toLowerCase() || null)
    && (left.browser_path_prefix?.trim() || null) === (right.browser_path_prefix?.trim() || null)
    && (left.focused_field ?? null) === (right.focused_field ?? null);
}

function newMappingId(selectors: Pick<ContextMapping, "bundle_id" | "executable" | "browser_host" | "browser_path_prefix" | "focused_field">): string {
  if (selectors.bundle_id && !selectors.executable && !selectors.browser_host && !selectors.browser_path_prefix && !selectors.focused_field) {
    return selectors.bundle_id;
  }
  if (selectors.bundle_id && selectors.browser_host && !selectors.executable && !selectors.browser_path_prefix && !selectors.focused_field) {
    return `${selectors.bundle_id}:${selectors.browser_host}`;
  }
  if (!selectors.bundle_id && !selectors.executable && selectors.browser_host && !selectors.browser_path_prefix && !selectors.focused_field) {
    return `site:${selectors.browser_host}`;
  }
  const unique = typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
  return `rule:${unique}`;
}

export function ContextSettings({
  writingModes: managedWritingModes = defaultWritingModes,
  onWritingModesChange,
  automationOnly = false,
  saveFailure,
  combined = false,
  outputMode,
  translationTargetLanguage,
  onOutputModeChange,
  onTranslationTargetLanguageChange,
  cleanupIntensity = "auto",
  onCleanupIntensityChange,
  windowOcrEnabled = false,
  onWindowOcrEnabledChange,
  visionProvider = "",
  visionModel = "",
  onVisionProviderChange,
  onVisionModelChange,
  accurateAsrProvider = "groq",
  accurateAsrModel = "",
  accurateAsrBaseUrl = "",
  onAccurateAsrProviderChange,
  onAccurateAsrModelChange,
  onAccurateAsrBaseUrlChange,
}: {
  writingModes?: WritingMode[];
  onWritingModesChange?: (writingModes: WritingMode[]) => void;
  automationOnly?: boolean;
  saveFailure?: SettingsSaveFailure | null;
  combined?: boolean;
  outputMode?: string;
  translationTargetLanguage?: string;
  onOutputModeChange?: (outputMode: string) => void;
  onTranslationTargetLanguageChange?: (language: string) => void;
  cleanupIntensity?: "auto" | "off" | "light" | "standard" | "heavy";
  onCleanupIntensityChange?: (intensity: "auto" | "off" | "light" | "standard" | "heavy") => void;
  windowOcrEnabled?: boolean;
  onWindowOcrEnabledChange?: (enabled: boolean) => void;
  visionProvider?: string;
  visionModel?: string;
  onVisionProviderChange?: (provider: string) => void;
  onVisionModelChange?: (model: string) => void;
  accurateAsrProvider?: ProviderId;
  accurateAsrModel?: string;
  accurateAsrBaseUrl?: string;
  onAccurateAsrProviderChange?: (provider: ProviderId) => void;
  onAccurateAsrModelChange?: (model: string) => void;
  onAccurateAsrBaseUrlChange?: (url: string) => void;
}) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<ContextSnapshot | null>(null);
  const [mappings, setMappings] = useState<ContextMapping[]>([]);
  const [applications, setApplications] = useState<ApplicationOption[]>([]);
  const [selectedApplicationId, setSelectedApplicationId] = useState("");
  const [selectedExecutable, setSelectedExecutable] = useState("");
  const [selectedWebsite, setSelectedWebsite] = useState("");
  const [browserPathPrefix, setBrowserPathPrefix] = useState("");
  const [focusedField, setFocusedField] = useState<FocusKind | "">("");
  const [editingMappingId, setEditingMappingId] = useState<string | null>(null);
  const [sourcePermissions, setSourcePermissions] = useState<ContextSourcePermissions>({
    ax_text: false,
    local_ocr: false,
    cloud_vision: false,
    context_text_to_providers: false,
  });
  const [selectedModeId, setSelectedModeId] = useState("general");
  const [styleExampleInput, setStyleExampleInput] = useState("");
  const [styleExampleOutput, setStyleExampleOutput] = useState("");
  const [styleExamplesApproved, setStyleExamplesApproved] = useState(false);
  const [mappingIntensity, setMappingIntensity] = useState<"inherit" | "auto" | "off" | "light" | "standard" | "heavy">("inherit");
  const [mappingCleanupEnabled, setMappingCleanupEnabled] = useState(true);
  const [mappingLearnEnabled, setMappingLearnEnabled] = useState(true);
  const [writingModes, setWritingModes] = useState<WritingMode[]>(managedWritingModes);
  const [draftModeId, setDraftModeId] = useState<string | null>(null);
  const [editingModeId, setEditingModeId] = useState("general");
  const [modeLabelDraft, setModeLabelDraft] = useState("");
  const [modePromptDraft, setModePromptDraft] = useState("");
  const [applicationsLoading, setApplicationsLoading] = useState(true);
  const [enabled, setEnabled] = useState(true);
  const [overrideFamily, setOverrideFamily] = useState<ContextFamily | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [initializationError, setInitializationError] = useState<string | null>(null);
  const [errorScope, setErrorScope] = useState<"prompt" | "mapping" | "override" | "context" | "browser" | "applications" | null>(null);
  const [promptErrorPlacement, setPromptErrorPlacement] = useState<"preview" | "prompt">("preview");
  const mappingDetailsRef = useRef<HTMLDetailsElement>(null);
  const editingModeSelectRef = useRef<HTMLButtonElement>(null);
  const confirmationFocusRef = useRef<HTMLElement | null>(null);
  const previousManagedModesRef = useRef(managedWritingModes);
  const [busyAction, setBusyAction] = useState<"context" | "browser" | "override" | "applications" | "mapping" | "delete_mapping" | null>(null);
  const busy = busyAction !== null;
  useEffect(() => {
    if (error && errorScope === "mapping" && mappingDetailsRef.current) mappingDetailsRef.current.open = true;
  }, [error, errorScope]);
  const [pendingConfirm, setPendingConfirm] = useState<{
    title: string;
    description: string;
    confirmLabel: string;
    action: { type: "select_mode"; modeId: string } | { type: "add_mode" } | { type: "delete_mode"; modeId: string } | { type: "delete_mapping"; mappingId: string };
  } | null>(null);

  const savedWritingModes = draftModeId ? writingModes.filter((mode) => mode.id !== draftModeId) : writingModes;
  const selectedMappingMode = savedWritingModes.find((mode) => mode.id === selectedModeId) ?? savedWritingModes[0];
  const editingMode = writingModes.find((mode) => mode.id === editingModeId) ?? writingModes[0];
  const editingModeLabel = editingMode?.builtin ? editingMode.label : modeLabelDraft;
  const modeIsDirty = Boolean(editingMode && (editingModeLabel !== editingMode.label || modePromptDraft !== editingMode.prompt));

  useEffect(() => {
    // Only reconcile an incoming settings snapshot. Local saves must not be
    // replaced by the previous prop value while persistence is pending.
    if (previousManagedModesRef.current === managedWritingModes) return;
    previousManagedModesRef.current = managedWritingModes;
    setWritingModes((current) => {
      const keepEditing = modeIsDirty || Boolean(draftModeId);
      const next = managedWritingModes.map((incoming) => {
        const existing = current.find((mode) => mode.id === incoming.id);
        return existing && ((keepEditing && existing.id === editingModeId) || sameWritingMode(existing, incoming))
          ? existing : incoming;
      });
      const protectedMode = keepEditing ? current.find((mode) => mode.id === editingModeId) : undefined;
      if (protectedMode && !next.some((mode) => mode.id === protectedMode.id)) next.push(protectedMode);
      return next.length === current.length && next.every((mode, index) => mode === current[index]) ? current : next;
    });
  }, [managedWritingModes, draftModeId, editingModeId, modeIsDirty]);

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
      .catch((reason: unknown) => {
        if (!cancelled) setInitializationError(reason instanceof Error ? reason.message : String(reason));
      });

    void invoke<ApplicationOption[]>("get_available_applications")
      .then((nextApplications) => {
        if (!cancelled) setApplications((current) => mergeApplicationOptions(nextApplications, current));
      })
      .catch((reason: unknown) => {
        if (!cancelled) setInitializationError(reason instanceof Error ? reason.message : String(reason));
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
        if (!cancelled) setInitializationError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      cancelled = true;
      cancelledListener = true;
      unlisten?.();
    };
  }, []);

  const refreshApplications = async () => {
    setBusyAction("applications");
    setApplicationsLoading(true);
    setErrorScope("applications");
    setError(null);
    try {
      const nextApplications = await invoke<ApplicationOption[]>("get_available_applications");
      setApplications((current) => mergeApplicationOptions(nextApplications, current));
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setApplicationsLoading(false);
      setBusyAction(null);
    }
  };

  const addApplicationFromDisk = async () => {
    setBusyAction("applications");
    setErrorScope("applications");
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
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const publishWritingModes = (nextWritingModes: WritingMode[]) => {
    setWritingModes(nextWritingModes);
    onWritingModesChange?.(nextWritingModes);
  };

  const withoutEditingDraft = () => writingModes.flatMap((mode) => {
    if (mode.id === draftModeId) return [];
    if (mode.id !== editingModeId || !modeIsDirty) return [mode];
    const saved = managedWritingModes.find((item) => item.id === mode.id);
    return saved ? [saved] : [];
  });

  const applySelectEditingMode = (modeId: string) => {
    const nextWritingModes = draftModeId && draftModeId !== modeId
      ? withoutEditingDraft() : modeIsDirty ? withoutEditingDraft() : writingModes;
    setWritingModes(nextWritingModes);
    if (draftModeId !== modeId) setDraftModeId(null);
    const mode = nextWritingModes.find((item) => item.id === modeId);
    if (!mode) return;
    setEditingModeId(mode.id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
    setErrorScope(null);
  };

  const selectEditingMode = (modeId: string) => {
    if (modeId === editingModeId) return;
    if (modeIsDirty) {
      confirmationFocusRef.current = editingModeSelectRef.current;
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前语气有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: { type: "select_mode", modeId },
      });
      return;
    }
    applySelectEditingMode(modeId);
  };

  const applyAddCustomMode = () => {
    const id = `custom.${Date.now()}.${Math.random().toString(36).slice(2, 8)}`;
    const mode: WritingMode = {
      id,
      label: t("自定义语气"),
      family: "general",
      prompt: t("根据我说的内容整理文字，保留事实、语气和具体信息，不要添加我没有说过的内容。"),
      builtin: false,
    };
    const baseWritingModes = withoutEditingDraft();
    const next = [...baseWritingModes, mode];
    setWritingModes(next);
    setDraftModeId(id);
    setEditingModeId(id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
    setErrorScope(null);
  };

  const addCustomMode = () => {
    if (modeIsDirty) {
      confirmationFocusRef.current = editingModeSelectRef.current;
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前语气有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: { type: "add_mode" },
      });
      return;
    }
    applyAddCustomMode();
  };

  const saveEditingMode = (placement: "preview" | "prompt") => {
    setErrorScope("prompt");
    setPromptErrorPlacement(placement);
    if (!editingMode) return;
    const label = editingMode.builtin ? editingMode.label : modeLabelDraft.trim();
    const prompt = modePromptDraft.trim();
    if (!label) {
      setError(t("请填写语气名称"));
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
    setErrorScope(null);
  };

  const deleteEditingMode = () => {
    setErrorScope("prompt");
    setPromptErrorPlacement("preview");
    if (!editingMode || editingMode.builtin) return;
    if (mappings.some((mapping) => mapping.mode_id === editingMode.id)) {
      setError(t("请先删除使用这个语气的 App 映射"));
      return;
    }
    confirmationFocusRef.current = editingModeSelectRef.current;
    setPendingConfirm({
      title: t("删除自定义语气"),
      description: t("确定删除“{name}”吗？").replace("{name}", editingMode.label),
      confirmLabel: t("删除自定义语气"),
      action: { type: "delete_mode", modeId: editingMode.id },
    });
  };

  const applyDeleteMode = (modeId: string) => {
    const mode = writingModes.find((item) => item.id === modeId);
    if (!mode || mode.builtin) return;
    if (mappings.some((mapping) => mapping.mode_id === modeId)) {
      setErrorScope("prompt");
      setPromptErrorPlacement("preview");
      setError(t("请先删除使用这个语气的 App 映射"));
      return;
    }
    const next = writingModes.filter((item) => item.id !== modeId);
    if (modeId === draftModeId) {
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
    if (selectedModeId === modeId) setSelectedModeId("general");
    setError(null);
    setErrorScope(null);
  };

  const loadMapping = (existing?: ContextMapping) => {
    setEditingMappingId(existing?.id ?? null);
    setSelectedApplicationId(existing?.bundle_id ?? "");
    setSelectedExecutable(existing?.executable ?? "");
    setSelectedWebsite(existing?.browser_host ?? "");
    setBrowserPathPrefix(existing?.browser_path_prefix ?? "");
    setFocusedField(existing?.focused_field ?? "");
    setSelectedModeId(existing?.mode_id ?? familyModeId(existing?.family ?? "general"));
    setStyleExampleInput(existing?.style_example_input ?? "");
    setStyleExampleOutput(existing?.style_example_output ?? "");
    setStyleExamplesApproved(existing?.style_examples_approved === true);
    setSourcePermissions({
      ax_text: sourcePermissionValue(existing?.source_permissions, "ax_text"),
      local_ocr: sourcePermissionValue(existing?.source_permissions, "local_ocr"),
      // Preserve an old, currently ineffective host-only grant so the user
      // can see and explicitly clear it instead of silently losing data.
      cloud_vision: sourcePermissionValue(existing?.source_permissions, "cloud_vision"),
      context_text_to_providers: sourcePermissionValue(existing?.source_permissions, "context_text_to_providers"),
    });
    setMappingIntensity(mappingIntensityFrom(existing));
    setMappingCleanupEnabled(existing?.cleanup_enabled !== false);
    setMappingLearnEnabled(existing?.dictionary_learn_enabled !== false);
    setError(null);
    setErrorScope(null);
  };

  const selectApplication = (bundleId: string) => {
    if (editingMappingId) {
      setSelectedApplicationId(bundleId);
      return;
    }
    const hasOtherSelector = Boolean(
      selectedExecutable.trim()
      || selectedWebsite.trim()
      || browserPathPrefix.trim()
      || focusedField
      || styleExampleInput.trim()
      || styleExampleOutput.trim()
      || styleExamplesApproved
      || Object.values(sourcePermissions).some(Boolean)
    );
    const existing = bundleId && !hasOtherSelector
      ? mappings.find((mapping) => selectorsEqual(mapping, {
        bundle_id: bundleId,
        executable: null,
        browser_host: null,
        browser_path_prefix: null,
        focused_field: null,
      }))
      : undefined;
    if (existing) {
      loadMapping(existing);
    } else {
      setEditingMappingId(null);
      setSelectedApplicationId(bundleId);
    }
  };

  const toggle = async (nextEnabled: boolean) => {
    setBusyAction("context");
    setErrorScope("context");
    setError(null);
    try {
      await invoke("set_context_enabled", { enabled: nextEnabled });
      setEnabled(nextEnabled);
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const requestBrowserAccess = async () => {
    setBusyAction("browser");
    setErrorScope("browser");
    setError(null);
    try {
      await invoke("request_browser_access");
      const next = await invoke<ContextSnapshot>("get_context_snapshot");
      setSnapshot(next);
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const setOverride = async (value: string) => {
    setErrorScope("override");
    const family = value === "auto" ? null : value as ContextFamily;
    setBusyAction("override");
    setError(null);
    try {
      const next = await invoke<ContextSnapshot>("set_context_override", { family });
      setSnapshot(next);
      setOverrideFamily(family);
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const saveMapping = async () => {
    setErrorScope("mapping");
    const application = applications.find((item) => item.bundle_id === selectedApplicationId);
    const mode = savedWritingModes.find((item) => item.id === selectedModeId);
    if (!mode) {
      setError(t("请先选择一个语气"));
      return;
    }
    const rawHost = selectedWebsite.trim();
    const browserHost = normalizeBrowserHost(rawHost);
    if (rawHost && !browserHost) {
      setError(t("请输入主机名，不要粘贴网址、路径或查询参数"));
      return;
    }
    const pathPrefix = browserPathPrefix.trim();
    if (pathPrefix && (
      !pathPrefix.startsWith("/")
      || pathPrefix.includes("//")
      || /[\\\s?#]/.test(pathPrefix)
    )) {
      setError(t("路径前缀必须以 / 开头，且不能含空格、双斜线、查询或片段"));
      return;
    }
    if (pathPrefix && !browserHost) {
      setError(t("路径前缀需要网站主机名"));
      return;
    }
    const executable = selectedExecutable.trim() || null;
    if (!application && !executable && !browserHost && !focusedField) {
      setError(t("至少选择一个匹配条件"));
      return;
    }
    const selectors = {
      bundle_id: application?.bundle_id ?? null,
      executable,
      browser_host: browserHost,
      browser_path_prefix: pathPrefix || null,
      focused_field: focusedField || null,
    };
    const savedSelectorMatch = mappings.find((mapping) => selectorsEqual(mapping, selectors));
    if (savedSelectorMatch && savedSelectorMatch.id !== editingMappingId) {
      if (!editingMappingId) loadMapping(savedSelectorMatch);
      setErrorScope("mapping");
      setError(t("此目标已有保存规则；请先检查其来源权限，再明确更新。"));
      return;
    }
    const cloudVisionNeedsNativeApp = sourcePermissions.cloud_vision && !selectors.bundle_id && !selectors.executable;
    if (cloudVisionNeedsNativeApp) {
      setError(t("自动云端视觉需要具体 App 或可执行文件；请关闭此权限或添加 App 选择器"));
      return;
    }
    const existing = mappings.find((mapping) => mapping.id === editingMappingId);
    const mappingId = existing?.id ?? newMappingId(selectors);
    const mappingLabel = existing?.label
      ?? (application?.label ?? (executable ? t("自定义 App 规则") : t("网站规则")));
    setBusyAction("mapping");
    setError(null);
    try {
      const mapping: ContextMapping = {
        id: mappingId,
        label: mappingLabel,
        family: mode.family,
        mode_id: mode.id,
        ...selectors,
        source_permissions: { ...sourcePermissions },
        style_examples_approved: styleExamplesApproved,
        enabled: true,
        cleanup_effort: null,
        cleanup_intensity: mappingIntensity === "inherit" ? null : mappingIntensity,
        cleanup_enabled: mappingCleanupEnabled,
        dictionary_learn_enabled: mappingLearnEnabled,
      };
      const styleInput = styleExampleInput.trim();
      const styleOutput = styleExampleOutput.trim();
      if (styleInput) mapping.style_example_input = styleInput;
      else if (existing?.style_example_input) mapping.style_example_input = existing.style_example_input;
      if (styleOutput) mapping.style_example_output = styleOutput;
      else if (existing?.style_example_output) mapping.style_example_output = existing.style_example_output;
      if (existing?.style_example_pairs?.length) {
        mapping.style_example_pairs = existing.style_example_pairs;
      }
      const next = await invoke<ContextMapping[]>("save_context_mapping", { mapping });
      setMappings(next);
      setEditingMappingId(null);
      setSelectedApplicationId("");
      setSelectedExecutable("");
      setSelectedWebsite("");
      setBrowserPathPrefix("");
      setFocusedField("");
      setSourcePermissions({
        ax_text: false,
        local_ocr: false,
        cloud_vision: false,
        context_text_to_providers: false,
      });
      setSelectedModeId("general");
      setStyleExampleInput("");
      setStyleExampleOutput("");
      setStyleExamplesApproved(false);
      setMappingIntensity("inherit");
      setMappingCleanupEnabled(true);
      setMappingLearnEnabled(true);
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const deleteMapping = async (id: string) => {
    setBusyAction("delete_mapping");
    setErrorScope("mapping");
    setError(null);
    try {
      setMappings(await invoke<ContextMapping[]>("delete_context_mapping", { id }));
      setErrorScope(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const selectedApplication = applications.find((application) => application.bundle_id === selectedApplicationId);
  const selectedBrowserHost = selectedWebsite.trim() ? normalizeBrowserHost(selectedWebsite) : null;
  const selectedSelectors = {
    bundle_id: selectedApplication?.bundle_id ?? null,
    executable: selectedExecutable.trim() || null,
    browser_host: selectedBrowserHost,
    browser_path_prefix: browserPathPrefix.trim() || null,
    focused_field: focusedField || null,
  };
  const selectedExistingMapping = mappings.find((mapping) => selectorsEqual(mapping, selectedSelectors));
  const editedMapping = mappings.find((mapping) => mapping.id === editingMappingId) ?? selectedExistingMapping;
  const retainedStylePairs = editedMapping?.style_example_pairs ?? [];
  const hasStyleExamples = Boolean(
    styleExampleInput.trim()
    || styleExampleOutput.trim()
    || editedMapping?.style_example_input
    || editedMapping?.style_example_output
    || retainedStylePairs.length > 0
  );
  const cloudVisionAvailable = Boolean(selectedApplication?.bundle_id || selectedExecutable.trim());
  const invalidCloudVisionGrant = sourcePermissions.cloud_vision && !cloudVisionAvailable;
  const promptError = (errorScope === "prompt" ? error : null)
    ?? (saveFailure?.fields.includes("writing_modes") ? saveFailure.message : null);
  const advancedError = (errorScope === "override" ? error : null)
    ?? (saveFailure?.fields.some((field) => field.startsWith("accurate_asr_") || field.startsWith("vision_")) ? saveFailure.message : null);
  const inlineError = (message: string | null) => message
    ? <p role="alert" className="mt-2 break-words text-[13px] leading-5 text-error-ink">{message}</p> : null;
  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t(automationOnly || combined ? "智能整理" : "语气")}
        description={t(combined ? "根据当前 App、输入框和你说的内容整理文字，也可以编辑 Prompt 和 App / 网站规则。" : automationOnly ? "根据当前 App、输入框和你说的内容，自动选择合适的 Prompt 和输出格式。" : "编辑语气 Prompt，并为 App 或网站指定固定语气。")}
      />
      {initializationError && <SettingsAlert>{initializationError}</SettingsAlert>}
      {errorScope === null && error && <SettingsAlert>{error}</SettingsAlert>}
      {(automationOnly || combined) && (
        <>
          <SettingsGroup title={t("App 上下文")} description={t("决定 VoiceFlow 是否根据当前 App 和输入框自动适配。")}>
            <SettingsRow title={t("App 上下文适配")} description={<>{t("根据你当前使用的 App 和输入框自动选择合适的 Prompt。")}{inlineError(errorScope === "context" ? error : null)}</>} icon={<ScanSearch size={17} strokeWidth={1.7} aria-hidden="true" />}>
              <Toggle checked={enabled} onChange={(nextEnabled) => void toggle(nextEnabled)} disabled={busy} label={t("App 上下文适配")} />
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
              <SettingsRow title={t("浏览器站点检测")} description={<>{t("Chrome / Safari 需要额外授权，VoiceFlow 只保存站点标识。")}{inlineError(errorScope === "browser" ? error : null)}</>} icon={<Globe size={17} className="text-warning-ink" aria-hidden="true" />}>
                <button type="button" onClick={() => void requestBrowserAccess()} aria-busy={busyAction === "browser"} disabled={busy} className={`${secondaryButtonClass} min-w-32`}>{t(busyAction === "browser" ? "处理中…" : "启用访问")}</button>
              </SettingsRow>
            )}
          </SettingsGroup>
          <SettingsGroup title={t("输出格式")} description={t("自动整理会综合 App 上下文和你说的内容；手动模式只影响当前录音。")}>
            <SettingsRow title={t("输出模式")} description={t("自动根据当前 App、输入框和你说的内容选择整理方式；手动模式会覆盖自动判断。")}>
              <Select aria-label={t("输出模式")} value={outputMode ?? "auto"} onValueChange={(value) => onOutputModeChange?.(value)} className={`w-40 max-w-full ${fieldClass}`}>
                {outputModeOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
              </Select>
            </SettingsRow>
            {outputMode === "translation" && <SettingsRow title={t("目标语言")} description={t("只控制输出语言；语音识别仍然自动检测。")}>
              <Select aria-label={t("翻译目标语言")} value={translationTargetLanguage ?? "en"} onValueChange={(value) => onTranslationTargetLanguageChange?.(value)} className={`w-40 max-w-full ${fieldClass}`}>
                {translationLanguageOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
              </Select>
            </SettingsRow>}
          </SettingsGroup>
          <SettingsDisclosure title={t("高级整理设置")} description={t("临时场景、精确转写和视觉模型。授权选项保持独立。")} error={advancedError}>
            {inlineError(advancedError)}
            <div className="divide-y divide-border">
            <SettingsRow title={t("临时覆盖")} description={t("只对当前运行有效，重启或切换 App 后会恢复自动适配。")}>
              <Select id="context-temporary-override" aria-label={t("临时覆盖")} value={overrideFamily ?? "auto"} onValueChange={(value) => void setOverride(value)} disabled={busy} className={`w-52 max-w-full ${fieldClass}`}>
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
                  {t("第二枪需要自定义 / 兼容接口上已填的百炼密钥。")}
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
      )}

      {(!automationOnly || combined) && <>
      <SettingsGroup variant="surface" title={t("语气与预览")} description={t("编辑与试跑不会切换当前听写语气。更改需保存。")}>
        <div className="px-5 py-4">
          <label className="block text-xs text-secondary">
            {t("编辑哪种语气")}
            <Select ref={editingModeSelectRef} aria-label={t("编辑哪种语气")} value={editingMode?.id ?? ""} onValueChange={(value) => value === ADD_CUSTOM_MODE_OPTION ? addCustomMode() : selectEditingMode(value)} disabled={busy || !editingMode} className={`mt-2 w-full ${fieldClass}`}>
              {writingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}
              <option value={ADD_CUSTOM_MODE_OPTION}>＋ {t("添加自定义语气")}</option>
            </Select>
          </label>
          {!editingMode?.builtin && <label className="mt-4 block text-xs text-secondary">{t("语气名称")}<input aria-label={t("自定义语气名称")} value={modeLabelDraft} onChange={(event) => setModeLabelDraft(event.target.value)} disabled={busy} maxLength={64} className={`mt-2 w-full ${fieldClass}`} /></label>}
          {editingMode && <WritingPreview mode={{ ...editingMode, label: editingModeLabel, prompt: modePromptDraft }} compareSaved={modeIsDirty && editingMode.id !== draftModeId} isDraft={modeIsDirty || Boolean(draftModeId)} />}
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <button type="button" onClick={() => saveEditingMode("preview")} disabled={busy || !editingMode} className={`${buttonClass} min-w-28`}>{t("保存语气")}</button>
            {!editingMode?.builtin && <button type="button" onClick={deleteEditingMode} disabled={busy} className={dangerButtonClass}>{t("删除自定义语气")}</button>}
          </div>
          {promptErrorPlacement === "preview" && inlineError(promptError)}
        </div>
      </SettingsGroup>
      <SettingsDisclosure
        title={t("编辑 Prompt")}
        description={t("编辑 Prompt，或创建只属于你的语气。")}
        summary={modeIsDirty || draftModeId ? t("有未保存的更改") : t("可选")}
        error={promptError}
      >
        <div className="pt-4">
          <p className="mb-4 max-w-prose text-xs leading-5 text-secondary">{t("Prompt 只作为写作指导，不会覆盖 VoiceFlow 的事实保护规则。")}</p>
          <label className="block text-xs text-secondary">
            {t("Prompt")}
            <textarea aria-label={t("语气 Prompt")} value={modePromptDraft} onChange={(event) => setModePromptDraft(event.target.value)} disabled={busy} maxLength={8_000} rows={6} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2.5 text-sm leading-6 outline-none transition-colors duration-150 ${focusRingClass}`} />
            <span className="mt-1 block text-xs text-tertiary">{t("可以用中文或英文描述希望保留什么、如何组织，以及明确禁止添加什么。")} </span>
          </label>
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <button type="button" onClick={() => saveEditingMode("prompt")} disabled={busy || !editingMode} className={`${buttonClass} min-w-28`}>{t("保存语气")}</button>
          </div>
          {promptErrorPlacement === "prompt" && inlineError(promptError)}
        </div>
      </SettingsDisclosure>

      <SettingsGroup variant="surface" title={t("App / 网站映射")} description={t("可组合 App、可执行文件、网站、路径和输入框条件；所有已选条件必须同时匹配。")}>
        <div className="vf-mapping-form space-y-4 px-5 py-4">
          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-0 flex-1 text-xs text-secondary">{t("选择 App（可选）")}
              <Select aria-label={t("选择 App")} value={selectedApplicationId} onValueChange={(value) => selectApplication(value)} disabled={applicationsLoading || busy || applications.length === 0} className={`mt-2 w-full ${fieldClass}`}>
                <option value="">{applicationsLoading ? t("正在加载 App…") : applications.length === 0 ? t("没有找到 App") : t("不限定 App")}</option>
                {applications.map((application) => <option key={application.bundle_id} value={application.bundle_id}>{application.label}</option>)}
              </Select>
            </label>
            <button type="button" onClick={() => void addApplicationFromDisk()} disabled={applicationsLoading || busy} className={secondaryButtonClass}><FolderOpen size={15} aria-hidden="true" />{t("从应用程序中选择")}</button>
            <IconButton size="md" label={t("刷新 App 列表")} icon={<RefreshCw size={16} aria-hidden="true" />} onClick={() => void refreshApplications()} disabled={applicationsLoading || busy} />
          </div>
          {inlineError(errorScope === "applications" ? error : null)}
          <p className="flex items-center gap-1.5 text-xs text-tertiary"><AppWindowMac size={14} aria-hidden="true" />{t("运行中的 App 会随刷新更新；手动添加的 App 即使未打开也能生效。")} </p>
          <label className="block text-xs text-secondary">{t("语气")}<Select aria-label={t("应用映射语气")} value={selectedMappingMode?.id ?? ""} onValueChange={(value) => setSelectedModeId(value)} disabled={busy || savedWritingModes.length === 0} className={`mt-2 w-full ${fieldClass}`}>{savedWritingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}</Select></label>
          <details ref={mappingDetailsRef} className="vf-inline-disclosure border-y border-border py-3 text-xs text-secondary">
            <summary className={`cursor-pointer rounded-md font-medium text-primary ${focusRingClass}`}>
              {t("高级映射选项")}{selectedExecutable || selectedWebsite || browserPathPrefix || focusedField || hasStyleExamples || mappingIntensity !== "inherit" || !mappingCleanupEnabled || !mappingLearnEnabled ? ` · ${t("已设置")}` : ""}
            </summary>
            <p className="mt-2 leading-5 text-tertiary">{t("可选条件必须同时匹配；收起不会清除已设置的选项。")}</p>
            <div className="mt-3 space-y-4">
          <label className="block text-xs text-secondary">
            {t("可执行文件（可选）")}
            <input aria-label={t("可执行文件名")} value={selectedExecutable} onChange={(event) => setSelectedExecutable(event.target.value)} disabled={busy} maxLength={255} placeholder={t("例如 Cursor")} className={`mt-2 w-full ${fieldClass}`} />
            <span className="mt-1 block text-xs text-tertiary">{t("可与其他条件组合；不会显示在听写 HUD。")}</span>
          </label>
          <label className="block text-xs text-secondary">
            {t("网站或域名（可选）")}
            <Autocomplete
              aria-label={t("应用映射网站")}
              options={websiteOptions.map((website) => ({ value: website.host, label: website.label }))}
              value={selectedWebsite}
              onValueChange={setSelectedWebsite}
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
              onChange={(event) => setBrowserPathPrefix(event.target.value)}
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
              onValueChange={(value) => setFocusedField(value as FocusKind | "")}
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
              <textarea aria-label={t("App 风格示例输入")} value={styleExampleInput} onChange={(event) => setStyleExampleInput(event.target.value)} maxLength={2_000} rows={3} disabled={busy} placeholder={t("贴一条你平时微信怎么打")} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent ${focusRingClass}`} />
            </label>
            <label className="block text-xs text-secondary">{t("期望输出")}
              <textarea aria-label={t("App 风格期望输出")} value={styleExampleOutput} onChange={(event) => setStyleExampleOutput(event.target.value)} maxLength={2_000} rows={3} disabled={busy} placeholder={t("希望 VoiceFlow 输出的样子…")} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none focus:border-accent ${focusRingClass}`} />
            </label>
          </div>
          <label className="block text-xs text-secondary">{t("整理强度")}
            <Select aria-label={t("整理强度")} value={mappingIntensity} onValueChange={(value) => setMappingIntensity(value as "inherit" | "auto" | "off" | "light" | "standard" | "heavy")} disabled={busy} className={`mt-2 w-full ${fieldClass}`}>
              <option value="inherit">{t("跟随全局")}</option>
              <option value="auto">{t("自动按场景")}</option>
              <option value="off">{t("关")}</option>
              <option value="light">{t("轻")}</option>
              <option value="standard">{t("中")}</option>
              <option value="heavy">{t("重")}</option>
            </Select>
          </label>
          <SettingsRow title={t("这个 App 使用 AI 整理")} description={t("关闭后仍会去掉 um / 嗯，但不请求整理服务。")}>
            <Toggle checked={mappingCleanupEnabled} onChange={setMappingCleanupEnabled} disabled={busy} label={t("这个 App 使用 AI 整理")} />
          </SettingsRow>
          <SettingsRow title={t("在这个 App 学习词条")} description={t("关闭后不会从该 App 的输入框学习纠正。")}>
            <Toggle checked={mappingLearnEnabled} onChange={setMappingLearnEnabled} disabled={busy} label={t("在这个 App 学习词条")} />
          </SettingsRow>
            </div>
          </details>
          <SettingsRow
            title={t("允许风格示例发送给整理服务商")}
            description={t("启用后，此映射保留的输入、期望输出和已学习示例对可能随整理请求发送给已配置的整理服务商。关闭时仍保留这些示例，但不发送。此权限独立于“允许文字发送给服务商”。")}
          >
            <Toggle
              checked={styleExamplesApproved}
              onChange={setStyleExamplesApproved}
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
          {hasStyleExamples && !styleExamplesApproved && (
            <p className="text-xs leading-5 text-warning-ink">{t("风格示例会保留，但在你单独批准前不会进入整理服务请求。")}</p>
          )}
          <div className="vf-mapping-permissions rounded-xl border border-border bg-elevated/30 px-3 sm:px-4">
            <div className="py-3">
              <p className="text-sm font-medium text-primary">{t("自动文字来源权限")}</p>
              <p className="mt-1 max-w-2xl text-xs leading-5 text-tertiary">{t("新规则和缺少来源授权的旧规则默认关闭；迁移时只会按之前明确开启的全局 OCR 选择恢复本机 OCR。全局上下文开关仍会统一关闭自动读取。")}</p>
            </div>
            <SettingsRow title={t("允许读取辅助功能文字")} description={t("允许为此匹配目标自动读取有限的当前输入框和附近文字。")}>
              <Toggle
                checked={sourcePermissions.ax_text}
                onChange={(ax_text) => setSourcePermissions((current) => ({ ...current, ax_text }))}
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
                onChange={(local_ocr) => setSourcePermissions((current) => ({ ...current, local_ocr }))}
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
                onChange={(cloud_vision) => setSourcePermissions((current) => ({ ...current, cloud_vision }))}
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
                onChange={(context_text_to_providers) => setSourcePermissions((current) => ({ ...current, context_text_to_providers }))}
                disabled={busy}
                label={t("允许文字发送给服务商")}
              />
            </SettingsRow>
          </div>
          <p className="text-xs text-tertiary">{t("示例不会自动从历史记录学习。")}</p>
          {selectedExistingMapping && (
            <p className="text-xs text-secondary">
              {t(editingMappingId
                ? "这个目标已有设置；保存后会更新它的语气。"
                : "此目标已有规则；使用编辑按钮查看并保留现有权限。")}
            </p>
          )}
          {inlineError(errorScope === "mapping" ? error : null)}
          <button type="button" onClick={() => void saveMapping()} aria-busy={busyAction === "mapping"} disabled={busy} className={`${buttonClass} min-w-36`}><Plus size={16} aria-hidden="true" />{t(busyAction === "mapping" ? "保存中…" : editingMappingId ? "更新映射" : "保存映射")}</button>
        </div>
        {mappings.length > 0 && (
          <div className="px-5">
            <p className="py-3 text-xs font-medium text-tertiary">{t("已保存的目标规则")}</p>
            {mappings.map((mapping) => (
              <div key={mapping.id} className="flex items-center gap-3 border-t border-border py-3">
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-elevated text-xs font-semibold text-primary">{mapping.label.slice(0, 1)}</span>
                <div className="min-w-0 flex-1">
                  <p className="break-words text-sm font-medium text-primary">{mapping.label} <span className="font-normal text-secondary">→ {t(writingModes.find((mode) => mode.id === mapping.mode_id)?.label ?? familyLabels[mapping.family])}</span></p>
                  <p className="mt-1 break-words text-xs leading-[18px] text-secondary">
                    {matchingSelectors(mapping, applications, t).length > 0
                      ? matchingSelectors(mapping, applications, t).join(" · ")
                      : t("按 App 匹配")}
                  </p>
                </div>
                <IconButton size="sm" label={t("编辑映射")} icon={<Pencil size={15} aria-hidden="true" />} onClick={() => loadMapping(mapping)} disabled={busy} />
                <IconButton size="sm" label={t("删除映射")} aria-label={`${t("删除应用映射")} ${mapping.label}`} tone="danger" icon={<Trash2 size={15} aria-hidden="true" />} onClick={() => setPendingConfirm({ title: t("删除映射"), description: t("确定删除“{name}”吗？").replace("{name}", mapping.label), confirmLabel: t("删除映射"), action: { type: "delete_mapping", mappingId: mapping.id } })} disabled={busy} />
              </div>
            ))}
          </div>
        )}
      </SettingsGroup>
      </>}
      <ConfirmDialog
        open={pendingConfirm != null}
        title={pendingConfirm?.title ?? ""}
        description={pendingConfirm?.description ?? ""}
        confirmLabel={pendingConfirm?.confirmLabel ?? t("确定")}
        cancelLabel={t("取消")}
        returnFocusRef={confirmationFocusRef}
        onCancel={() => setPendingConfirm(null)}
        onConfirm={() => {
          const action = pendingConfirm?.action;
          setPendingConfirm(null);
          if (action?.type === "select_mode") applySelectEditingMode(action.modeId);
          else if (action?.type === "add_mode") applyAddCustomMode();
          else if (action?.type === "delete_mode") applyDeleteMode(action.modeId);
          else if (action?.type === "delete_mapping") void deleteMapping(action.mappingId);
        }}
      />
    </SettingsShell>
  );
}

const fieldClass = inputClass;
