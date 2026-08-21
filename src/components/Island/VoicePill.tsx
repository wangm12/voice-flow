import { memo, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Archive, ArrowUp, Check, CircleAlert, Clipboard, Clock3, Sparkles, Square, Undo2, X } from "lucide-react";
import { VoiceWaveform } from "./VoiceWaveform";
import { IconButton } from "../IconButton";
import { useI18n } from "../../lib/i18n";
import { pillCaption, hasSelectedActionCaption, voicePillCaptionMaxWidthForPartial } from "./voicePillTokens";

type DictationState = "idle" | "starting" | "recording" | "recording_limited" | "processing" | "rate_limited" | "done" | "unverified" | "copied" | "degraded" | "history" | "error";
type ProcessingPhase = "finalizing_audio" | "asr" | "cleanup" | "delivery" | "waiting_retry" | "idle";

function stateAriaLabel(state: DictationState, t: (source: string) => string): string {
  switch (state) {
    case "starting":
    case "processing":
    case "rate_limited":
    case "recording_limited":
      return t("加载中…");
    case "recording":
      return t("录音中");
    case "done":
    case "copied":
      return t("已完成");
    case "history":
      return t("已保存到历史");
    case "unverified":
      return t("已尝试写入，请确认输入框内容");
    case "degraded":
      return t("部分结果");
    case "error":
      return t("错误");
    default:
      return t("VoiceFlow");
  }
}

