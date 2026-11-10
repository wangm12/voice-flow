import { memo, useEffect, useRef, useState, type CSSProperties } from "react";
import { BorderBeam } from "border-beam";
import { useI18n } from "../../lib/i18n";
import { deliveryReasonHudMessage, deliveryReasonMessage } from "../../lib/deliveryCopy";
import { VoiceHudOrb } from "./VoiceHudOrb";
import {
  HUD_LABEL_FADE_MS,
  HUD_LABEL_IN_DELAY_MS,
  HUD_LABEL_IN_DURATION_MS,
  HUD_LABEL_OUT_MS,
  HUD_MOTION_MS,
  HUD_THINK_BEAM_FADE_MS,
  HUD_PILL_RADIUS_PX,
  HUD_LISTENING_BEAM,
  HUD_THINKING_BEAM,
  formatHudElapsed,
  hudAudioLevel,
  hudBorderBeamFor,
  hudOrbFor,
  hudStatusLabel,
  type DictationState,
  type ProcessingPhase,
} from "./hudOrb";
import { usePageVisible } from "./hudLineBeam";
import { hudPresentation, type HudDestination, type HudIconName } from "./hudPresentation";
import {
  selectedActionCaption,
  voicePillHeight,
  voicePillWidth,
  voicePillWindowHeight,
  voicePillWindowWidth,
  voicePillRecoveryHeight,
  voicePillRecoveryWidth,
} from "./voicePillTokens";

type AudioCaptureStatus = "waiting_first_frame" | "receiving_audio" | "no_audio_yet" | "no_recent_audio" | "device_error" | "unavailable" | "no_speech";

export type VoicePillRecoveryAction = {
  title: string;
  message: string;
  actionLabel: string;
  busy: boolean;
  disabled?: boolean;
  error: string | null;
  onAction: () => void;
  onDismiss: () => void;
};

export type VoicePillAction = "stop" | "cancel";

function HudGlyph({ name, size = 18 }: { name: HudIconName | "arrow" | "close" | "stop"; size?: number }) {
  return (
    <svg
      className="voice-pill__glyph"
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.35"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {name === "stop" ? <rect x="5" y="5" width="10" height="10" rx="1" fill="currentColor" strokeWidth="1.6" />
        : name === "arrow" ? <path d="m8 5 5 5-5 5" />
          : name === "close" ? <path d="m6 6 8 8M14 6l-8 8" />
            : name === "archive" ? <><path d="M3 6.5h14l-1.1 10H4.1L3 6.5Z" /><path d="M2.5 3.5h15v3h-15zM7.5 10.5h5" /></>
              : name === "clipboard" ? <><rect x="5" y="4.5" width="10" height="13" rx="1.5" /><path d="M8 4.5v-2h4v2m-4 4h4m-4 3h4" /></>
                : name === "cursor" ? <path d="M4 2.5v14l4-3 2.2 4 2-1.1-2.1-4.1h5.4L4 2.5Z" />
                  : name === "target" ? <><circle cx="10" cy="10" r="6.5" /><circle cx="10" cy="10" r="2.2" /><path d="M10 1.5v2M18.5 10h-2M10 18.5v-2M1.5 10h2" /></>
                    : name === "lock" ? <><rect x="4.5" y="8.5" width="11" height="9" rx="1.5" /><path d="M7 8.5V6a3 3 0 0 1 6 0v2.5m-3 4v2" /></>
                      : name === "shield" ? <><path d="M10 2.5 16 5v4.3c0 3.8-2.5 6.5-6 8.2-3.5-1.7-6-4.4-6-8.2V5l6-2.5Z" /><path d="m7.5 9.8 1.7 1.7 3.4-3.7" /></>
                        : name === "document" ? <><path d="M5 2.5h6l4 4v11H5z" /><path d="M11 2.5v4h4m-7 3.5h4m-4 3h4" /></>
                          : name === "clock" ? <><circle cx="10" cy="10" r="7" /><path d="M10 5.5V10l3 2" /></>
                            : name === "model" ? <><path d="m10 2.5 6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2L10 2.5Z" /><path d="m3.5 6.2 6.5 3.9 6.5-3.9M10 10v7.5" /></>
                              : name === "service" ? <><path d="M4 13.5a3.5 3.5 0 0 1 .5-7A5.5 5.5 0 0 1 15 7a3.2 3.2 0 0 1 1 6.3" /><path d="M10 10v7m-2.5-2.5L10 17l2.5-2.5" /></>
                                : name === "dictionary" ? <><path d="M4 3.5h9.5A2.5 2.5 0 0 1 16 6v10.5H6.5A2.5 2.5 0 0 1 4 14V3.5Z" /><path d="M4 14a2.5 2.5 0 0 1 2.5-2.5H16M8 7h5" /></>
                                  : name === "notice" ? <><circle cx="10" cy="10" r="7" /><path d="M10 6v4m0 3h.01" /></>
                                    : <>
                                      <rect x="7" y="2.5" width="6" height="10" rx="3" />
                                      <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
                                      {name === "mic-off" && <path d="M3 3 17 17" />}
                                    </>}
    </svg>
  );
}

