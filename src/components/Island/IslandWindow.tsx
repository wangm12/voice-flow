import { useEffect, useState } from "react";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { VoicePill } from "./VoicePill";
import { WAVEFORM_BAR_COUNT } from "./VoiceWaveform";
import { useReducedMotionPreference } from "./springs";

type DictationState = "idle" | "starting" | "recording" | "recording_limited" | "processing" | "rate_limited" | "done" | "unverified" | "copied" | "degraded" | "history" | "error";
type ProcessingPhase = "finalizing_audio" | "asr" | "cleanup" | "delivery" | "waiting_retry" | "idle";
type HudState = {
  sessionGeneration: number;
  state: DictationState;
  phase: ProcessingPhase;
  retryAfterSecs: number | null;
  undoAvailable: boolean;
  contextLabel: string | null;
  fallbackReason: string | null;
  waveformLevels: number[];
  progress: number;
  selectedActionState: string | null;
};

function emptyWaveform(): number[] {
  return Array.from({ length: WAVEFORM_BAR_COUNT }, () => 0);
}

export function acceptsSessionGeneration(current: number, incoming?: number): incoming is number {
  return incoming !== undefined && incoming >= current;
}

export function IslandWindow() {
  const [hud, setHud] = useState<HudState>({
    sessionGeneration: 0,
    state: "idle",
    phase: "idle",
    retryAfterSecs: null,
    undoAvailable: false,
    contextLabel: null,
    fallbackReason: null,
    waveformLevels: emptyWaveform(),
    progress: 0,
    selectedActionState: null,
  });
  const reduced = useReducedMotionPreference();

  useEffect(() => {
    let disposed = false;
    const unlisteners = new Set<UnlistenFn>();

    function register<T>(event: string, handler: EventCallback<T>) {
      void listen<T>(event, handler)
        .then((unlisten) => {
          // A listener can resolve after React has already unmounted this
          // window. Unsubscribe immediately instead of leaking the late
          // registration into the next HUD instance.
          if (disposed) {
            unlisten();
          } else {
            unlisteners.add(unlisten);
          }
        })
        .catch(() => {
          // A torn-down WebView can reject registration during shutdown. The
          // HUD has no useful recovery action for that transient condition.
        });
    }

    register<DictationStatePayload>("dictation://state", (event) => {
        const eventGeneration = event.payload.session_generation;
        if (!acceptsSessionGeneration(0, eventGeneration)) return;
        const next = event.payload.state;
        const phase = event.payload.phase ?? (next === "processing" ? "cleanup" : next === "idle" ? "idle" : "finalizing_audio");
          const nextProgress = next === "idle" || next === "recording" ? 0 : ["done", "unverified", "copied", "degraded", "history"].includes(next) ? 1 : next === "processing" ? phase === "asr" ? 0.35 : phase === "cleanup" ? 0.65 : phase === "delivery" ? 0.9 : 0.05 : 0;
        setHud((current) => {
          if (!acceptsSessionGeneration(current.sessionGeneration, eventGeneration)) return current;
          const nextHud = {
            sessionGeneration: eventGeneration,
            state: next,
            phase,
            retryAfterSecs: event.payload.retry_after_secs ?? null,
            undoAvailable: event.payload.undo_available ?? false,
            contextLabel: next === "idle" ? null : event.payload.context_label ?? null,
            fallbackReason: next === "idle" ? null : event.payload.fallback_reason ?? null,
            // Audio starts before the final recording state is committed. Keep
            // the live waveform continuous through that short starting phase.
            waveformLevels: next === "starting" || next === "recording" ? current.waveformLevels : emptyWaveform(),
            progress: nextProgress,
            selectedActionState: current.selectedActionState,
          };
          return current.sessionGeneration === nextHud.sessionGeneration
            && current.state === nextHud.state
            && current.phase === nextHud.phase
            && current.retryAfterSecs === nextHud.retryAfterSecs
            && current.undoAvailable === nextHud.undoAvailable
            && current.contextLabel === nextHud.contextLabel
            && current.fallbackReason === nextHud.fallbackReason
            && current.waveformLevels === nextHud.waveformLevels
            && current.progress === nextHud.progress
            ? current
            : nextHud;
        });
    });
    register<{ level?: number }>("audio://level", (event) => {
      const nextLevel = Math.max(0, Math.min(1, event.payload.level ?? 0));
      setHud((current) => {
        if (current.state !== "starting" && current.state !== "recording") {
          return current;
        }
        const waveformLevels = [...current.waveformLevels.slice(1), nextLevel];
        return { ...current, waveformLevels };
      });
    });
    register<{ progress?: number; session_generation?: number }>("dictation://progress", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      const nextProgress = Math.max(0, Math.min(1, event.payload.progress ?? 0));
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration) || current.progress === nextProgress
        ? current
        : { ...current, progress: nextProgress });
    });
    register<{ max_recording_secs: number; session_generation?: number }>("audio://limit", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : { ...current, sessionGeneration: eventGeneration, state: "recording_limited", phase: "idle", retryAfterSecs: null, fallbackReason: null, waveformLevels: emptyWaveform() });
    });
    register<{ retry_after_secs: number; session_generation?: number }>("quota://rate_limited", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : { ...current, sessionGeneration: eventGeneration, state: "rate_limited", phase: "waiting_retry", retryAfterSecs: event.payload.retry_after_secs, fallbackReason: null, waveformLevels: emptyWaveform() });
    });
    register<{ state?: string }>("selected-action://state", (event) => {
      setHud((current) => ({
        ...current,
        selectedActionState: event.payload.state && event.payload.state !== "idle" ? event.payload.state : null,
      }));
    });

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
      unlisteners.clear();
    };
  }, []);

  return (
    <div className="voice-pill-stage">
      <VoicePill state={hud.state} phase={hud.phase} retryAfterSecs={hud.retryAfterSecs} undoAvailable={hud.undoAvailable} contextLabel={hud.contextLabel} fallbackReason={hud.fallbackReason} selectedActionState={hud.selectedActionState} waveformLevels={hud.waveformLevels} progress={hud.progress} reduced={reduced} />
    </div>
  );
}

type DictationStatePayload = { state: DictationState; session_generation?: number; phase?: ProcessingPhase; retry_after_secs?: number; completed_chunks?: number; total_chunks?: number; cleanup_status?: string | null; undo_available?: boolean; context_id?: string; context_label?: string; delivery_method?: string; fallback_reason?: string | null };