export const VoicePill = memo(function VoicePill({
  state,
  phase = "idle",
  retryAfterSecs = null,
  undoAvailable = false,
  contextLabel,
  fallbackReason = null,
  selectedActionState = null,
  waveformLevels,
  progress,
  chunkProgress,
  partialText = null,
  reduced,
}: {
  state: DictationState;
  phase?: ProcessingPhase;
  retryAfterSecs?: number | null;
  undoAvailable?: boolean;
  contextLabel: string | null;
  fallbackReason?: string | null;
  selectedActionState?: string | null;
  waveformLevels: number[];
  progress: number;
  chunkProgress?: {
    completed: number;
    total: number;
  } | null;
  partialText?: string | null;
  reduced: boolean;
}) {
  const { t } = useI18n();
  const isStarting = state === "starting";
  const isTerminal = state === "done" || state === "copied" || state === "history";
  const visualState = isStarting ? "recording" : state;
  const isLoading = ["processing", "rate_limited"].includes(state);
  const isStatus = state === "degraded" || state === "error";
  const showWaveform = visualState === "recording" || state === "recording_limited";
  const [undoBusy, setUndoBusy] = useState(false);
  const [retryRemaining, setRetryRemaining] = useState(retryAfterSecs);
  useEffect(() => {
    setRetryRemaining(retryAfterSecs);
  }, [retryAfterSecs, state]);
  useEffect(() => {
    if (state !== "rate_limited" || retryAfterSecs == null || retryAfterSecs <= 0) return;
    const timer = window.setInterval(() => {
      setRetryRemaining((current) => (current == null || current <= 1 ? 0 : current - 1));
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [state, retryAfterSecs]);
  const canCancel = ["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(state);
  const canStop = isStarting || state === "recording" || state === "recording_limited";
  const canCancelProcessing = state === "processing" || state === "rate_limited";
  const progressValue = Math.max(0, Math.min(1, progress));
  const showProgress = isLoading;
  const indeterminateProgress = showProgress && state !== "processing";
  const caption = pillCaption(
    state,
    t,
    state === "rate_limited" ? retryRemaining : retryAfterSecs,
    { fallbackReason, selectedActionState, chunkProgress, contextLabel, partialText },
  );
  const showingPartial = Boolean(partialText?.trim())
    && ["recording", "recording_limited", "starting", "processing"].includes(state);
  const stackHidden = state === "idle" && !caption;
  const showStackExit = isTerminal && !(caption && hasSelectedActionCaption(selectedActionState));
  const stackClassName = [
    "voice-pill-stack",
    stackHidden ? "voice-pill-stack--hidden" : "",
    showStackExit ? "voice-pill-stack--exit" : "",
    reduced ? "voice-pill-stack--reduced" : "",
  ].filter(Boolean).join(" ");
  const progressClassName = [
    "voice-pill__progress",
    showProgress ? "voice-pill__progress--visible" : "",
    indeterminateProgress ? "voice-pill__progress--indeterminate" : "",
  ].filter(Boolean).join(" ");

  function cancelDictation() {
    void invoke("cancel_dictation").catch((error) => {
      console.error("Failed to cancel dictation", error);
    });
  }

  function stopDictation() {
    void invoke(isStarting ? "cancel_dictation" : "stop_dictation").catch((error) => {
      console.error("Failed to stop dictation", error);
    });
  }
  async function undoDelivery() {
    if (undoBusy) return;
    setUndoBusy(true);
    try {
      const result = await invoke<string>("undo_last_delivery");
      if (result !== "success") {
        console.warn("Undo insertion unavailable", result);
      }
    } catch (error) {
      console.error("Failed to undo delivery", error);
    } finally {
      setUndoBusy(false);
    }
  }
  const captionTone = state === "error"
    ? "error"
    : state === "degraded" || fallbackReason || selectedActionState === "accessibility_required"
      ? "warning"
      : "status";
  const statusLabel = stateAriaLabel(state, t);
  const liveLabel = caption ? `${statusLabel}。${caption}` : statusLabel;
  const pillClassName = [
    "voice-pill",
    `voice-pill--${visualState}`,
    reduced ? "voice-pill--reduced" : "",
  ].filter(Boolean).join(" ");

  return (
    <div className={stackClassName} aria-hidden={stackHidden}>
      <div className={pillClassName} role="status" aria-label={liveLabel} aria-live="polite">
        <span
          className={progressClassName}
          style={indeterminateProgress ? undefined : { transform: `scaleX(${progressValue})` }}
          aria-hidden="true"
        />
        {canCancel && (
          <IconButton
            unstyled
            tooltip={false}
            label={t("取消录音")}
            className="voice-pill__side voice-pill__side--cancel"
            onClick={cancelDictation}
            icon={<X size={13} strokeWidth={2.25} absoluteStrokeWidth aria-hidden="true" />}
          />
        )}
        <div className="voice-pill__content">
          <span className="voice-pill__center-region">
            <span
              className={`voice-pill__center-state${isLoading ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={!isLoading}
            >
              {phase === "cleanup" ? <Sparkles size={14} strokeWidth={2.2} aria-hidden="true" />
                : phase === "delivery" ? <ArrowUp size={14} strokeWidth={2.35} aria-hidden="true" />
                : phase === "waiting_retry" ? <Clock3 size={14} strokeWidth={2.2} aria-hidden="true" />
                : <span className="voice-pill__dots voice-pill__dots--thinking"><i /><i /><i /></span>}
            </span>
            <span
              className={`voice-pill__center-state${showWaveform ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={!showWaveform}
            >
              <VoiceWaveform
                active={visualState === "recording"}
                dim={state === "recording_limited"}
                levels={visualState === "recording" ? waveformLevels : undefined}
              />
            </span>
            <span
              className={`voice-pill__center-state${state === "done" ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={state !== "done"}
            >
              <Check className="voice-pill__terminal-icon" size={14} strokeWidth={2.5} absoluteStrokeWidth aria-hidden="true" />
            </span>
            <span
              className={`voice-pill__center-state${state === "unverified" ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={state !== "unverified"}
            >
              <Check className="voice-pill__caution-icon" size={14} strokeWidth={2.5} absoluteStrokeWidth aria-hidden="true" />
            </span>
            <span
              className={`voice-pill__center-state${state === "copied" ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={state !== "copied"}
            >
              <Clipboard className="voice-pill__terminal-icon" size={14} strokeWidth={2.3} absoluteStrokeWidth aria-hidden="true" />
            </span>
            <span
              className={`voice-pill__center-state${state === "history" ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={state !== "history"}
            >
              <Archive className="voice-pill__terminal-icon" size={14} strokeWidth={2.3} absoluteStrokeWidth aria-hidden="true" />
            </span>
            <span
              className={`voice-pill__center-state${isStatus ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={!isStatus}
            >
              <CircleAlert className="voice-pill__status-icon" size={14} strokeWidth={2.25} absoluteStrokeWidth aria-hidden="true" />
            </span>
          </span>
        </div>
        {(canStop || canCancelProcessing) && (
          <IconButton
            unstyled
            tooltip={false}
            label={isStarting ? t("取消录音") : canStop ? t("停止录音") : t("取消转写")}
            className="voice-pill__side voice-pill__side--action"
            onClick={canStop ? stopDictation : cancelDictation}
            icon={canCancelProcessing
              ? <Square size={12} strokeWidth={2.5} absoluteStrokeWidth aria-hidden="true" />
              : <ArrowUp size={14} strokeWidth={2.35} absoluteStrokeWidth aria-hidden="true" />}
          />
        )}
        {undoAvailable && !canStop && !canCancelProcessing && (
          <IconButton
            unstyled
            tooltip={false}
            label={t("撤销插入")}
            className="voice-pill__side voice-pill__side--action"
            disabled={undoBusy}
            onClick={() => void undoDelivery()}
            icon={<Undo2 size={13} strokeWidth={2.35} absoluteStrokeWidth aria-hidden="true" />}
          />
        )}
      </div>
      {caption && (
        <p
          className={[
            "voice-pill-caption",
            `voice-pill-caption--${captionTone}`,
            showingPartial ? "voice-pill-caption--partial" : "",
          ].filter(Boolean).join(" ")}
          style={showingPartial ? { maxWidth: voicePillCaptionMaxWidthForPartial(true) } : undefined}
        >
          <span className="voice-pill-caption__dot" aria-hidden="true" />
          <span className="voice-pill-caption__text">{caption}</span>
        </p>
      )}
    </div>
  );
});
