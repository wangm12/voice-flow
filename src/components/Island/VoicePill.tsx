import { memo, useEffect, useRef, useState, type CSSProperties } from "react";
import { BorderBeam } from "border-beam";
import { useI18n } from "../../lib/i18n";
import { VoiceHudOrb } from "./VoiceHudOrb";
import {
  HUD_CAPTION_REVEAL_DELAY_MS,
  HUD_CAPTION_REVEAL_MS,
  HUD_LABEL_FADE_MS,
  HUD_LABEL_IN_DELAY_MS,
  HUD_LABEL_IN_DURATION_MS,
  HUD_LABEL_OUT_MS,
  HUD_ORB_FADE_MS,
  HUD_THINK_BEAM_FADE_MS,
  HUD_PILL_RADIUS_PX,
  HUD_LISTENING_BEAM,
  HUD_THINKING_BEAM,
  hudBorderBeamFor,
  hudOrbFor,
  hudStatusLabel,
  perceptualLevel,
  shouldMountHudOrb,
  type DictationState,
  type HudBorderBeamConfig,
  type HudOrbConfig,
  type ProcessingPhase,
} from "./hudOrb";
import { useSmoothLineBeam, type LineBeamMode } from "./hudLineBeam";
import { CONTEXT_LABEL_VISIBLE_MS, pillCaption, visibleContextLabel, voicePillCaptionMaxWidthForPartial, voicePillCaptionNeedsWide } from "./voicePillTokens";

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
      return t("已复制，请按 ⌘V");
    case "degraded":
      return t("部分结果");
    case "error":
      return t("错误");
    default:
      return t("VoiceFlow");
  }
}

