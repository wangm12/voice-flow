/** Human-readable delivery fallback reasons keyed by backend enum values. */
export const deliveryReasonLabels: Record<string, string> = {
  critical_memory_pressure: "系统内存压力过高，未运行本地整理；已保留 ASR 原稿",
  local_memory_unavailable: "本机内存预算不足，未运行本地整理；已保留 ASR 原稿",
  delivery_failed: "自动插入失败，文字已保存，可复制",
  delivery_cancelled: "听写已取消，结果已保存在历史记录中",
  history_save_failed: "无法保存到历史记录，请检查存储空间后重试",
  history_save_failed_recovery_kept: "历史记录保存失败，录音已保留，可稍后恢复",
  processing_timeout: "处理超时，请检查本次任务状态",
  local_processing_timeout: "本地处理超时，请检查本次任务状态",
  onboarding_required: "设置尚未完成，请检查语音服务配置",
  microphone_required: "没有可用的麦克风权限，请检查系统设置",
  audio_error: "麦克风输入中断，请检查所选设备后重试",
  closed_lid_builtin_input_unavailable: "合上显示屏时内置麦克风不可用，请打开显示屏或选择外接麦克风。",
  no_speech: "未检测到声音，请检查麦克风后重试",
  accessibility_required: "开启辅助功能权限后才能插入",
  browser_permission_required: "允许浏览器访问后才能安全插入",
  secure_input: "安全输入阻止自动粘贴",
  input_unavailable: "请将光标放在可编辑输入框中",
  input_changed: "输入框已变化",
  target_changed: "目标已变化",
  target_unavailable: "无法确认输入目标",
  paste_failed: "自动插入未完成",
  retry_clipboard_only: "历史重试已完成，结果已复制到剪贴板（仅复制）",
  retry_clipboard_changed: "剪贴板在重试期间已更新，保留了当前内容；转写结果已保存在历史记录，可手动复制",
  retry_clipboard_ownership_unverified: "无法确认剪贴板在重试期间是否变化；转写结果已保存在历史记录，可手动复制",
  retry_clipboard_write_failed: "复制状态未能确认；转写结果已保存在历史记录，请检查剪贴板或从历史记录复制",
  reclean_clipboard_only: "重新整理结果已复制到剪贴板",
  reclean_clipboard_changed: "重新整理期间剪贴板已更新，保留了较新内容；结果已保存在历史记录，可手动复制",
  reclean_clipboard_ownership_unverified: "无法确认重新整理期间剪贴板是否变化；结果已保存在历史记录，可手动复制",
  reclean_clipboard_write_failed: "复制状态未能确认；重新整理结果已保存在历史记录，请手动复制",
  reclean_copy_not_attempted: "重新整理结果已保存在历史记录；未尝试更新剪贴板",
  paste_unverified: "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
  paste_shortcut_outcome_unknown: "已尝试触发粘贴，但系统未返回可确认结果；请检查输入框和历史记录，未重复粘贴",
  paste_expected_splice_unavailable: "已尝试粘贴，但无法构造输入框核对结果；请检查输入框和历史记录，未重复粘贴",
  paste_target_unconfirmed_after_attempt: "已尝试粘贴，但无法确认目标应用在回读时的状态；请检查输入框和历史记录，未重复粘贴",
  paste_focus_unconfirmed_after_attempt: "已尝试粘贴，但无法确认输入框焦点在回读时的状态；请检查输入框和历史记录，未重复粘贴",
  // Compatibility for existing diagnostics written before target/focus read
  // failures were kept distinct from observed changes.
  paste_target_changed_after_attempt: "已尝试粘贴，但无法确认目标应用在回读时的状态；请检查输入框和历史记录，未重复粘贴",
  paste_focus_changed_after_attempt: "已尝试粘贴，但无法确认输入框焦点在回读时的状态；请检查输入框和历史记录，未重复粘贴",
  paste_readback_unavailable: "已尝试粘贴，但无法读取输入框结果；请检查输入框和历史记录，未重复粘贴",
  paste_readback_mismatch: "已尝试粘贴，但本次输入框读回结果与预期不同；请检查输入框和历史记录，未重复粘贴",
  selected_action_clipboard_fallback: "目标变化，结果已复制",
  clipboard_changed: "剪贴板已被其他应用更新，已保留较新的内容；请先检查输入框，再从历史记录复制",
  clipboard_ownership_unverified: "无法确认剪贴板内容；请先检查输入框，再从历史记录复制",
  clipboard_unavailable: "剪贴板不可用",
  clipboard_write_failed: "剪贴板写入失败",
  keyboard_paste_failed: "自动粘贴未完成",
  paste_mutation_uncertain: "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
};

