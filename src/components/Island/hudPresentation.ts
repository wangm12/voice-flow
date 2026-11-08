export type HudIconName =
  | "mic"
  | "mic-off"
  | "archive"
  | "clipboard"
  | "cursor"
  | "target"
  | "lock"
  | "shield"
  | "document"
  | "clock"
  | "model"
  | "service"
  | "dictionary"
  | "notice";

export type HudDestination = "history" | "microphone" | "permissions" | "models" | "service";

export type HudDetailsAction = {
  destination: HudDestination;
  label: string;
};

export type HudPresentation = {
  title: string | null;
  subtitle: string | null;
  detail: string | null;
  icon: HudIconName;
  action: HudDetailsAction | null;
};

type Translate = (source: string) => string;

const FALLBACK_PRESENTATIONS: Record<string, Omit<HudPresentation, "detail">> = {
  critical_memory_pressure: { title: "已保留 ASR 原稿", subtitle: "系统内存压力过高，未运行本地整理", icon: "document", action: { destination: "history", label: "检查历史记录" } },
  local_memory_unavailable: { title: "已保留 ASR 原稿", subtitle: "本机内存预算不足，未运行本地整理", icon: "document", action: { destination: "history", label: "检查历史记录" } },
  microphone_required: { title: "需要麦克风权限", subtitle: "在系统权限中开启", icon: "lock", action: { destination: "microphone", label: "打开麦克风设置" } },
  audio_error: { title: "麦克风已中断", subtitle: "检查输入设备", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
  closed_lid_builtin_input_unavailable: { title: "内置麦克风不可用", subtitle: "打开显示屏或选择外接麦克风", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
  no_speech: { title: "没有检测到语音", subtitle: "检查麦克风后重试", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
  accessibility_required: { title: "需要辅助功能权限", subtitle: "开启后可自动输入", icon: "lock", action: { destination: "permissions", label: "打开权限设置" } },
  browser_permission_required: { title: "需要浏览器权限", subtitle: "检查浏览器访问权限", icon: "lock", action: { destination: "permissions", label: "查看权限设置" } },
  secure_input: { title: "无法在此处自动输入", subtitle: "安全输入框会阻止粘贴", icon: "lock", action: { destination: "history", label: "检查历史记录" } },
  input_unavailable: { title: "请先选择输入框", subtitle: "将光标放在可编辑位置", icon: "cursor", action: { destination: "history", label: "检查历史记录" } },
  input_changed: { title: "输入框已变化", subtitle: "请先检查当前内容", icon: "target", action: { destination: "history", label: "检查历史记录" } },
  target_changed: { title: "输入目标已变化", subtitle: "先检查输入框内容", icon: "target", action: { destination: "history", label: "检查历史记录" } },
  target_unavailable: { title: "无法确认输入目标", subtitle: "请检查目标和结果", icon: "target", action: { destination: "history", label: "检查历史记录" } },
  delivery_failed: { title: "没有完成输入", subtitle: "检查目标后再手动继续", icon: "cursor", action: { destination: "history", label: "检查历史记录" } },
  paste_failed: { title: "没有完成输入", subtitle: "检查目标后再手动继续", icon: "cursor", action: { destination: "history", label: "检查历史记录" } },
  keyboard_paste_failed: { title: "没有完成输入", subtitle: "检查目标后再手动继续", icon: "cursor", action: { destination: "history", label: "检查历史记录" } },
  paste_unverified: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_shortcut_outcome_unknown: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_expected_splice_unavailable: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_target_unconfirmed_after_attempt: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_focus_unconfirmed_after_attempt: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_target_changed_after_attempt: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_focus_changed_after_attempt: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_readback_unavailable: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_readback_mismatch: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  paste_mutation_uncertain: { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
  retry_clipboard_only: { title: "文字已复制", subtitle: "请检查目标后手动粘贴", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } },
  reclean_clipboard_only: { title: "文字已复制", subtitle: "请检查目标后手动粘贴", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } },
  selected_action_clipboard_fallback: { title: "修改结果已复制", subtitle: "原选区已变化；请先检查目标", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } },
  clipboard_changed: { title: "保留了较新的剪贴板", subtitle: "先检查输入框，需要时查看历史", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  retry_clipboard_changed: { title: "保留了较新的剪贴板", subtitle: "先检查输入框，需要时查看历史", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  reclean_clipboard_changed: { title: "保留了较新的剪贴板", subtitle: "先检查输入框，需要时查看历史", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  clipboard_ownership_unverified: { title: "复制状态未确认", subtitle: "先检查输入框和剪贴板", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  retry_clipboard_ownership_unverified: { title: "复制状态未确认", subtitle: "先检查输入框和剪贴板", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  reclean_clipboard_ownership_unverified: { title: "复制状态未确认", subtitle: "先检查输入框和剪贴板", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  clipboard_write_failed: { title: "暂时无法复制", subtitle: "需要时从历史记录检查结果", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  clipboard_unavailable: { title: "暂时无法复制", subtitle: "需要时从历史记录检查结果", icon: "clipboard", action: { destination: "history", label: "检查历史记录" } },
  history_save_failed: { title: "无法保存到历史", subtitle: "检查可用空间", icon: "archive", action: { destination: "history", label: "打开历史记录" } },
  history_save_failed_recovery_kept: { title: "录音仍可恢复", subtitle: "恢复音频保留在本机", icon: "archive", action: { destination: "history", label: "打开历史记录" } },
  processing_timeout: { title: "处理超时", subtitle: "请检查本次任务状态", icon: "clock", action: { destination: "history", label: "检查历史记录" } },
  local_processing_timeout: { title: "本地处理超时", subtitle: "请检查本次任务状态", icon: "clock", action: { destination: "history", label: "检查历史记录" } },
  onboarding_required: { title: "还需要完成设置", subtitle: "检查语音服务配置", icon: "service", action: { destination: "service", label: "打开语音服务" } },
  llm_cleanup_failed: { title: "整理服务未完成", subtitle: "请检查结果后再继续", icon: "service", action: { destination: "service", label: "打开语音服务" } },
  llm_cleanup_empty: { title: "整理服务未完成", subtitle: "请检查结果后再继续", icon: "service", action: { destination: "service", label: "打开语音服务" } },
  partial_asr_failure: { title: "仅有部分结果", subtitle: "检查本次识别结果", icon: "document", action: { destination: "history", label: "检查历史记录" } },
  partial_asr_and_llm_failure: { title: "仅有部分结果", subtitle: "检查本次识别结果", icon: "document", action: { destination: "history", label: "检查历史记录" } },
  preservation_guard: { title: "整理结果未通过检查", subtitle: "未采用未通过检查的候选", icon: "shield", action: { destination: "history", label: "检查历史记录" } },
  dictation_error: { title: "听写未能完成", subtitle: "请检查恢复信息", icon: "notice", action: { destination: "history", label: "检查历史记录" } },
};

const PASTE_UNVERIFIED_REASONS = new Set([
  "paste_unverified",
  "paste_shortcut_outcome_unknown",
  "paste_expected_splice_unavailable",
  "paste_target_unconfirmed_after_attempt",
  "paste_focus_unconfirmed_after_attempt",
  "paste_target_changed_after_attempt",
  "paste_focus_changed_after_attempt",
  "paste_readback_unavailable",
  "paste_readback_mismatch",
  "paste_mutation_uncertain",
]);

function reasonPresentation(reason: string): Omit<HudPresentation, "detail"> | null {
  const direct = FALLBACK_PRESENTATIONS[reason];
  if (direct) return direct;
  if (PASTE_UNVERIFIED_REASONS.has(reason)) return FALLBACK_PRESENTATIONS.paste_unverified;
  if (reason.startsWith("paste_")) return FALLBACK_PRESENTATIONS.paste_unverified;
  if (reason.startsWith("clipboard_") || reason.startsWith("retry_clipboard_") || reason.startsWith("reclean_clipboard_")) {
    return FALLBACK_PRESENTATIONS.clipboard_ownership_unverified;
  }
  if (["api_key_required", "unauthorized", "network", "timeout", "rate_limited", "server"].includes(reason)) {
    return { title: "语音服务暂不可用", subtitle: "查看服务配置或稍后重试", icon: "service", action: { destination: "service", label: "打开语音服务" } };
  }
  if (["model_missing", "model_mismatch", "local_native", "local_native_model_missing", "local_native_model_mismatch"].includes(reason)) {
    return { title: "本地模型暂不可用", subtitle: "检查模型状态后重试", icon: "model", action: { destination: "models", label: "打开模型设置" } };
  }
  if (reason === "guard") return FALLBACK_PRESENTATIONS.preservation_guard;
  return null;
}

function capturePresentation(status: string): Omit<HudPresentation, "detail"> | null {
  const entries: Record<string, Omit<HudPresentation, "detail">> = {
    waiting_first_frame: { title: "等待麦克风", subtitle: "尚未收到音频", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
    no_audio_yet: { title: "麦克风尚无输入", subtitle: "检查所选设备", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
    no_recent_audio: { title: "麦克风无响应", subtitle: "检查连接后重试", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
    device_error: { title: "麦克风已中断", subtitle: "检查输入设备", icon: "mic-off", action: { destination: "microphone", label: "检查麦克风" } },
    unavailable: { title: "无法打开麦克风", subtitle: "换一个输入设备", icon: "mic-off", action: { destination: "microphone", label: "选择麦克风" } },
    no_speech: FALLBACK_PRESENTATIONS.no_speech,
  };
  return entries[status] ?? null;
}

export function hudPresentation(
  input: {
    state: string;
    fallbackReason?: string | null;
    deliveryMethod?: string | null;
    cleanupStatus?: string | null;
    captureStatus?: string | null;
    recoveryOutcome?: string | null;
    selectedActionState?: string | null;
  },
  translate: Translate = (source) => source,
): HudPresentation {
  const { state, fallbackReason, deliveryMethod, cleanupStatus, captureStatus, recoveryOutcome, selectedActionState } = input;
  let presentation: Omit<HudPresentation, "detail"> | null = null;
  if (state === "copied" && deliveryMethod === "clipboard" && fallbackReason === "target_changed") {
    presentation = { title: "输入目标已变化", subtitle: "文字已复制", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } };
  } else if (state === "history" && deliveryMethod === "history" && fallbackReason === "target_changed") {
    presentation = { title: "输入目标已变化", subtitle: "结果已存历史", icon: "archive", action: { destination: "history", label: "打开历史记录" } };
  } else if (state === "degraded" && deliveryMethod === "clipboard") {
    presentation = { title: "部分结果已复制", subtitle: "请检查完整录音，再手动粘贴", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } };
  } else if (state === "degraded" && deliveryMethod === "history") {
    presentation = { title: "部分结果已保存到历史", subtitle: "请检查完整录音和历史结果", icon: "archive", action: { destination: "history", label: "打开历史记录" } };
  }
  if (!presentation && recoveryOutcome) {
    const recovery: Record<string, Omit<HudPresentation, "detail">> = {
      history_saved: { title: "录音已保存", subtitle: "可从历史记录继续处理", icon: "archive", action: { destination: "history", label: "查看历史" } },
      audio_retained: { title: "录音仍可恢复", subtitle: "保存到历史后再继续", icon: "archive", action: { destination: "history", label: "保存到历史" } },
      unknown_failure: { title: "录音状态无法确认", subtitle: "请检查历史记录", icon: "notice", action: { destination: "history", label: "检查历史" } },
      no_audio: { title: "无法打开麦克风", subtitle: "更换输入设备后重试", icon: "mic-off", action: { destination: "microphone", label: "选择麦克风" } },
    };
    presentation = recovery[recoveryOutcome] ?? null;
  }
  if (!presentation && state === "unverified" && selectedActionState && selectedActionState !== "replaced") {
    presentation = { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: null };
  }
  if (!presentation && state === "unverified" && deliveryMethod === "paste_unverified" && !selectedActionState) {
    presentation = { ...FALLBACK_PRESENTATIONS.paste_unverified, action: null };
  }
  if (!presentation && fallbackReason) presentation = reasonPresentation(fallbackReason);
  if (!presentation && captureStatus && ["starting", "recording", "recording_limited"].includes(state)) {
    presentation = capturePresentation(captureStatus);
  }
  if (!presentation && selectedActionState === "accessibility_required") {
    presentation = FALLBACK_PRESENTATIONS.accessibility_required;
  }
  if (!presentation && cleanupStatus === "preservation_guard") {
    presentation = FALLBACK_PRESENTATIONS.preservation_guard;
  }
  if (!presentation && cleanupStatus === "critical_memory_pressure") {
    presentation = FALLBACK_PRESENTATIONS.critical_memory_pressure;
  }
  if (!presentation && ["ai_failed_local", "ai_failed_raw"].includes(cleanupStatus ?? "")) {
    presentation = { title: "整理未完成", subtitle: "请检查本次结果", icon: "document", action: { destination: "history", label: "检查历史记录" } };
  }
  if (!presentation) {
    switch (state) {
      case "history": presentation = { title: "已保存到历史", subtitle: "需要时可在历史记录中查看", icon: "archive", action: { destination: "history", label: "查看历史" } }; break;
      case "copied": presentation = { title: "文字已复制", subtitle: "请检查目标后手动粘贴", icon: "clipboard", action: { destination: "history", label: "打开历史记录" } }; break;
      case "unverified": presentation = { title: "已尝试输入", subtitle: "先检查输入框", icon: "notice", action: { destination: "history", label: "检查历史记录" } }; break;
      case "degraded": presentation = { title: "仅有部分结果", subtitle: "检查本次结果后再继续", icon: "document", action: { destination: "history", label: "检查历史记录" } }; break;
      case "error": presentation = { title: "听写未能完成", subtitle: "请检查恢复信息", icon: "notice", action: { destination: "history", label: "检查历史记录" } }; break;
      case "idle":
        if (selectedActionState === "waiting_for_selection") {
          presentation = { title: "先选中一段文字", subtitle: "再说出修改要求", icon: "cursor", action: null };
        }
        break;
    }
  }
  if (!presentation) return { title: null, subtitle: null, detail: null, icon: "mic", action: null };

  return {
    ...presentation,
    title: presentation.title ? translate(presentation.title) : null,
    subtitle: presentation.subtitle ? translate(presentation.subtitle) : null,
    detail: cleanupStatus === "critical_memory_pressure"
      ? translate("系统内存压力过高，未运行本地整理；已保留 ASR 原稿")
      : cleanupStatus === "local_memory_unavailable"
        ? translate("本机内存预算不足，未运行本地整理；已保留 ASR 原稿")
        : null,
    icon: presentation.icon,
    action: presentation.action
      ? { ...presentation.action, label: translate(presentation.action.label) }
      : null,
  };
}