function stateAriaLabel(state: DictationState, t: (source: string) => string): string {
  switch (state) {
    case "starting": return t("准备录音");
    case "recording": return t("录音中");
    case "recording_limited": return t("录音已达上限");
    case "processing":
    case "rate_limited": return t("处理中");
    case "done": return t("已完成");
    case "copied": return t("已复制");
    case "history": return t("已保存到历史");
    case "unverified": return t("输入状态无法确认，未重复粘贴；请检查输入框和历史记录");
    case "degraded": return t("部分结果");
    case "error": return t("听写未能完成");
    default: return t("VoiceFlow");
  }
}

function recordingAudioIssue(
  status: AudioCaptureStatus | null | undefined,
  translate: (source: string) => string,
): { title: string; detail: string } | null {
  switch (status) {
    case "waiting_first_frame": return { title: translate("等待麦克风"), detail: translate("尚未收到音频") };
    case "no_audio_yet": return { title: translate("麦克风尚无输入"), detail: translate("检查所选设备") };
    case "no_recent_audio": return { title: translate("麦克风无响应"), detail: translate("检查连接后重试") };
    case "device_error": return { title: translate("麦克风已中断"), detail: translate("检查输入设备") };
    case "unavailable": return { title: translate("无法打开麦克风"), detail: translate("换一个输入设备") };
    case "no_speech": return { title: translate("没有检测到语音"), detail: translate("检查麦克风后重试") };
    default: return null;
  }
}

