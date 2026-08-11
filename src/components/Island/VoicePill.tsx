import { memo, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowUp, Check, CircleAlert, Square, X } from "lucide-react";
import { VoiceWaveform } from "./VoiceWaveform";
import { pillCaption } from "./voicePillTokens";
import { IconButton } from "../IconButton";
import { useI18n } from "../../lib/i18n";

type DictationState = "idle" | "starting" | "recording" | "recording_limited" | "processing" | "rate_limited" | "done" | "copied" | "degraded" | "error";

function stateAriaLabel(state: DictationState, t: (source: string) => string): string {
  switch (state) {
    case "starting":
    case "processing":
    case "rate_limited":
    case "recording_limited":
      return t("加载中");
    case "recording":
      return t("录音中");
    case "done":
    case "copied":
      return t("已完成");
    case "degraded":
      return t("部分结果");
    case "error":
      return t("错误");
    default:
      return "VoiceFlow";
  }
}

function fallbackCaption(reason: string, t: (source: string) => string): string {
  switch (reason) {
    case "microphone_required":
      return t("请检查麦克风权限");
    case "api_key_required":
      return t("请先配置 Groq API Key");
    case "onboarding_required":
      return t("请先完成初始设置");
    case "audio_error":
      return t("录音失败，请检查麦克风或输入设备");
    case "no_speech":
      return t("没有检测到语音，请再试一次");
    case "processing_timeout":
      return t("处理超时，请再试一次");
    case "dictation_error":
      return t("语音输入失败，请重试");
    case "accessibility_required":
      return t("开启辅助功能权限后才能插入");
    case "browser_permission_required":
      return t("允许浏览器访问后才能安全插入");
    case "input_unavailable":
      return t("请将光标放在可编辑输入框中");
    case "input_changed":
      return t("输入框已变化，请手动粘贴");
    case "target_changed":
      return t("目标已变化，请手动粘贴");
    case "paste_failed":
      return t("无法自动粘贴，请手动粘贴");
    case "partial_asr_failure":
      return t("部分音频处理失败，请检查结果");
    case "partial_asr_and_llm_failure":
      return t("部分音频处理失败，请检查结果");
    case "partial_llm_cleanup_failure":
      return t("文字整理不可用，已保留原文");
    default:
      return t("已复制到剪贴板，请手动粘贴");
  }
}

function selectedActionCaption(state: string | null, t: (source: string) => string): string | null {
  switch (state) {
    case "waiting_for_selection":
      return t("等待选中文本");
    case "listening":
      return t("正在听取操作");
    case "preparing_rewrite":
      return t("正在准备改写");
    case "ready_to_replace":
      return t("准备替换选中文本");
    case "copied_instead":
      return t("目标变化，结果已复制");
    case "selection_changed":
      return t("选区已变化，结果已复制");
    case "accessibility_required":
      return t("需要辅助功能权限");
    default:
      return null;
  }
}

function targetCaption(contextLabel: string | null): string | null {
  if (!contextLabel) return null;
  return contextLabel.split(" · ", 1)[0]?.trim() || null;
}

