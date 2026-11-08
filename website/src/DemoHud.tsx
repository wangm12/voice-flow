import { useState, type CSSProperties } from "react";
import { BorderBeam } from "border-beam";
import { VoiceHudOrb } from "../../src/components/Island/VoiceHudOrb";
import {
  HUD_LABEL_FADE_MS,
  HUD_LABEL_OUT_MS,
  HUD_MOTION_MS,
  HUD_LISTENING_BEAM,
  HUD_PILL_RADIUS_PX,
  HUD_THINKING_BEAM,
  formatHudElapsed,
  hudOrbFor,
  hudStatusLabel,
  type DictationState,
} from "../../src/components/Island/hudOrb";
import { usePageVisible } from "../../src/components/Island/hudLineBeam";
import type { Locale, Phase } from "./content";

const overlayStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  width: "100%",
  height: "100%",
};

function useHudLabel(next: string | null, reduced: boolean) {
  const [text, setText] = useState(next);
  const [outgoing, setOutgoing] = useState<string | null>(null);
  const [entering, setEntering] = useState(false);

  if (next !== text) {
    setOutgoing(!reduced && text != null && next != null ? text : null);
    setEntering(!reduced && text != null && next != null);
    setText(next);
  } else if (reduced && (outgoing != null || entering)) {
    setOutgoing(null);
    setEntering(false);
  }

  return { text, outgoing, entering };
}

/** A preset visual only: it never presents the demo's timer as microphone evidence. */
export default function DemoHud({
  phase,
  running,
  reducedMotion,
  elapsedMs,
  locale,
}: {
  phase: Phase;
  progress: number;
  running: boolean;
  reducedMotion: boolean;
  elapsedMs: number;
  locale: Locale;
}) {
  const pageVisible = usePageVisible();
  const state: DictationState = phase === "idle"
    ? "idle"
    : phase === "listening"
      ? "recording"
      : phase === "complete"
        ? "done"
        : "processing";
  const listening = state === "recording";
  const thinking = state === "processing";
  const orb = hudOrbFor(state, "idle", {
    reduced: reducedMotion,
    pageVisible,
    level: 0,
    levelFresh: false,
  });
  const englishLabels: Record<string, string> = {
    "录音中": "Listening",
    "处理中": "Processing",
    "预设演示 · 未使用麦克风": "Preset demo · no microphone input",
    "停止录音并整理": "Stop and process",
  };
  const translate = (source: string) => locale === "zh" ? source : englishLabels[source] ?? source;
  const label = useHudLabel(hudStatusLabel(state, orb, { translate }), reducedMotion);
  const hidden = state === "idle" || state === "done";
  const stackClass = [
    "voice-pill-stack",
    hidden ? "voice-pill-stack--hidden voice-pill-stack--hidden-now" : "",
    reducedMotion ? "voice-pill-stack--reduced" : "",
  ].filter(Boolean).join(" ");

  return (
    <div
      className="demo-hud voice-pill-stage"
      data-hud-state={state}
      data-running={running}
      aria-hidden="true"
      style={{
        width: 220,
        height: 48,
        ["--voice-pill-width" as string]: "220px",
        ["--voice-pill-height" as string]: "48px",
        ["--hud-motion-duration" as string]: `${HUD_MOTION_MS}ms`,
        ["--hud-label-in-duration" as string]: `${HUD_LABEL_FADE_MS}ms`,
        ["--hud-label-out-duration" as string]: `${HUD_LABEL_OUT_MS}ms`,
      }}
    >
      <div className={stackClass}>
        <div className="voice-pill__beam-host">
          <div className={`voice-pill voice-pill--${state} voice-pill--expanded voice-pill--labeled${thinking ? " voice-pill--orb" : ""}`} style={{ width: 220, height: 48 }}>
            <span className="voice-pill__indicator" aria-hidden="true">
              {thinking && orb ? (
                <span className="voice-pill__center-state voice-pill__center-state--active">
                  <VoiceHudOrb state="working" paused={orb.paused || !running || !pageVisible} speed={orb.speed} dim={orb.dim} />
                </span>
              ) : (
                <svg className="voice-pill__glyph" width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="7" y="2.5" width="6" height="10" rx="3" />
                  <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
                </svg>
              )}
            </span>
            <span className="voice-pill__copy">
              <span className="voice-pill__title-row">
                <span className="voice-pill__label-stack" aria-hidden="true">
                  {label.text && <span key={label.text} className={`voice-pill__label${label.entering ? " voice-pill__label--in" : ""}`}>{label.text}</span>}
                  {label.outgoing && <span key={`out-${label.outgoing}`} className="voice-pill__label voice-pill__label--out">{label.outgoing}</span>}
                </span>
                {listening && <time className="voice-pill__elapsed">{formatHudElapsed(elapsedMs)}</time>}
              </span>
              {listening && (
                <span className="voice-pill__subtitle">
                  <svg className="voice-pill__glyph" width="11" height="11" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <rect x="7" y="2.5" width="6" height="10" rx="3" />
                    <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
                  </svg>
                  <span>{translate("预设演示 · 未使用麦克风")}</span>
                </span>
              )}
            </span>
            {state !== "idle" && state !== "done" && (
              <span className="voice-pill__action" aria-hidden="true" title={translate("停止录音并整理")}>
                <svg className="voice-pill__glyph" width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <rect x="5" y="5" width="10" height="10" rx="1" fill="currentColor" strokeWidth="1.6" />
                </svg>
              </span>
            )}
          </div>
          <BorderBeam className="voice-pill__beam-fx" {...HUD_LISTENING_BEAM} theme="dark" borderRadius={HUD_PILL_RADIUS_PX} active={false} style={overlayStyle}>
            <div className="voice-pill__beam-ghost" />
          </BorderBeam>
          <div className={`voice-pill__beam-layer${thinking ? "" : " voice-pill__beam-layer--exit"}`}>
            <BorderBeam className="voice-pill__beam-fx" {...HUD_THINKING_BEAM} theme="dark" borderRadius={HUD_PILL_RADIUS_PX} active={thinking && !reducedMotion && running && pageVisible} style={overlayStyle}>
              <div className="voice-pill__beam-ghost" />
            </BorderBeam>
          </div>
        </div>
      </div>
    </div>
  );
}
