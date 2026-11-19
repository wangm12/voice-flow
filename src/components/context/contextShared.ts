import { BriefcaseBusiness, CodeXml, Mail, MessageCircle } from "lucide-react";
import { inputClass } from "../../lib/theme";
import type { WritingPreferenceCandidate } from "../WritingPreferenceReview";

export const fieldClass = inputClass;

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

export type ContextPrivacyPreset = "off" | "local" | "providers" | "custom";

export function contextPrivacyPreset(
  permissions: ContextSourcePermissions,
  styleExamplesApproved: boolean,
  preferenceGuidanceProviderApproved: boolean,
): ContextPrivacyPreset {
  const noProviderEgress = !permissions.cloud_vision
    && !permissions.context_text_to_providers
    && !styleExamplesApproved
    && !preferenceGuidanceProviderApproved;
  if (!permissions.ax_text && !permissions.local_ocr && noProviderEgress) return "off";
  if (permissions.ax_text && permissions.local_ocr && noProviderEgress) return "local";
  if (permissions.ax_text && !permissions.local_ocr && !permissions.cloud_vision
    && permissions.context_text_to_providers && !styleExamplesApproved && !preferenceGuidanceProviderApproved) {
    return "providers";
  }
  return "custom";
}

export type WritingTaskId = "project_update" | "email" | "work_chat" | "personal_chat" | "document_editing";
export type WritingPreferenceDimension = "list_structure" | "greeting_tone" | "terminal_punctuation";
export type WritingPreferenceValue = "bullets" | "conversational_greeting" | "respectful_greeting" | "terminal_period" | "no_terminal_period";
export type WritingPreferenceStatus = "active" | "paused" | "forgotten";

export type ContextWritingPreference = {
  task_id?: WritingTaskId;
  dimension: WritingPreferenceDimension;
  value: WritingPreferenceValue;
  status: WritingPreferenceStatus;
  version: number;
};

type PreferenceDraftItem = {
  value: WritingPreferenceValue | "";
  status: WritingPreferenceStatus | "";
};

export type WritingPreferenceDraft = Record<WritingPreferenceDimension, PreferenceDraftItem>;

export type PreferenceReviewEditorState = {
  editingMappingId: string | null;
  preferenceRevisionBaseline: { mappingId: string | null; revision: number | null };
  selectedModeFamily: ContextFamily | null;
  selectedWritingTaskId: WritingTaskId | "";
  selectedApplicationId: string;
  selectedExecutable: string;
  selectedWebsite: string;
  browserPathPrefix: string;
  focusedField: FocusKind | "";
  writingPreferenceDraft: WritingPreferenceDraft;
  preferenceGuidanceProviderApproved: boolean;
};

export type PreferenceReviewStartSnapshot = {
  mapping: ContextMapping | undefined;
};

export type PreferenceReviewConflict = {
  candidateId: string;
  mappingId: string;
  taskId: WritingTaskId;
  dimension: WritingPreferenceDimension;
  reason: "scope_changed" | "preference_draft_changed" | "provider_grant_changed";
};

export function writingPreferenceCandidateKey(candidate: Pick<WritingPreferenceCandidate, "candidate_id" | "candidate_generation">): string {
  return `${candidate.candidate_id}:${candidate.candidate_generation}`;
}

export function samePreferenceDraftItem(left: PreferenceDraftItem, right: PreferenceDraftItem): boolean {
  return left.value === right.value && left.status === right.status;
}

export function selectedModeFamilyForEditor(
  modes: WritingMode[],
  selectedModeId: string,
  draftModeId: string | null,
): ContextFamily | null {
  return modes.find((mode) => mode.id === selectedModeId && mode.id !== draftModeId)?.family ?? null;
}

export function mappingSelectorsFromEditor(editor: PreferenceReviewEditorState): Pick<ContextMapping, "bundle_id" | "executable" | "browser_host" | "browser_path_prefix" | "focused_field"> | null {
  const rawWebsite = editor.selectedWebsite.trim();
  const browserHost = rawWebsite ? normalizeBrowserHost(rawWebsite) : null;
  if (rawWebsite && !browserHost) return null;
  return {
    bundle_id: editor.selectedApplicationId || null,
    executable: editor.selectedExecutable.trim() || null,
    browser_host: browserHost,
    browser_path_prefix: editor.browserPathPrefix.trim() || null,
    focused_field: editor.focusedField || null,
  };
}

