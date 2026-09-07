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
