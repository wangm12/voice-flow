import { deliveryReasonHudMessage } from "../../lib/deliveryCopy";

export const voicePillHeight = 40;
// Keep the transparent native surface close to the visible HUD. A large
// transparent WebView still participates in WindowServer composition.
export const voicePillWindowWidth = 172;
export const voicePillWindowWidthWithPartial = 400;
export const voicePillWindowHeight = 68;
export const voicePillCaptionHeight = 18;
export const voicePillWarningCaptionHeight = 42;
export const voicePillCaptionGap = 4;
export const voicePillLearnToastHeight = 38;
export const voicePillStagePaddingTop =
  voicePillWindowHeight - voicePillHeight - voicePillCaptionGap - voicePillCaptionHeight;
export const voicePillCaptionMaxWidth = 164;
export const voicePillCaptionMaxWidthWithPartial = 360;

export function voicePillWindowWidthForPartial(hasPartial: boolean): number {
  return hasPartial ? voicePillWindowWidthWithPartial : voicePillWindowWidth;
}

export function voicePillCaptionMaxWidthForPartial(hasPartial: boolean): number {
  return hasPartial ? voicePillCaptionMaxWidthWithPartial : voicePillCaptionMaxWidth;
}

/** Compact fixed width; state labels ellipsize inside the center slot. */
export function voicePillWidthForState(state: string): number {
  void state;
  return 132;
}

export const selectedActionCaptionLabels: Record<string, string> = {
  waiting_for_selection: "等待选中文本",
  listening: "正在听取操作",
  preparing_rewrite: "正在准备改写",
  accessibility_required: "需要辅助功能权限",
  replaced: "已替换选中文本",
  copied_instead: "目标变化，结果已复制",
};

export function selectedActionCaption(
  state: string | null | undefined,
  translate: (source: string) => string = (source) => source,
): string | null {
  if (!state) return null;
  const label = selectedActionCaptionLabels[state];
  return label ? translate(label) : null;
}

type PillCaptionOptions = {
  fallbackReason?: string | null;
  selectedActionState?: string | null;
  contextLabel?: string | null;
  partialText?: string | null;
  phase?: string | null;
  chunkProgress?: {
    completed: number;
    total: number;
  } | null;
};

const CONTEXT_LABEL_STATES = new Set(["recording", "recording_limited", "starting", "processing"]);

/** How long the App · template chip stays before it dismisses. */
export const CONTEXT_LABEL_VISIBLE_MS = 2_500;

export function visibleContextLabel(
  contextLabel: string | null | undefined,
  shownAtMs: number | null,
  nowMs: number,
  ttlMs = CONTEXT_LABEL_VISIBLE_MS,
): string | null {
  const label = contextLabel?.trim() || null;
  if (!label || shownAtMs == null) return null;
  if (nowMs - shownAtMs >= ttlMs) return null;
  return label;
}

export function voicePillCaptionNeedsWide(
  state: string,
  options?: Pick<PillCaptionOptions, "fallbackReason" | "partialText" | "phase">,
): boolean {
  if (options?.fallbackReason) return true;
  if (options?.phase === "soniox_recovery") return true;
  return ["error", "degraded", "copied", "unverified", "rate_limited", "recording_limited"].includes(state);
}

export function pillCaption(
  state: string,
  translate: (source: string) => string = (source) => source,
  retryAfterSecs?: number | null,
  options?: PillCaptionOptions,
): string | null {
  switch (state) {
    case "rate_limited": {
      const base = translate("处理时间比平时长…");
      if (retryAfterSecs != null && retryAfterSecs > 0) {
        return `${base} ${translate("约 {n} 秒后重试").replace("{n}", String(retryAfterSecs))}`;
      }
      return base;
    }
    case "degraded": {
      const fallbackCaption = deliveryReasonHudMessage(options?.fallbackReason, translate);
      if (fallbackCaption) return fallbackCaption;
      return translate("部分结果已保存，请检查后再使用");
    }
    case "error":
      return deliveryReasonHudMessage(options?.fallbackReason, translate) ?? translate("语音输入失败，请重试");
    default:
      break;
  }

  const fallbackCaption = deliveryReasonHudMessage(options?.fallbackReason, translate);
  if (fallbackCaption) return fallbackCaption;

  let statusCaption: string | null = null;
  if (state === "recording_limited") {
    statusCaption = translate("已达上限 · 按热键结束");
  }

  const selectedState = options?.selectedActionState;
  if (!statusCaption && state === "processing" && options?.phase === "cascade_accurate") {
    statusCaption = translate("精确重打中");
  }
  if (!statusCaption && state === "processing" && options?.phase === "soniox_recovery") {
    statusCaption = translate("正在重新转写完整录音");
  }
  if (!statusCaption && state === "processing" && selectedState === "preparing_rewrite") {
    statusCaption = selectedActionCaption("preparing_rewrite", translate);
  }
  if (!statusCaption && state === "recording" && selectedState === "listening") {
    statusCaption = selectedActionCaption("listening", translate);
  }
  if (!statusCaption && state === "idle" && selectedState) {
    statusCaption = selectedActionCaption(selectedState, translate);
  }
  if (!statusCaption && selectedState && ["copied", "done", "unverified"].includes(state)) {
    statusCaption = selectedActionCaption(selectedState, translate);
  }
  if (!statusCaption && state === "unverified") {
    statusCaption = translate("先检查输入框；需要时从历史记录复制");
  }
  if (!statusCaption && state === "copied") {
    statusCaption = translate("已复制到剪贴板，请手动粘贴");
  }

  const contextLabel = CONTEXT_LABEL_STATES.has(state) ? options?.contextLabel?.trim() || null : null;
  if (statusCaption) {
    if (contextLabel) {
      // Keep the stop instruction first even when an app/template name is long.
      return state === "recording_limited"
        ? `${statusCaption} · ${contextLabel}`
        : `${contextLabel} · ${statusCaption}`;
    }
    return statusCaption;
  }

  if (contextLabel) return contextLabel;
  return statusCaption;
}

export function hasSelectedActionCaption(
  selectedActionState: string | null | undefined,
): boolean {
  return Boolean(selectedActionState && selectedActionCaptionLabels[selectedActionState]);
}