export function writingPreferenceMappingScope(
  mapping: ContextMapping | undefined,
  applications: ApplicationOption[],
  translate: (source: string) => string,
): string | null {
  if (!mapping || !mapping.enabled) return null;
  const selectors: string[] = [];
  if (mapping.bundle_id) {
    const app = applications.find((option) => option.bundle_id === mapping.bundle_id);
    selectors.push(`${translate("App")}: ${app?.label ?? mapping.label} (${mapping.bundle_id})`);
  }
  if (mapping.executable) selectors.push(`${translate("可执行文件")}: ${mapping.executable}`);
  if (mapping.browser_host) {
    selectors.push(`${translate("网站")}: ${translate(websiteLabels[mapping.browser_host] ?? mapping.browser_host)} (${mapping.browser_host})`);
  }
  if (mapping.browser_path_prefix) selectors.push(`${translate("路径前缀")}: ${mapping.browser_path_prefix}`);
  if (mapping.focused_field) selectors.push(`${translate("输入框")}: ${focusFieldLabel(mapping.focused_field, translate)}`);
  return selectors.length > 0 ? selectors.join(" · ") : null;
}

export const emptyWritingPreferenceDraft: WritingPreferenceDraft = {
  list_structure: { value: "", status: "" },
  greeting_tone: { value: "", status: "" },
  terminal_punctuation: { value: "", status: "" },
};

export type ContextSnapshot = {
  profile: { id: string; family: ContextFamily; writing_mode_id?: string | null; writing_task_id?: WritingTaskId | null; app_label: string; icon_key: string; source: string; confidence: number };
  policy?: { input_kind?: FocusKind };
  browser_access_status: "not_applicable" | "disabled" | "needs_permission" | "granted" | "unavailable";
};