function useHudLabel(
  next: string | null,
  reduced: boolean,
): { text: string | null; outgoing: string | null; entering: boolean } {
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
  retryAfterSecs = null,
  undoAvailable: _undoAvailable = false,
  contextLabel,
  fallbackReason = null,
  selectedActionState = null,
  waveformLevels,
  progress,
  chunkProgress: _chunkProgress,
  partialText: _partialText = null,
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
  const [retryRemaining, setRetryRemaining] = useState(retryAfterSecs);
  const [contextShown, setContextShown] = useState<{ label: string; at: number } | null>(null);
  const [contextNowMs, setContextNowMs] = useState(() => Date.now());
  const nextContextLabel = contextLabel?.trim() || null;
  if (state === "idle") {
    if (contextShown != null) setContextShown(null);
  } else if (nextContextLabel && contextShown?.label !== nextContextLabel) {
    setContextShown({ label: nextContextLabel, at: Date.now() });
    setContextNowMs(Date.now());
  }
  const visibleContext = visibleContextLabel(
    nextContextLabel,
    contextShown?.at ?? null,
    contextNowMs,
  );
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
  useEffect(() => {
    if (contextShown == null) return;
    const remaining = CONTEXT_LABEL_VISIBLE_MS - (Date.now() - contextShown.at);
    const timer = window.setTimeout(() => setContextNowMs(Date.now()), Math.max(0, remaining));
    return () => window.clearTimeout(timer);
  }, [contextShown]);
  const progressValue = Math.max(0, Math.min(1, progress));
  const showProgress = isLoading && (reduced || hudBorderBeamFor(state)?.size !== "line");
  const indeterminateProgress = showProgress && state !== "processing";
  const caption = pillCaption(
    state,
    t,
    state === "rate_limited" ? retryRemaining : retryAfterSecs,
    { fallbackReason, selectedActionState, contextLabel: visibleContext },
  );
  const showingPartial = false;
  const wideCaption = voicePillCaptionNeedsWide(state, { fallbackReason });
  const stackHidden = state === "idle" && !caption;
  const showStackExit = isTerminal && !caption;
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

  const captionTone = state === "error"
    ? "error"
    : state === "degraded" || fallbackReason || selectedActionState === "accessibility_required"
      ? "warning"
      : "status";
  const statusLabel = stateAriaLabel(state, t);
  const liveLabel = caption ? `${statusLabel}。${caption}` : statusLabel;
  const orbConfig = hudOrbFor(state, phase, {
    reduced,
    level: perceptualLevel(waveformLevels),
    selectedActionState,
  });
  const showOrb = shouldMountHudOrb(state, Boolean(caption)) && orbConfig != null;
  const lastOrbRef = useRef<HudOrbConfig | null>(orbConfig);
  if (orbConfig) lastOrbRef.current = orbConfig;
  const [orbVisible, setOrbVisible] = useState(showOrb);
  useEffect(() => {
    if (showOrb) {
      setOrbVisible(true);
      return undefined;
    }
    if (reduced) {
      setOrbVisible(false);
      return undefined;
    }
    const timer = window.setTimeout(() => setOrbVisible(false), HUD_ORB_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [showOrb, reduced]);
  const paintedOrb = orbVisible ? (orbConfig ?? lastOrbRef.current) : null;
  const statusCopy = hudStatusLabel(state, showOrb ? orbConfig : null);
  const { text: labelText, outgoing: labelOutgoing, entering: labelEntering } = useHudLabel(statusCopy, reduced);
  const captionReveal = !reduced && [
    "copied",
    "unverified",
    "done",
    "degraded",
    "error",
    "history",
  ].includes(state);
  const liveCaptionRef = useRef<string | null>(null);
  if (!captionReveal) liveCaptionRef.current = caption;
  const [terminalCaptionOn, setTerminalCaptionOn] = useState(false);
  if (!captionReveal && terminalCaptionOn) setTerminalCaptionOn(false);
  else if (captionReveal && reduced && !terminalCaptionOn) setTerminalCaptionOn(true);
  useEffect(() => {
    if (!captionReveal || reduced) return undefined;
    const timer = window.setTimeout(() => setTerminalCaptionOn(true), HUD_LABEL_OUT_MS);
    return () => window.clearTimeout(timer);
  }, [captionReveal, reduced, state]);
  const holdTerminalChrome = captionReveal && !terminalCaptionOn && !reduced;
  const paintedCaption = holdTerminalChrome ? liveCaptionRef.current : caption;
  const paintedTone = holdTerminalChrome ? "status" : captionTone;
  const captionExiting = holdTerminalChrome && Boolean(liveCaptionRef.current);
  const showCaptionReveal = captionReveal && terminalCaptionOn && !reduced;
  const beam = hudBorderBeamFor(state);
  const thinkOn = beam?.size === "line" && !reduced;
  const [thinkLinger, setThinkLinger] = useState(false);
  const wasThinkOn = useRef(thinkOn);
  if (thinkOn && thinkLinger) setThinkLinger(false);
  else if (wasThinkOn.current && !thinkOn && !thinkLinger && !reduced) setThinkLinger(true);
  wasThinkOn.current = thinkOn;
  useEffect(() => {
    if (!thinkLinger) return undefined;
    const timer = window.setTimeout(() => setThinkLinger(false), HUD_THINK_BEAM_FADE_MS);
    return () => window.clearTimeout(timer);
  }, [thinkLinger]);
  const thinkFx = thinkOn || thinkLinger;
  const beamRef = useRef<HudBorderBeamConfig>(HUD_LISTENING_BEAM);
  const previousBeam = beamRef.current;
  if (beam) beamRef.current = beam;
  const beamElRef = useRef<HTMLDivElement>(null);
  const lineMode: LineBeamMode = thinkFx && (beam?.size === "line" || previousBeam.size === "line")
    ? "scrub"
    : "off";
  useSmoothLineBeam(beamElRef, lineMode, progressValue);
  const chromeState = holdTerminalChrome ? "processing" : visualState;
  const pillClassName = [
    "voice-pill",
    `voice-pill--${chromeState}`,
    labelText ? "voice-pill--labeled" : "",
    paintedOrb ? "voice-pill--orb" : "",
    reduced ? "voice-pill--reduced" : "",
  ].filter(Boolean).join(" ");
  const indicatorClassName = "voice-pill__indicator";

  return (
    <div
      className={stackClassName}
      aria-hidden={stackHidden}
      style={{
        ["--hud-label-in-delay" as string]: `${HUD_LABEL_IN_DELAY_MS}ms`,
        ["--hud-label-in-duration" as string]: `${HUD_LABEL_IN_DURATION_MS}ms`,
        ["--hud-label-out-duration" as string]: `${HUD_LABEL_OUT_MS}ms`,
        ["--hud-caption-reveal-delay" as string]: `${HUD_CAPTION_REVEAL_DELAY_MS}ms`,
        ["--hud-caption-reveal-duration" as string]: `${HUD_CAPTION_REVEAL_MS}ms`,
      }}
    >
      <div className="voice-pill__beam-host">
        <div className={pillClassName} role="status" aria-label={liveLabel} aria-live="polite">
          <span
            className={progressClassName}
            style={indeterminateProgress ? undefined : { transform: `scaleX(${progressValue})` }}
            aria-hidden="true"
          />
          <div className="voice-pill__content">
            <span className={indicatorClassName}>
              <span
                className={`voice-pill__center-state${showOrb ? " voice-pill__center-state--active" : ""}`}
                aria-hidden={!showOrb}
              >
                {paintedOrb && (
                  <VoiceHudOrb
                    key="hud-orb"
                    state={paintedOrb.state}
                    paused={paintedOrb.paused}
                    speed={paintedOrb.speed}
                    dim={paintedOrb.dim}
                    caution={state === "unverified" || state === "degraded" || state === "error"}
                  />
                )}
              </span>
            </span>
            {(labelText || labelOutgoing) && (
              <span className="voice-pill__label-stack" aria-hidden="true">
                {labelText && (
                  <span
                    key={labelText}
                    className={`voice-pill__label${labelEntering ? " voice-pill__label--in" : ""}`}
                  >
                    {labelText}
                  </span>
                )}
                {labelOutgoing && (
                  <span key={`out-${labelOutgoing}`} className="voice-pill__label voice-pill__label--out">
                    {labelOutgoing}
                  </span>
                )}
              </span>
            )}
          </div>
        </div>
        <BorderBeam
          className="voice-pill__beam-fx"
          size={HUD_LISTENING_BEAM.size}
          colorVariant={HUD_LISTENING_BEAM.colorVariant}
          strength={HUD_LISTENING_BEAM.strength}
          duration={HUD_LISTENING_BEAM.duration}
          theme="dark"
          borderRadius={HUD_PILL_RADIUS_PX}
          active={beam?.size === "md" && !reduced}
          style={HUD_BEAM_OVERLAY_STYLE}
        >
          <div className="voice-pill__beam-ghost" />
        </BorderBeam>
        <div
          className={`voice-pill__beam-layer${thinkOn ? "" : " voice-pill__beam-layer--exit"}`}
        >
          <BorderBeam
            ref={beamElRef}
            className="voice-pill__beam-fx"
            size={HUD_THINKING_BEAM.size}
            colorVariant={HUD_THINKING_BEAM.colorVariant}
            strength={HUD_THINKING_BEAM.strength}
            theme="dark"
            borderRadius={HUD_PILL_RADIUS_PX}
            active={thinkFx}
            style={HUD_BEAM_OVERLAY_STYLE}
          >
            <div className="voice-pill__beam-ghost" />
          </BorderBeam>
        </div>
      </div>
      {paintedCaption && (
        <p
          key={showCaptionReveal ? state : "live"}
          className={[
            "voice-pill-caption",
            `voice-pill-caption--${paintedTone}`,
            showingPartial && !holdTerminalChrome ? "voice-pill-caption--partial" : "",
            wideCaption && !holdTerminalChrome ? "voice-pill-caption--wide" : "",
            showCaptionReveal ? "voice-pill-caption--reveal" : "",
            captionExiting ? "voice-pill-caption--exit" : "",
          ].filter(Boolean).join(" ")}
          style={wideCaption && !holdTerminalChrome ? { maxWidth: voicePillCaptionMaxWidthForPartial(true) } : undefined}
        >
          <span className="voice-pill-caption__dot" aria-hidden="true" />
          <span className="voice-pill-caption__text">{paintedCaption}</span>
        </p>
      )}
    </div>
  );
});