const genericFallbackMessage = "处理失败，请检查结果或重试";

const closedLidMicrophoneErrorMarker =
  "selected input device is unavailable while the lid is closed";

export function closedLidMicrophoneErrorMessage(
  message: string,
  translate: (source: string) => string,
): string | null {
  return message.includes(closedLidMicrophoneErrorMarker)
    ? translate(deliveryReasonLabels.closed_lid_builtin_input_unavailable)
    : null;
}

/** Keep the next step legible on the HUD; History retains the full reason. */
export const deliveryReasonHudLabels: Record<string, string> = {
  critical_memory_pressure: "内存压力过高；已保留 ASR 原稿",
  local_memory_unavailable: "内存预算不足；已保留 ASR 原稿",
  delivery_failed: "自动输入未完成",
  delivery_cancelled: "听写已取消",
  history_save_failed: "历史记录暂不可用",
  history_save_failed_recovery_kept: "录音仍在本机保留",
  processing_timeout: "处理超时；检查本次任务",
  local_processing_timeout: "本地处理超时；检查本次任务",
  onboarding_required: "检查语音服务设置",
  microphone_required: "请检查麦克风权限",
  audio_error: "检查麦克风设备后重试",
  closed_lid_builtin_input_unavailable: "打开显示屏或选择外接麦克风",
  no_speech: "未检测到声音；检查麦克风",
  accessibility_required: "需要辅助功能权限",
  browser_permission_required: "允许浏览器访问后才能安全插入",
  secure_input: "安全输入阻止自动粘贴",
  input_unavailable: "请将光标放在可编辑输入框中",
  input_changed: "输入框已变化",
  target_changed: "目标已变化",
  target_unavailable: "无法确认输入目标",
  paste_failed: "自动插入未完成",
  retry_clipboard_only: "重试已复制，可手动粘贴",
  retry_clipboard_changed: "剪贴板有变化；先检查输入框",
  retry_clipboard_ownership_unverified: "复制状态未确认；先检查输入框",
  retry_clipboard_write_failed: "复制状态未确认",
  reclean_clipboard_only: "整理结果已复制",
  reclean_clipboard_changed: "剪贴板有变化；先检查输入框",
  reclean_clipboard_ownership_unverified: "复制状态未确认；先检查输入框",
  reclean_clipboard_write_failed: "复制状态未确认",
  reclean_copy_not_attempted: "整理完成；未更新剪贴板",
  paste_unverified: "先检查输入框",
  paste_shortcut_outcome_unknown: "先检查输入框",
  paste_expected_splice_unavailable: "先检查输入框",
  paste_target_unconfirmed_after_attempt: "先检查输入框",
  paste_focus_unconfirmed_after_attempt: "先检查输入框",
  paste_target_changed_after_attempt: "先检查输入框",
  paste_focus_changed_after_attempt: "先检查输入框",
  paste_readback_unavailable: "先检查输入框",
  paste_readback_mismatch: "先检查输入框",
  selected_action_clipboard_fallback: "目标变化；检查结果",
  clipboard_changed: "剪贴板有变化；先检查输入框",
  clipboard_ownership_unverified: "复制状态未确认；先检查输入框",
  clipboard_unavailable: "剪贴板不可用",
  clipboard_write_failed: "剪贴板写入失败",
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