export const VoicePill = memo(function VoicePill({
  state,
  contextLabel,
  fallbackReason,
  selectedActionState,
  waveformLevels,
  progress,
  reduced,
}: {
  state: DictationState;
  contextLabel: string | null;
  fallbackReason?: string | null;
  selectedActionState?: string | null;
  waveformLevels: number[];
  progress: number;
  reduced: boolean;
}) {
  const { t } = useI18n();
  const isStarting = state === "starting";
  const isTerminal = state === "done" || state === "copied";
  const visualState = isStarting ? "recording" : state;
  const isLoading = ["processing", "rate_limited", "recording_limited"].includes(state);
  const isStatus = state === "degraded" || state === "error";
  const selectedDetail = selectedActionCaption(selectedActionState ?? null, t);
  const detail = fallbackReason
      ? fallbackCaption(fallbackReason, t)
      : selectedDetail
      ? selectedDetail
      : isTerminal ? null : pillCaption(state, t);
  const caption = isTerminal ? null : detail ?? targetCaption(contextLabel);
  const captionHasStatus = Boolean(detail);
  const [captionVisible, setCaptionVisible] = useState(false);
  const previousCaptionRef = useRef<string | null>(null);
  const previousStateRef = useRef<DictationState>(state);
  const captionTimerRef = useRef<number | null>(null);

  useEffect(() => {
    const enteredActiveState = previousStateRef.current === "idle" && state !== "idle";
    const changed = previousCaptionRef.current !== caption;
    previousCaptionRef.current = caption;
    previousStateRef.current = state;

    if (!caption) {
      if (captionTimerRef.current !== null) {
        window.clearTimeout(captionTimerRef.current);
        captionTimerRef.current = null;
      }
      setCaptionVisible(false);
      return;
    }

    if (captionHasStatus) {
      if (captionTimerRef.current !== null) {
        window.clearTimeout(captionTimerRef.current);
        captionTimerRef.current = null;
      }
      setCaptionVisible(true);
      return;
    }

    if (!enteredActiveState && !changed) return;

    if (captionTimerRef.current !== null) window.clearTimeout(captionTimerRef.current);
    setCaptionVisible(true);
    captionTimerRef.current = window.setTimeout(() => {
      captionTimerRef.current = null;
      setCaptionVisible(false);
    }, 1400);
  }, [caption, captionHasStatus, state]);

  useEffect(() => () => {
    if (captionTimerRef.current !== null) window.clearTimeout(captionTimerRef.current);
  }, []);

  const showCaption = Boolean(caption && (captionHasStatus || captionVisible));
  const captionClassName = [
    "voice-pill-caption",
    showCaption ? "" : "voice-pill-caption--empty",
    captionHasStatus ? "voice-pill-caption--status" : "",
    state === "error" ? "voice-pill-caption--error" : state === "degraded" ? "voice-pill-caption--warning" : "",
  ].filter(Boolean).join(" ");
  const canCancel = ["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(state);
  const canStop = isStarting || state === "recording" || state === "recording_limited";
  const canCancelProcessing = state === "processing" || state === "rate_limited";
  const progressValue = Math.max(0, Math.min(1, progress));
  const showProgress = isLoading;
  const indeterminateProgress = showProgress && state !== "processing";
  const stackClassName = [
    "voice-pill-stack",
    state === "idle" ? "voice-pill-stack--hidden" : "",
    isTerminal ? "voice-pill-stack--exit" : "",
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
  const pillClassName = [
    "voice-pill",
    `voice-pill--${visualState}`,
    reduced ? "voice-pill--reduced" : "",
  ].filter(Boolean).join(" ");

  return (
    <div className={stackClassName} aria-hidden={state === "idle"}>
      <p className={captionClassName} aria-hidden={!showCaption}>
        <span className="voice-pill-caption__dot" />
        <span className="voice-pill-caption__text">{showCaption ? caption : "\u00a0"}</span>
      </p>
      <div className={pillClassName} role="status" aria-label={stateAriaLabel(state, t)} aria-live="polite">
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
              <span className="voice-pill__dots voice-pill__dots--thinking"><i /><i /><i /></span>
            </span>
            <span
              className={`voice-pill__center-state${visualState === "recording" ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={visualState !== "recording"}
            >
              <VoiceWaveform
                active={visualState === "recording"}
                levels={visualState === "recording" ? waveformLevels : undefined}
              />
            </span>
            <span
              className={`voice-pill__center-state${isTerminal ? " voice-pill__center-state--active" : ""}`}
              aria-hidden={!isTerminal}
            >
              <Check className="voice-pill__terminal-icon" size={14} strokeWidth={2.5} absoluteStrokeWidth aria-hidden="true" />
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
      </div>
    </div>
  );
});
