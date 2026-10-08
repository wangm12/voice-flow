/** Human-readable delivery fallback reasons keyed by backend enum values. */
export const deliveryReasonLabels: Record<string, string> = {
  delivery_failed: "自动插入失败，文字已保存，可复制",
  delivery_cancelled: "听写已取消，结果已保存在历史记录中",
  history_save_failed: "无法保存到历史记录，请检查存储空间后重试",
  history_save_failed_recovery_kept: "历史记录保存失败，录音已保留，可稍后恢复",
  accessibility_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  browser_permission_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  secure_input: "安全输入阻止自动粘贴，文字已复制；请手动粘贴",
  input_unavailable: "未找到可编辑输入框，文字已复制到剪贴板，请手动粘贴",
  input_changed: "输入框已变化，文字已复制到剪贴板，请手动粘贴",
  target_changed: "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
  target_unavailable: "未能确认输入目标，文字已复制到剪贴板，请手动粘贴",
  paste_failed: "自动粘贴未完成，文字已复制到剪贴板，请手动粘贴",
  retry_clipboard_only: "自动插入失败，文字已复制到剪贴板",
  paste_unverified: "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
  selected_action_clipboard_fallback: "目标变化，结果已复制",
  clipboard_changed: "剪贴板已被其他应用更新，已保留较新的内容；请先检查输入框，再从历史记录复制",
  clipboard_ownership_unverified: "无法确认剪贴板内容；请先检查输入框，再从历史记录复制",
  clipboard_unavailable: "无法安全使用剪贴板，听写文字已保存在历史记录中",
  clipboard_write_failed: "无法准备剪贴板，听写文字已保存在历史记录中",
  keyboard_paste_failed: "自动粘贴状态未确认，请先检查输入框；听写文字已保存在历史记录中",
  paste_mutation_uncertain: "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
};

const genericFallbackMessage = "处理失败，请检查结果或重试";

/** Keep the next step legible on the HUD; History retains the full reason. */
const deliveryReasonHudLabels: Record<string, string> = {
  delivery_failed: "文字已保存，请从历史记录复制",
  delivery_cancelled: "已取消，结果在历史记录",
  history_save_failed: "检查存储空间后重试",
  history_save_failed_recovery_kept: "录音已保留，可稍后恢复",
  accessibility_required: "已复制，请手动粘贴",
  browser_permission_required: "已复制，请手动粘贴",
  secure_input: "已复制，请手动粘贴",
  input_unavailable: "已复制，请手动粘贴",
  input_changed: "已复制，请手动粘贴",
  target_changed: "已复制，请手动粘贴",
  target_unavailable: "已复制，请手动粘贴",
  paste_failed: "已复制，请手动粘贴",
  retry_clipboard_only: "已复制，请手动粘贴",
  paste_unverified: "先检查输入框；需要时从历史记录复制",
  selected_action_clipboard_fallback: "已复制，请手动粘贴",
  clipboard_changed: "先检查输入框；需要时从历史记录复制",
  clipboard_ownership_unverified: "先检查输入框；需要时从历史记录复制",
  clipboard_unavailable: "文字已保存，请从历史记录复制",
  clipboard_write_failed: "文字已保存，请从历史记录复制",
  keyboard_paste_failed: "先检查输入框；需要时从历史记录复制",
  paste_mutation_uncertain: "先检查输入框；需要时从历史记录复制",
};

export function deliveryReasonHudMessage(
  reason: string | null | undefined,
  translate: (source: string) => string,
): string | null {
  if (!reason) return null;
  return translate(deliveryReasonHudLabels[reason] ?? genericFallbackMessage);
}

export function deliveryReasonMessage(
  reason: string | null | undefined,
  translate: (source: string) => string,
): string | null {
  if (!reason) return null;
  const label = deliveryReasonLabels[reason];
  if (label) return translate(label);
  return translate(genericFallbackMessage);
}