export type ContextMapping = {
  id: string;
  label: string;
  family: ContextFamily;
  writing_task_id?: WritingTaskId | null;
  writing_preferences?: ContextWritingPreference[];
  preference_guidance_provider_approved?: boolean;
  preference_revision?: number;
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

export type ApplicationOption = {
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

export function sameWritingMode(left: WritingMode, right: WritingMode): boolean {
  return left.id === right.id && left.label === right.label && left.family === right.family
    && left.prompt === right.prompt && left.builtin === right.builtin;
}

export function mergeApplicationOptions(...groups: ApplicationOption[][]): ApplicationOption[] {
  const options = new Map<string, ApplicationOption>();
  for (const group of groups) {
    for (const application of group) {
      if (!options.has(application.bundle_id)) options.set(application.bundle_id, application);
    }
  }
  return [...options.values()].sort((left, right) => left.label.localeCompare(right.label));
}

export function applicationsFromMappings(mappings: ContextMapping[]): ApplicationOption[] {
  return mappings.flatMap((mapping) => mapping.bundle_id ? [{ bundle_id: mapping.bundle_id, label: mapping.label }] : []);
}

export function mappingIntensityFrom(
  mapping?: ContextMapping,
): "inherit" | "auto" | "off" | "light" | "standard" | "heavy" {
  if (mapping?.cleanup_intensity) return mapping.cleanup_intensity;
  if (mapping?.cleanup_effort === "light" || mapping?.cleanup_effort === "standard") {
    return mapping.cleanup_effort;
  }
  return "inherit";
}

export const familyLabels: Record<ContextFamily, string> = {
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

export const coreWritingSceneCards = [
  {
    modeId: "personal_chat",
    families: ["personal_chat"],
    label: "个人聊天",
    description: "保留各语言口语语气词；单句句末可不加句号。",
    icon: MessageCircle,
  },
  {
    modeId: "work_chat",
    families: ["work_chat", "project_management", "calendar_task", "developer_collaboration", "customer_support"],
    label: "工作沟通",
    description: "按场景保留状态、任务、日期和下一步。",
    icon: BriefcaseBusiness,
  },
  {
    modeId: "email",
    families: ["email", "document", "form_filling", "notes_journaling"],
    label: "邮件与文档",
    description: "保留原意与结构，不补写未说出的信息。",
    icon: Mail,
  },
  {
    modeId: "prompt_or_code",
    families: ["prompt_or_code", "terminal"],
    label: "代码与终端",
    description: "保护标识符、命令、路径和代码内容。",
    icon: CodeXml,
  },
] as const;

export const outputModeOptions = [
  { value: "auto", label: "自动整理" },
  { value: "email", label: "邮件" },
  { value: "bullets", label: "要点 / 步骤" },
  { value: "meeting_notes", label: "会议记录" },
  { value: "code", label: "代码 / 终端" },
  { value: "translation", label: "翻译" },
] as const;

export const privacyPresetOptions: { value: Exclude<ContextPrivacyPreset, "custom">; label: string; description: string }[] = [
  {
    value: "off",
    label: "关闭上下文读取",
    description: "不读取辅助功能文字或本机 OCR；上下文文字、样例、偏好和图片都不会发给服务。",
  },
  {
    value: "local",
    label: "仅本机使用（不出本机）",
    description: "允许本机读取辅助功能文字和可选 OCR；不发送上下文文字、样例、偏好或窗口图片。",
  },
  {
    value: "providers",
    label: "允许文字发送给转写与整理服务",
    description: "允许有限辅助功能文字进入已配置 ASR 术语提示和当前整理请求；不会发送截图，也不会批准云端视觉、风格样例或结构化偏好。",
  },
];



export const defaultWritingModes: WritingMode[] = Object.entries(familyLabels).map(([id, label]) => ({
  id,
  label,
  family: id as ContextFamily,
  prompt: id === "personal_chat"
    ? "Keep the user's casual chat voice in every language. Preserve spoken particles, laughter, hesitation, slang, swearing, and informal fragments, including particles such as 哈哈, 嗯, 呃, 啊, uh, and um; do not remove them just because they sound like fillers. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Keep existing punctuation and do not force a terminal period onto a single casual sentence when the user did not say one. Never turn the message into an email."
    : "Use the smallest useful cleanup and preserve the original structure.",
  builtin: true,
}));

export const knownNativeTaskBundleIds = new Set([
  "com.openai.codex",
  "com.todesktop.230313mzl4w4u92",
  "com.microsoft.vscode",
  "com.apple.terminal",
  "com.googlecode.iterm2",
  "dev.warp.warp-stable",
  "com.apple.mail",
  "com.microsoft.outlook",
  "com.tinyspeck.slackmacgap",
  "com.microsoft.teams2",
  "com.tencent.xinwechat",
  "com.apple.mobilesms",
  "com.hnc.discord",
  "notion.id",
  "com.microsoft.word",
  "com.apple.ical",
  "com.apple.reminders",
]);
export const ADD_CUSTOM_MODE_OPTION = "__add_custom_mode__";

export function familyModeId(family: ContextFamily): string {
  return family;
}

export const websiteOptions = [
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

export const focusFieldOptions: { value: FocusKind; label: string }[] = [
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

export const writingTaskOptions: { value: WritingTaskId; label: string }[] = [
  { value: "project_update", label: "项目更新" },
  { value: "email", label: "邮件正文" },
  { value: "work_chat", label: "工作聊天" },
  { value: "personal_chat", label: "私人聊天" },
  { value: "document_editing", label: "文档撰写" },
];

export function sourcePermissionValue(
  permissions: ContextMapping["source_permissions"],
  key: keyof ContextSourcePermissions,
): boolean {
  return permissions?.[key] === true;
}

export function preferenceDraftFrom(mapping?: ContextMapping, taskId?: WritingTaskId | ""): WritingPreferenceDraft {
  const draft: WritingPreferenceDraft = {
    list_structure: { ...emptyWritingPreferenceDraft.list_structure },
    greeting_tone: { ...emptyWritingPreferenceDraft.greeting_tone },
    terminal_punctuation: { ...emptyWritingPreferenceDraft.terminal_punctuation },
  };
  for (const preference of mapping?.writing_preferences ?? []) {
    if ((preference.task_id ?? mapping?.writing_task_id) === taskId && preference.dimension in draft) {
      draft[preference.dimension] = { value: preference.value, status: preference.status };
    }
  }
  return draft;
}

export function preferencesFromDraft(draft: WritingPreferenceDraft, taskId: WritingTaskId | ""): ContextWritingPreference[] {
  if (!taskId) return [];
  return (Object.entries(draft) as [WritingPreferenceDimension, PreferenceDraftItem][])
    .flatMap(([dimension, item]) => item.value && item.status
      ? [{ task_id: taskId, dimension, value: item.value, status: item.status, version: 1 }]
      : []);
}

export function hasCloudSupportedActivePreference(preferences: ContextWritingPreference[]): boolean {
  return preferences.some((preference) => preference.status === "active"
    && (preference.dimension === "greeting_tone" || preference.dimension === "terminal_punctuation"));
}

export function sourceLabel(source: string, translate: (source: string) => string): string {
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

export function matchingSelectors(
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

export function normalizeBrowserHost(value: string): string | null {
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

export function selectorsEqual(left: ContextMapping, right: Pick<ContextMapping, "bundle_id" | "executable" | "browser_host" | "browser_path_prefix" | "focused_field">): boolean {
  const normalizePath = (value: string | null | undefined) => {
    const path = value?.trim() ?? "";
    if (!path) return null;
    if (!path.startsWith("/") || path.includes("//") || /[\\\s?#]/.test(path)) return path;
    return path === "/" ? path : path.replace(/\/+$/, "");
  };
  const normalizeHost = (value: string | null | undefined) => {
    const host = value?.trim() ?? "";
    return host ? normalizeBrowserHost(host) ?? host.toLowerCase() : null;
  };
  return (left.bundle_id ?? null) === (right.bundle_id ?? null)
    && (left.executable || null) === (right.executable || null)
    && normalizeHost(left.browser_host) === normalizeHost(right.browser_host)
    && normalizePath(left.browser_path_prefix) === normalizePath(right.browser_path_prefix)
    && (left.focused_field ?? null) === (right.focused_field ?? null);
}

export function newMappingId(): string {
  const unique = typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
  return `rule:${unique}`;
}

