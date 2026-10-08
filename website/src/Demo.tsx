import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUp,
  Check,
  Code2,
  Mail,
  MessageCircle,
  MoreHorizontal,
  Pause,
  Play,
  Add,
  RotateCcw,
  AudioWave,
} from "./Icons";
import { content, type Locale, type Scene } from "./content";
import { FlowPath } from "./Icons";
import { demoFrame, demoTiming } from "./demoTimeline";
import { useDemoPlayback } from "./useDemoPlayback";
import DemoHud from "./DemoHud";

const scenes: Scene[] = ["chat", "email", "code"];
const sceneIcons = { chat: MessageCircle, email: Mail, code: Code2 };

export default function Demo({ locale }: { locale: Locale }) {
  const copy = content[locale].demo;
  const [scene, setScene] = useState<Scene>("chat");
  const sample = copy.scenes[scene];
  const holdDuration = Math.max(
    3200,
    Math.min(6000, sample.result.length * 32),
  );
  const playback = useDemoPlayback(demoTiming.complete + holdDuration, locale);
  const [announcement, setAnnouncement] = useState("");
  const manualCompletion = useRef(false);
  const frame = demoFrame(playback.elapsed);
  const { phase } = frame;
  const showResult = phase === "writing" || phase === "complete";
  const spokenTokens = useMemo(
    () =>
      locale === "zh"
        ? Array.from(sample.spoken)
        : (sample.spoken.match(/\S+\s*/g) ?? []),
    [locale, sample.spoken],
  );
  const resultCharacters = useMemo(
    () => Array.from(sample.result),
    [sample.result],
  );
  const heardCount = playback.reducedMotion
    ? spokenTokens.length
    : Math.ceil(frame.spokenProgress * spokenTokens.length);
  const resultCount =
    playback.reducedMotion && showResult
      ? resultCharacters.length
      : Math.ceil(frame.resultProgress * resultCharacters.length);
  const result = resultCharacters.slice(0, resultCount).join("");
  const SceneIcon = sceneIcons[scene];

  useEffect(() => {
    setAnnouncement("");
    manualCompletion.current = false;
  }, [locale]);

  useEffect(() => {
    if (phase !== "complete" || !manualCompletion.current) return;
    manualCompletion.current = false;
    setAnnouncement(copy.phases.complete);
  }, [phase, copy.phases.complete]);

  function selectScene(next: Scene) {
    if (scene === next) return;
    playback.reset();
    manualCompletion.current = false;
    setScene(next);
  }

  function togglePlayback() {
    if (playback.paused) manualCompletion.current = true;
    playback.toggle();
    setAnnouncement(playback.paused ? copy.resume : copy.paused);
  }

  function replay() {
    manualCompletion.current = true;
    playback.replay();
    setAnnouncement(copy.restarted);
  }

  const PlaybackIcon = playback.paused ? Play : Pause;
  const playbackLabel = !playback.paused
    ? copy.pause
    : playback.reducedMotion && phase === "complete"
      ? copy.play
      : copy.resume;

  return (
    <section
      className="demo-section container"
      id="demo"
      aria-label={copy.label}
    >
      <div className="demo-toolbar">
        <div className="scene-switcher" role="group" aria-label={copy.label}>
          {scenes.map((item) => {
            const Icon = sceneIcons[item];
            return (
              <button
                key={item}
                type="button"
                className="scene-button"
                aria-pressed={scene === item}
                onClick={() => selectScene(item)}
              >
                <Icon size={16} aria-hidden="true" />
                {copy.scenes[item].label}
              </button>
            );
          })}
        </div>
        <div className="demo-controls">
          <button
            type="button"
            className="play-demo"
            onClick={togglePlayback}
            aria-label={playbackLabel}
          >
            <PlaybackIcon size={18} />
            <span>{playbackLabel}</span>
          </button>
          <button
            type="button"
            className="replay-demo"
            onClick={replay}
            aria-label={copy.replay}
          >
            <RotateCcw size={16} aria-hidden="true" />
          </button>
        </div>
      </div>

      <div
        className="demo-stage"
        ref={playback.stageRef}
        data-phase={phase}
        data-scene={scene}
        data-running={playback.running}
      >
        <div className="demo-atmosphere" aria-hidden="true">
          <span />
          <span />
        </div>
        <div className="demo-grain" aria-hidden="true" />
        <div className="spoken-example">
          <span className="mini-label">
            <AudioWave size={18} className="tiny-wave" />
            {copy.spoken}
          </span>
          <p key={scene}>
            <span className="sr-only">{sample.spoken}</span>
            <span className="spoken-tokens" aria-hidden="true">
              {spokenTokens.map((token, index) => (
                <span
                  key={index}
                  className={index < heardCount ? "is-heard" : ""}
                >
                  {token}
                </span>
              ))}
            </span>
          </p>
        </div>
        <FlowPath className="demo-flow-path" />

        <div className="app-window" role="group" aria-label={copy.appWindow}>
          <div className="window-chrome">
            <span className="traffic-lights" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
            <span className="window-name">
              <SceneIcon size={16} />
              {sample.app}
            </span>
            <MoreHorizontal size={18} />
          </div>
          <div className="window-content" key={scene}>
            <div className="recipient-row">
              <span className="recipient-avatar" aria-hidden="true">
                {scene === "chat" ? "D" : scene === "email" ? "M" : "⌘"}
              </span>
              <div>
                <strong>{sample.recipient}</strong>
                <span>{sample.context}</span>
              </div>
            </div>
            <div
              className={`result-composer ${showResult ? "has-result" : ""}`}
            >
              <div className="composer-content">
                <p className="result-reserve" aria-hidden="true">
                  {sample.result}
                  <span className="writing-cursor" />
                </p>
                <p className="result-text" aria-hidden="true">
                  {result}
                  <span className="writing-cursor" aria-hidden="true" />
                </p>
                <span className="sr-only">
                  {copy.written}: {sample.result}
                </span>
                <div className="result-waiting" aria-hidden="true">
                  <span>{copy.placeholder}</span>
                  <div className="waiting-lines" aria-hidden="true">
                    <i />
                    <i />
                    <i />
                  </div>
                </div>
              </div>
              <div className="composer-bottom" aria-hidden="true">
                <Add size={18} />
                <span className="mock-send">
                  <ArrowUp size={18} />
                </span>
              </div>
            </div>
          </div>
          <div className="window-caption">
            <span className="caption-dot" />
            <span>{phase === "complete" ? copy.written : copy.preview}</span>
            {phase === "complete" && <Check size={16} aria-hidden="true" />}
          </div>
        </div>

        <div className="voice-feedback">
          <DemoHud
            phase={phase}
            progress={Math.max(
              0,
              Math.min(
                1,
                (playback.elapsed - demoTiming.processing) /
                  (demoTiming.complete - demoTiming.processing),
              ),
            )}
            running={playback.running}
            reducedMotion={playback.reducedMotion}
          />
          <p className="phase-label" aria-live="off">
            {playback.paused && !playback.reducedMotion
              ? copy.paused
              : copy.phases[phase]}
          </p>
        </div>
      </div>
      <p
        className="sr-only"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {announcement}
      </p>
      <p className="demo-disclaimer">
        <span className="example-dot" aria-hidden="true" />
        {copy.caption}
      </p>
    </section>
  );
}
