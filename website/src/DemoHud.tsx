import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { BorderBeam } from "border-beam";
import { VoiceHudOrb } from "../../src/components/Island/VoiceHudOrb";
import {
  HUD_LABEL_IN_DELAY_MS,
  HUD_LABEL_IN_DURATION_MS,
  HUD_LABEL_OUT_MS,
  HUD_LISTENING_BEAM,
  HUD_PILL_RADIUS_PX,
  HUD_THINKING_BEAM,
  hudOrbFor,
  hudStatusLabel,
  type DictationState,
} from "../../src/components/Island/hudOrb";
import {
  applyLineBeamProgress,
  driveLineBeamWithJs,
  releaseLineBeamDrive,
  stepSmoothThinkingProgress,
} from "../../src/components/Island/hudLineBeam";
import type { Phase } from "./content";

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

  // Match VoicePill's label crossfade. CSS animation-end cleanup also respects
  // the website's pause control without running completion timers offscreen.
  if (next !== text) {
    setOutgoing(!reduced && text != null && next != null ? text : null);
    setEntering(!reduced && text != null && next != null);
    setText(next);
  } else if (reduced && (outgoing != null || entering)) {
    setOutgoing(null);
    setEntering(false);
  }

  return {
    text,
    outgoing,
    entering,
    finishEntering: () => setEntering(false),
    finishOutgoing: () => setOutgoing(null),
  };
}

/** Present the actual product's UI using the browser demo's preset clock. */
export default function DemoHud({
  phase,
  progress,
  running,
  reducedMotion,
}: {
  phase: Phase;
  progress: number;
  running: boolean;
  reducedMotion: boolean;
}) {
  const state: DictationState =
    phase === "idle"
      ? "idle"
      : phase === "listening"
        ? "recording"
        : phase === "complete"
          ? "done"
          : "processing";
  const orb =
    state === "idle"
      ? null
      : hudOrbFor(state, phase === "writing" ? "delivery" : "cleanup", {
          reduced: reducedMotion,
          level: 0,
        });
  const listening = state === "recording";
  const thinking = state === "processing";
  const label = useHudLabel(hudStatusLabel(state, orb), reducedMotion);
  const lineRef = useRef<HTMLDivElement>(null);
  const visualProgress = useRef(0);
  const targetProgress = useRef(progress);
  targetProgress.current = progress;

  // Reuse the product's line geometry and easing. Its live hook intentionally
  // creeps while waiting, so the website adds pause/resume without resetting it.
  useLayoutEffect(() => {
    const line = lineRef.current;
    if (!line) return;
    if (!thinking || reducedMotion) {
      visualProgress.current = 0;
      releaseLineBeamDrive(line);
      applyLineBeamProgress(line, null);
      line.style.removeProperty("animation-play-state");
      return;
    }
    driveLineBeamWithJs(line);
    line.style.setProperty(
      "animation-play-state",
      running ? "running" : "paused",
      "important",
    );
    if (!running) return;
    let frame: number;
    let previous = performance.now();
    function tick(now: number) {
      const dt = Math.min(0.05, Math.max(0, (now - previous) / 1000));
      previous = now;
      visualProgress.current = stepSmoothThinkingProgress(
        visualProgress.current,
        targetProgress.current,
        dt,
      );
      applyLineBeamProgress(line!, visualProgress.current);
      frame = requestAnimationFrame(tick);
    }
    applyLineBeamProgress(line, visualProgress.current);
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [thinking, running, reducedMotion]);

  const stackClass = [
    "voice-pill-stack",
    state === "idle" ? "voice-pill-stack--hidden" : "",
    state === "done" ? "voice-pill-stack--exit" : "",
    reducedMotion ? "voice-pill-stack--reduced" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className="demo-hud"
      data-hud-state={state}
      data-running={running}
      aria-hidden="true"
      style={
        {
          "--hud-label-in-delay": `${HUD_LABEL_IN_DELAY_MS}ms`,
          "--hud-label-in-duration": `${HUD_LABEL_IN_DURATION_MS}ms`,
          "--hud-label-out-duration": `${HUD_LABEL_OUT_MS}ms`,
        } as CSSProperties
      }
    >
      <div className={stackClass}>
        <div className="voice-pill__beam-host">
          <div
            className={`voice-pill voice-pill--${state} voice-pill--labeled${orb ? " voice-pill--orb" : ""}`}
          >
            {thinking && reducedMotion && (
              <span
                className="voice-pill__progress voice-pill__progress--visible"
                style={{ transform: `scaleX(${progress})` }}
              />
            )}
            <div className="voice-pill__content">
              <span className="voice-pill__indicator">
                <span
                  className={`voice-pill__center-state${orb ? " voice-pill__center-state--active" : ""}`}
                >
                  {orb && (
                    <VoiceHudOrb
                      state={orb.state}
                      paused={orb.paused || !running}
                      speed={orb.speed}
                      dim={orb.dim}
                    />
                  )}
                </span>
              </span>
              <span className="voice-pill__label-stack">
                {label.text && (
                  <span
                    key={label.text}
                    className={`voice-pill__label${label.entering ? " voice-pill__label--in" : ""}`}
                    onAnimationEnd={label.finishEntering}
                  >
                    {label.text}
                  </span>
                )}
                {label.outgoing && (
                  <span
                    key={`out-${label.outgoing}`}
                    className="voice-pill__label voice-pill__label--out"
                    onAnimationEnd={label.finishOutgoing}
                  >
                    {label.outgoing}
                  </span>
                )}
              </span>
            </div>
          </div>
          <BorderBeam
            className="voice-pill__beam-fx"
            {...HUD_LISTENING_BEAM}
            theme="dark"
            borderRadius={HUD_PILL_RADIUS_PX}
            active={listening && !reducedMotion}
            style={overlayStyle}
          >
            <div className="voice-pill__beam-ghost" />
          </BorderBeam>
          <div
            className={`voice-pill__beam-layer${thinking ? "" : " voice-pill__beam-layer--exit"}`}
          >
            <BorderBeam
              ref={lineRef}
              className="voice-pill__beam-fx"
              {...HUD_THINKING_BEAM}
              theme="dark"
              borderRadius={HUD_PILL_RADIUS_PX}
              active={thinking && !reducedMotion}
              style={overlayStyle}
            >
              <div className="voice-pill__beam-ghost" />
            </BorderBeam>
          </div>
        </div>
      </div>
    </div>
  );
}