function useHudLabel(next: string | null, reduced: boolean) {
  const [text, setText] = useState(next);
  const [outgoing, setOutgoing] = useState<string | null>(null);
  const [entering, setEntering] = useState(false);

  if (next !== text) {
    if (!reduced && text != null && next != null) {
      setOutgoing(text);
      setEntering(true);
    } else {
      setOutgoing(null);
      setEntering(false);
    }
    setText(next);
  } else if (reduced && (outgoing != null || entering)) {
    setOutgoing(null);
    setEntering(false);
  }

  useEffect(() => {
    if (outgoing == null) return undefined;
    const timer = window.setTimeout(() => setOutgoing(null), HUD_LABEL_OUT_MS);
    return () => window.clearTimeout(timer);
  }, [outgoing, text]);

  useEffect(() => {
    if (!entering) return undefined;
    const timer = window.setTimeout(() => setEntering(false), HUD_LABEL_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [entering, text]);

  return { text, outgoing, entering };
}

const HUD_BEAM_OVERLAY_STYLE: CSSProperties = {
  position: "absolute",
  inset: 0,
  width: "100%",
  height: "100%",
};

export const VoicePill = memo(function VoicePill({
  state,
  phase = "idle",
  phaseConfirmed = false,
  hideImmediately = false,
  stageElapsedMs = 0,
  clockNowMs = 0,
  lastAudioLevelAtMs = null,
  sessionGeneration = 0,
  contextLabel,
  contextSourceLabel = null,
  fallbackReason = null,
  deliveryMethod = null,
  audioStatus = null,
  audioCaptureStatus = null,
  cleanupStatus = null,
  recoveryOutcome = null,
  audioDeviceName = null,
  audioDeviceSourceLabel = null,
  audioDeviceTitle = null,
  recordingTargetIcon = null,
  selectedActionState: rawSelectedActionState = null,
  recoveryAction = null,
  recordingHint = null,
  waveformLevels,
  partialText: _partialText = null,
  reduced,
  detailsOpen = false,
  onDetailsOpenChange,
  onActiveAction,
  actionBusy = false,
  actionError = null,
  detailsActionBusy = false,
  detailsActionError = null,
  onDetailsAction,
}: {
  state: DictationState;
  phase?: ProcessingPhase;
  phaseConfirmed?: boolean;
  hideImmediately?: boolean;
  stageElapsedMs?: number;
  clockNowMs?: number;
  lastAudioLevelAtMs?: number | null;
  sessionGeneration?: number;
  contextLabel: string | null;
  contextSourceLabel?: string | null;
  fallbackReason?: string | null;
  deliveryMethod?: string | null;
  audioStatus?: string | null;
  audioCaptureStatus?: AudioCaptureStatus | null;
  cleanupStatus?: string | null;
  recoveryOutcome?: string | null;
  audioDeviceName?: string | null;
  audioDeviceSourceLabel?: string | null;
  audioDeviceTitle?: string | null;
  recordingTargetIcon?: string | null;
  selectedActionState?: string | null;
  recoveryAction?: VoicePillRecoveryAction | null;
  recordingHint?: string | null;
  waveformLevels: number[];
  partialText?: string | null;
  reduced: boolean;
  detailsOpen?: boolean;
  onDetailsOpenChange?: (open: boolean) => void;
  onActiveAction?: (action: VoicePillAction, expectedGeneration: number) => void;
  actionBusy?: boolean;
  actionError?: string | null;
  detailsActionBusy?: boolean;
  detailsActionError?: string | null;
  onDetailsAction?: (destination: HudDestination, expectedGeneration: number) => void;
}) {
  const { t } = useI18n();
  const pageVisible = usePageVisible();
  // An unverified delivery must never be captioned as a completed replace,
  // even if a `replaced` selected-action state arrived earlier.
  const selectedActionState = state === "unverified" && rawSelectedActionState === "replaced"
    ? null
    : rawSelectedActionState;
  const uncertainPasteAttempt = state === "unverified"
    && deliveryMethod === "paste_unverified"
    && rawSelectedActionState == null;
  const unverifiedTextAction = state === "unverified" && Boolean(selectedActionState);
  const listening = ["starting", "recording", "recording_limited"].includes(state);
  const recording = state === "recording" || state === "recording_limited";
  const processing = state === "processing" || state === "rate_limited";
  const activeHud = listening || processing;
  const selectedActionWaiting = state === "idle" && Boolean(selectedActionState);
  const isIdle = state === "idle" && !selectedActionWaiting;
  const audioIssue = listening ? recordingAudioIssue(audioCaptureStatus, t) : null;
  const presentation = hudPresentation({
    state,
    fallbackReason,
    deliveryMethod,
    cleanupStatus,
    captureStatus: audioCaptureStatus,
    recoveryOutcome,
    selectedActionState,
  }, t);
  const uncertainPasteDetailsAvailable = uncertainPasteAttempt && Boolean(presentation.detail);
  const device = audioDeviceName?.trim() || t("设备名称不可用");
  const deviceSource = audioDeviceSourceLabel?.trim() || t("录音设备未知");
  const deviceDescription = audioDeviceTitle?.trim() || `${deviceSource}：${device}`;
  const audioNow = Math.max(Date.now(), clockNowMs);
  const listeningAudio = listening
    ? hudAudioLevel(waveformLevels, lastAudioLevelAtMs, audioNow)
    : { fresh: false, level: 0 };
  const statusCopy = recoveryAction?.title
    ?? (processing
      ? hudStatusLabel(state, null, { phase, phaseConfirmed, translate: t })
      : audioIssue?.title
        ?? presentation.title
        ?? (selectedActionWaiting
          ? selectedActionCaption(selectedActionState, t)
          : hudStatusLabel(state, null, { phase, phaseConfirmed, translate: t })));
  const statusLabel = statusCopy ?? stateAriaLabel(state, t);
  const confirmedTargetChangeOutcome = fallbackReason === "target_changed"
    && ((state === "copied" && deliveryMethod === "clipboard")
      || (state === "history" && deliveryMethod === "history"));
  const caption = listening
    ? audioIssue?.detail ?? presentation.subtitle ?? (recording ? device : audioStatus)
    : processing ? null
      : confirmedTargetChangeOutcome ? presentation.subtitle
        : uncertainPasteAttempt || unverifiedTextAction ? t("先检查输入框")
          : selectedActionCaption(selectedActionState, t)
        ?? deliveryReasonHudMessage(fallbackReason, t)
        ?? (state === "unverified" ? t("先检查输入框")
          : state === "degraded" ? t("检查完整录音")
            : state === "copied" ? t("请手动粘贴")
              : state === "history" ? t("需要时再复制")
                : state === "error" ? t("查看恢复详情")
                  : null);
  const captionWithCleanupNotice = [caption, presentation.detail].filter(Boolean).join(" · ") || null;
  const elapsed = formatHudElapsed(stageElapsedMs);
  const audioLevel = listeningAudio.fresh ? listeningAudio.level : 0;
  const reasonDetail = unverifiedTextAction || (uncertainPasteAttempt && !uncertainPasteDetailsAvailable)
    ? null
    : fallbackReason === "delivery_failed"
      ? t("自动输入未完成，请检查历史记录后再继续")
      : deliveryReasonMessage(fallbackReason, t);
  const detailText = [
    actionError,
    presentation.subtitle,
    reasonDetail,
    presentation.detail,
    recoveryAction?.message,
    (unverifiedTextAction ? t("先检查输入框")
      : state === "unverified" ? stateAriaLabel(state, t)
      : state === "degraded" ? t("部分结果，请检查完整录音")
        : state === "error" ? t("语音输入失败，请检查设置后重试")
          : selectedActionCaption(selectedActionState, t)
            ?? contextSourceLabel
            ?? contextLabel
            ?? statusLabel),
  ].filter((value): value is string => Boolean(value)).filter((value, index, values) => values.indexOf(value) === index).join(" · ");
  const showSecondary = detailsOpen
    && (Boolean(recoveryAction)
      || ((!uncertainPasteAttempt || uncertainPasteDetailsAvailable) && !isIdle && Boolean(detailText)));
  const stackHidden = hideImmediately || (isIdle && !recoveryAction);
  const stackClassName = [
    "voice-pill-stack",
    stackHidden ? "voice-pill-stack--hidden" : "",
    hideImmediately && stackHidden ? "voice-pill-stack--hidden-now" : "",
    showSecondary ? "voice-pill-stack--with-details" : "",
    reduced ? "voice-pill-stack--reduced" : "",
  ].filter(Boolean).join(" ");
  const liveDetails = [
    listening ? audioStatus : null,
    listening ? deviceDescription : null,
    actionError,
    detailsActionError,
    recoveryAction?.error,
    presentation.subtitle,
    presentation.detail,
    recoveryAction?.message ?? null,
    contextSourceLabel,
    contextLabel,
    unverifiedTextAction ? null : selectedActionCaption(selectedActionState, t),
    reasonDetail,
  ].filter((value): value is string => Boolean(value) && value !== statusLabel).join(" · ");
  const liveLabel = liveDetails ? `${statusLabel}。${liveDetails}` : statusLabel;
  const { text: labelText, outgoing: labelOutgoing, entering: labelEntering } = useHudLabel(statusLabel, reduced);
  const orbConfig = hudOrbFor(state, phase, {
    reduced,
    pageVisible,
    level: audioLevel,
    levelFresh: listeningAudio.fresh,
    selectedActionState,
  });
  const beam = hudBorderBeamFor(state);
  const showThinkingBeam = beam?.size === "line" && !reduced && pageVisible;
  const showListeningBeam = beam?.size === "md" && listeningAudio.fresh && !reduced && pageVisible;
  const [thinkingLinger, setThinkingLinger] = useState(false);
  const wasThinking = useRef(showThinkingBeam);
  if (showThinkingBeam && thinkingLinger) setThinkingLinger(false);
  else if (wasThinking.current && !showThinkingBeam && !thinkingLinger && !reduced) setThinkingLinger(true);
  wasThinking.current = showThinkingBeam;
  useEffect(() => {
    if (!thinkingLinger) return undefined;
    if (reduced || !pageVisible) {
      setThinkingLinger(false);
      return undefined;
    }
    const timer = window.setTimeout(() => setThinkingLinger(false), HUD_THINK_BEAM_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [thinkingLinger, reduced, pageVisible]);
  const thinkFx = showThinkingBeam || (thinkingLinger && !reduced && pageVisible);
  const showOrb = processing && orbConfig != null;
  const activeAction: VoicePillAction = state === "starting" || processing ? "cancel" : "stop";
  const hasAction = activeHud
    || (!isIdle && !activeHud && (!uncertainPasteAttempt || uncertainPasteDetailsAvailable))
    || Boolean(recoveryAction);
  const actionLabel = activeHud
    ? activeAction === "stop" ? t("停止录音并整理") : t("取消本次听写")
    : detailsOpen ? t("关闭详情") : t("查看详情");
  const actionIcon = activeHud ? "stop" : detailsOpen ? "close" : "arrow";
  const pillClassName = [
    "voice-pill",
    `voice-pill--${state}`,
    "voice-pill--expanded",
    "voice-pill--labeled",
    recording ? "voice-pill--recording-layout" : "",
    showOrb ? "voice-pill--orb" : "",
    reduced ? "voice-pill--reduced" : "",
  ].filter(Boolean).join(" ");
  const iconUrl = recording && recordingTargetIcon?.startsWith("data:image/png;base64,")
    ? recordingTargetIcon
    : null;
  const iconMode = activeHud
    ? audioIssue || presentation.icon === "mic-off" ? "mic-off" : "mic"
    : presentation.icon;

  return (
    <div
      className={stackClassName}
      aria-hidden={stackHidden}
      style={{
        width: showSecondary ? voicePillRecoveryWidth : voicePillWindowWidth,
        height: showSecondary ? voicePillRecoveryHeight : voicePillWindowHeight,
        ["--voice-pill-width" as string]: `${voicePillWidth}px`,
        ["--voice-pill-height" as string]: `${voicePillHeight}px`,
        ["--hud-motion-duration" as string]: `${HUD_MOTION_MS}ms`,
        ["--hud-label-in-delay" as string]: `${HUD_LABEL_IN_DELAY_MS}ms`,
        ["--hud-label-in-duration" as string]: `${HUD_LABEL_IN_DURATION_MS}ms`,
        ["--hud-label-out-duration" as string]: `${HUD_LABEL_OUT_MS}ms`,
        ["--voice-pill-radius" as string]: `${HUD_PILL_RADIUS_PX}px`,
      }}
    >
      <div className="voice-pill__beam-host">
        <div
          className={pillClassName}
          role={hasAction ? "group" : "status"}
          aria-label={liveLabel}
          title={recordingHint ?? undefined}
          style={{ width: voicePillWidth, height: voicePillHeight }}
        >
          <span className="voice-pill__sr-only" role="status" aria-live="polite" aria-atomic="true">
            {liveLabel}
          </span>
          <span className="voice-pill__indicator" aria-hidden="true">
            {showOrb && orbConfig ? (
              <span className="voice-pill__center-state voice-pill__center-state--active">
                <VoiceHudOrb
                  key="hud-orb-working"
                  state="working"
                  paused={orbConfig.paused}
                  speed={orbConfig.speed}
                  level={0}
                  dim={orbConfig.dim}
                />
              </span>
            ) : iconUrl ? (
              <img className="voice-pill__target-icon" src={iconUrl} alt="" />
            ) : (
              <HudGlyph name={iconMode} />
            )}
          </span>
          <div className="voice-pill__copy">
            <div className="voice-pill__title-row">
              <span className="voice-pill__label-stack" aria-hidden="true">
                {labelText && <span key={labelText} className={`voice-pill__label${labelEntering ? " voice-pill__label--in" : ""}`}>{labelText}</span>}
                {labelOutgoing && <span key={`out-${labelOutgoing}`} className="voice-pill__label voice-pill__label--out">{labelOutgoing}</span>}
              </span>
              {recording && <time className="voice-pill__elapsed" aria-live="off" aria-label={`${t("已等待")} ${elapsed}`}>{elapsed}</time>}
            </div>
            {caption && (
              <span className={`voice-pill__subtitle${audioIssue ? " voice-pill__subtitle--issue" : ""}`} title={deviceDescription}>
                {recording && <HudGlyph name="mic" size={11} />}
                <span>{captionWithCleanupNotice}</span>
              </span>
            )}
          </div>
          {hasAction && (
            <button
              className="voice-pill__action"
              type="button"
              aria-label={actionLabel}
              title={actionLabel}
              disabled={actionBusy}
              onMouseDown={(event) => event.preventDefault()}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                if (event.detail > 1) return;
                if (activeHud) onActiveAction?.(activeAction, sessionGeneration);
                else onDetailsOpenChange?.(!detailsOpen);
              }}
            >
              <HudGlyph name={actionIcon} size={actionIcon === "stop" ? 20 : 16} />
            </button>
          )}
        </div>
        <BorderBeam
          className="voice-pill__beam-fx"
          size={HUD_LISTENING_BEAM.size}
          colorVariant={HUD_LISTENING_BEAM.colorVariant}
          staticColors
          strength={HUD_LISTENING_BEAM.strength}
          brightness={HUD_LISTENING_BEAM.brightness}
          duration={HUD_LISTENING_BEAM.duration}
          theme="dark"
          borderRadius={HUD_PILL_RADIUS_PX}
          active={showListeningBeam}
          style={HUD_BEAM_OVERLAY_STYLE}
        >
          <div className="voice-pill__beam-ghost" />
        </BorderBeam>
        <div className={`voice-pill__beam-layer${showThinkingBeam ? "" : " voice-pill__beam-layer--exit"}`}>
          <BorderBeam
            className="voice-pill__beam-fx"
            size={HUD_THINKING_BEAM.size}
            colorVariant={HUD_THINKING_BEAM.colorVariant}
            staticColors
            strength={HUD_THINKING_BEAM.strength}
            brightness={HUD_THINKING_BEAM.brightness}
            duration={HUD_THINKING_BEAM.duration}
            theme="dark"
            borderRadius={HUD_PILL_RADIUS_PX}
            active={thinkFx}
            style={HUD_BEAM_OVERLAY_STYLE}
          >
            <div className="voice-pill__beam-ghost" />
          </BorderBeam>
        </div>
      </div>
      {recoveryAction && detailsOpen ? (
        <div className="voice-pill__secondary voice-pill__secondary--recovery" aria-busy={recoveryAction.busy}>
          <span className="voice-pill__recovery-message" title={recoveryAction.message}>{recoveryAction.message}</span>
          {(actionError ?? recoveryAction.error) && <span className="voice-pill__recovery-error" title={actionError ?? recoveryAction.error ?? undefined}>{actionError ?? recoveryAction.error}</span>}
          <button className="voice-pill__recovery-action" type="button" disabled={recoveryAction.busy || recoveryAction.disabled === true} aria-busy={recoveryAction.busy} onMouseDown={(event) => event.preventDefault()} onClick={recoveryAction.onAction}>{recoveryAction.actionLabel}</button>
          <button className="voice-pill__recovery-dismiss" type="button" aria-label={t("关闭恢复提示")} title={t("关闭恢复提示")} onMouseDown={(event) => event.preventDefault()} onClick={recoveryAction.onDismiss}><HudGlyph name="close" size={14} /></button>
        </div>
      ) : detailsOpen && (!uncertainPasteAttempt || uncertainPasteDetailsAvailable) && !isIdle ? (
        <div className="voice-pill__secondary voice-pill__secondary--details" aria-label={t("恢复详情")}>
          <span className="voice-pill__details-icon" aria-hidden="true"><HudGlyph name={presentation.icon} size={16} /></span>
          <span className="voice-pill__details-copy">
            <strong className="voice-pill__details-title">{presentation.title ?? statusLabel}</strong>
            <span className="voice-pill__details-text">{detailText}</span>
            {contextSourceLabel && <span className="voice-pill__details-source">{contextSourceLabel}</span>}
            {detailsActionError && <span className="voice-pill__details-error">{detailsActionError}</span>}
          </span>
          {!activeHud && presentation.action && onDetailsAction && (
            <button
              className="voice-pill__details-action"
              type="button"
              disabled={detailsActionBusy}
              aria-busy={detailsActionBusy}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onDetailsAction(presentation.action!.destination, sessionGeneration)}
            >
              {presentation.action.label}
            </button>
          )}
        </div>
      ) : null}
    </div>
  );
});
