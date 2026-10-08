import { translationLanguageOptions } from "./translationLanguages";

export function formatHudTranslationLabel(target: string | null | undefined, translate: (source: string) => string): string | null {
  const language = translationLanguageOptions.find((option) => option.value === target);
  return language ? `${translate("翻译")} → ${language.label}` : null;
}

export const HUD_STYLE_LABELS: Record<string, string> = {
  general: "通用",
  email: "邮件",
  browser_search: "搜索",
  work_chat: "工作短讯",
  personal_chat: "口语",
  document: "文档",
  project_management: "待办",
  calendar_task: "日程",
  developer_collaboration: "代码",
  prompt_or_code: "代码",
  terminal: "命令",
  form_filling: "表单",
  notes_journaling: "笔记",
  social_media: "社交",
  customer_support: "客服",
};

export type HudContextSource = "none" | "ax" | "ocr" | "cloud_vision";

const HUD_CONTEXT_SOURCE_LABELS: Record<Exclude<HudContextSource, "none">, string> = {
  ax: "辅助功能文字",
  ocr: "本机 OCR",
  cloud_vision: "云端视觉",
};

export function formatHudContextSource(
  source: HudContextSource | null | undefined,
  translate: (source: string) => string = (value) => value,
): string | null {
  if (!source || source === "none" || !(source in HUD_CONTEXT_SOURCE_LABELS)) return null;
  return translate(HUD_CONTEXT_SOURCE_LABELS[source]);
}

function isUnknownAppName(app: string): boolean {
  const trimmed = app.trim();
  return (
    trimmed.length === 0
    || /^general$/i.test(trimmed)
    || /^unknown app$/i.test(trimmed)
    || trimmed === "未知 App"
    || trimmed === "未知应用"
  );
}

const HUD_INTENSITY_LABELS: Record<string, string> = {
  off: "关",
  light: "轻",
  standard: "中",
  heavy: "重",
};

export function formatHudIntensityLabel(
  app: string | null | undefined,
  intensity: "off" | "light" | "standard" | "heavy" | null | undefined,
  translate: (source: string) => string = (source) => source,
): string | null {
  if (intensity == null || !(intensity in HUD_INTENSITY_LABELS)) {
    return null;
  }
  const appLabel = app && !isUnknownAppName(app) ? app.trim() : translate("未知应用");
  return `${appLabel} · ${translate(HUD_INTENSITY_LABELS[intensity])}`;
}

export function formatHudContextLabel(
  app: string | null | undefined,
  style: string | null | undefined,
  fallbackLabel: string | null | undefined,
  translate: (source: string) => string = (source) => source,
): string | null {
  if (app != null || style != null) {
    const appLabel = app && !isUnknownAppName(app) ? app.trim() : translate("未知应用");
    const styleSource = HUD_STYLE_LABELS[style ?? "general"] ?? "通用";
    return `${appLabel} · ${translate(styleSource)}`;
  }
  const fallback = fallbackLabel?.trim();
  return fallback || null;
}
