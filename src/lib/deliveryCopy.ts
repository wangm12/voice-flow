/** Human-readable delivery fallback reasons keyed by backend enum values. */
export const deliveryReasonLabels: Record<string, string> = {
  delivery_failed: "自动插入失败，文字已保存，可复制",
  accessibility_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  browser_permission_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  input_unavailable: "未找到可编辑输入框，文字已复制到剪贴板，请手动粘贴",
  input_changed: "输入框已变化，文字已复制到剪贴板，请手动粘贴",
  target_changed: "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
  target_unavailable: "未能确认输入目标，文字已复制到剪贴板，请手动粘贴",
  paste_failed: "自动粘贴未完成，文字已复制到剪贴板，请手动粘贴",
  retry_clipboard_only: "自动插入失败，文字已复制到剪贴板",
  paste_unverified: "已尝试写入输入框，请确认目标内容",
  selected_action_clipboard_fallback: "目标变化，结果已复制",
};

const genericFallbackMessage = "处理失败，请检查结果或重试";

export function deliveryReasonMessage(
  reason: string | null | undefined,
  translate: (source: string) => string,
): string | null {
  if (!reason) return null;
  const label = deliveryReasonLabels[reason];
  if (label) return translate(label);
  return translate(genericFallbackMessage);
}
