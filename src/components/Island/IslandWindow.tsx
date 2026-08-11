import { useEffect, useState } from "react";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { VoicePill } from "./VoicePill";
import { WAVEFORM_BAR_COUNT } from "./VoiceWaveform";
import { useReducedMotionPreference } from "./springs";

type DictationState = "idle" | "starting" | "recording" | "recording_limited" | "processing" | "rate_limited" | "done" | "copied" | "degraded" | "error";
type HudState = {
  state: DictationState;
  contextLabel: string | null;
  fallbackReason: string | null;
  waveformLevels: number[];
  progress: number;
  selectedActionState: string | null;
};

function emptyWaveform(): number[] {
  return Array.from({ length: WAVEFORM_BAR_COUNT }, () => 0);
}

export function IslandWindow() {
  const [hud, setHud] = useState<HudState>({
    state: "idle",
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
        const next = event.payload.state;
        const nextProgress = next === "idle" || next === "recording" ? 0 : ["done", "copied", "degraded"].includes(next) ? 1 : next === "processing" ? 0.05 : 0;
        setHud((current) => {
          const nextHud = {
            state: next,
            contextLabel: next === "idle" ? null : event.payload.context_label ?? null,
            fallbackReason: next === "idle" ? null : event.payload.fallback_reason ?? null,
            // Audio starts before the final recording state is committed. Keep
            // the live waveform continuous through that short starting phase.
            waveformLevels: next === "starting" || next === "recording" ? current.waveformLevels : emptyWaveform(),
            progress: nextProgress,
            selectedActionState: current.selectedActionState,
          };
          return current.state === nextHud.state
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
    register<{ progress?: number }>("dictation://progress", (event) => {
      const nextProgress = Math.max(0, Math.min(1, event.payload.progress ?? 0));
      setHud((current) => current.progress === nextProgress ? current : { ...current, progress: nextProgress });
    });
    register<{ max_recording_secs: number }>("audio://limit", () => {
      setHud((current) => ({ ...current, state: "recording_limited", fallbackReason: null, waveformLevels: emptyWaveform() }));
    });
    register<{ retry_after_secs: number }>("quota://rate_limited", () => {
      setHud((current) => ({ ...current, state: "rate_limited", fallbackReason: null, waveformLevels: emptyWaveform() }));
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
      <VoicePill state={hud.state} contextLabel={hud.contextLabel} fallbackReason={hud.fallbackReason} selectedActionState={hud.selectedActionState} waveformLevels={hud.waveformLevels} progress={hud.progress} reduced={reduced} />
    </div>
  );
}

type DictationStatePayload = { state: DictationState; context_id?: string; context_label?: string; delivery_method?: string; fallback_reason?: string | null };
