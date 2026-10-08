import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { tryItHint } from "../../lib/activationCopy";
import { formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { VoicePill } from "./VoicePill";
import { voicePillLearnToastHeight } from "./voicePillTokens";
import { WAVEFORM_BAR_COUNT } from "./VoiceWaveform";
import { useReducedMotionPreference } from "./springs";
import { formatHudContextLabel, formatHudContextSource, formatHudIntensityLabel, formatHudTranslationLabel, type HudContextSource } from "../../lib/hudContextLabel";
import { useI18n } from "../../lib/i18n";

type DictationState = "idle" | "starting" | "recording" | "recording_limited" | "processing" | "rate_limited" | "done" | "unverified" | "copied" | "degraded" | "history" | "error";
type ProcessingPhase = "finalizing_audio" | "asr" | "soniox_recovery" | "cascade_accurate" | "cleanup" | "delivery" | "waiting_retry" | "idle";
type ContextSourcePayload = {
  source: HudContextSource;
  label: string;
  matched_rule_label?: string | null;
};
type HudState = {
  sessionGeneration: number;
  state: DictationState;
  phase: ProcessingPhase;
  retryAfterSecs: number | null;
  undoAvailable: boolean;
  contextApp: string | null;
  contextStyle: string | null;
  contextLabel: string | null;
  contextSource: HudContextSource | null;
  cleanupIntensity: "off" | "light" | "standard" | "heavy" | null;
  translationTargetLanguage: string | null;
  fallbackReason: string | null;
  waveformLevels: number[];
  progress: number;
  completedChunks: number | null;
  totalChunks: number | null;
  selectedActionState: string | null;
  partialText: string | null;
  recordingMode: string | null;
  recordingHotkey: string | null;
};

export function hudTranslationFromEvent(nextState: string, current: string | null, incoming?: string | null): string | null {
  if (!["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(nextState)) return null;
  if (incoming !== undefined) return incoming;
  return nextState === "starting" ? null : current;
}

function emptyWaveform(): number[] {
  return Array.from({ length: WAVEFORM_BAR_COUNT }, () => 0);
}

export function acceptsSessionGeneration(current: number, incoming?: number): incoming is number {
  return incoming !== undefined && incoming >= current;
}

export function hudPartialFromEvent(
  currentGeneration: number,
  incomingGeneration: number | undefined,
  currentState: string,
  text: string | undefined,
): string | null | undefined {
  if (!acceptsSessionGeneration(currentGeneration, incomingGeneration)) return undefined;
  if (currentState === "idle") return undefined;
  const next = text?.trim() ?? "";
  return next.length > 0 ? next : null;
}

export function hudPartialAfterState(
  currentGeneration: number,
  incomingGeneration: number | undefined,
  nextState: string,
  currentPartial: string | null,
): string | null {
  if (!acceptsSessionGeneration(currentGeneration, incomingGeneration)) return currentPartial;
  if (incomingGeneration > currentGeneration || nextState === "idle") return null;
  return currentPartial;
}

export function normalizeChunkProgress(
  completed?: number,
  total?: number,
): { completed: number; total: number } | null {
  if (
    completed === undefined
    || total === undefined
    || !Number.isInteger(completed)
    || !Number.isInteger(total)
    || total <= 0
    || completed < 0
    || completed > total
  ) {
    return null;
  }
  return { completed, total };
}

export function hudProgressForState(
  next: string,
  phase: string,
  currentProgress: number,
): number {
  if (next === "idle" || next === "recording" || next === "starting") return 0;
  if (["done", "unverified", "copied", "degraded", "history"].includes(next)) return 1;
  if (next === "rate_limited") return currentProgress;
  if (next !== "processing") return currentProgress;
  if (phase === "asr") return 0.35;
  if (phase === "soniox_recovery") return 0.35;
  if (phase === "cascade_accurate") return 0.5;
  if (phase === "cleanup") return 0.65;
  if (phase === "delivery") return 0.9;
  return 0.05;
}

type HudContext = {
  contextApp: string | null;
  contextStyle: string | null;
  contextLabel: string | null;
  cleanupIntensity: "off" | "light" | "standard" | "heavy" | null;
};

export function hudContextFromEvent(
  nextState: string,
  current: HudContext,
  incoming: {
    context_app?: string | null;
    context_style?: string | null;
    context_label?: string | null;
    cleanup_intensity?: "off" | "light" | "standard" | "heavy" | null;
  },
): HudContext {
  if (nextState === "idle") {
    return { contextApp: null, contextStyle: null, contextLabel: null, cleanupIntensity: null };
  }
  return {
    contextApp: incoming.context_app !== undefined ? incoming.context_app ?? null : current.contextApp,
    contextStyle: incoming.context_style !== undefined ? incoming.context_style ?? null : current.contextStyle,
    contextLabel: incoming.context_label !== undefined ? incoming.context_label ?? null : current.contextLabel,
    cleanupIntensity: incoming.cleanup_intensity !== undefined
      ? incoming.cleanup_intensity ?? null
      : current.cleanupIntensity,
  };
}

export function hudContextSourceFromEvent(
  nextState: string,
  current: HudContextSource | null,
  incoming: { context_source?: ContextSourcePayload | null },
  newGeneration = false,
): HudContextSource | null {
  if (nextState === "idle" || nextState === "starting" || nextState === "recording" || nextState === "recording_limited") return null;
  if (incoming.context_source === undefined) return newGeneration ? null : current;
  const source = incoming.context_source?.source;
  return typeof source === "string" && (["none", "ax", "ocr", "cloud_vision"] as const).includes(source as HudContextSource)
    ? source as HudContextSource
    : null;
}

export function selectedActionStateForDictation(
  current: string | null,
  next: DictationState,
): string | null {
  if (!current) return null;
  if (next === "idle") {
    return current === "waiting_for_selection" ? current : null;
  }
  if (next === "starting" || next === "recording" || next === "processing") {
    return ["waiting_for_selection", "listening", "preparing_rewrite"].includes(current)
      ? current
      : null;
  }
  return current;
}

type LearnToast = { pair_key: string; pair_keys?: string[]; before: string; after: string };

export function IslandWindow() {
  const { t } = useI18n();
  const [learnToast, setLearnToast] = useState<LearnToast | null>(null);
  const hasLearnToast = learnToast != null;
  const [learnUndoError, setLearnUndoError] = useState<string | null>(null);
  const [learnUndoBusy, setLearnUndoBusy] = useState(false);
  const learnToastRef = useRef(learnToast);
  learnToastRef.current = learnToast;
  const [hud, setHud] = useState<HudState>({
    sessionGeneration: 0,
    state: "idle",
    phase: "idle",
    retryAfterSecs: null,
    undoAvailable: false,
    contextApp: null,
    contextStyle: null,
    contextLabel: null,
    contextSource: null,
    cleanupIntensity: null,
    translationTargetLanguage: null,
    fallbackReason: null,
    waveformLevels: emptyWaveform(),
    progress: 0,
    completedChunks: null,
    totalChunks: null,
    selectedActionState: null,
    partialText: null,
    recordingMode: null,
    recordingHotkey: null,
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
        setHud((current) => {
          if (!acceptsSessionGeneration(current.sessionGeneration, eventGeneration)) return current;
          const chunkProgress = normalizeChunkProgress(
            event.payload.completed_chunks,
            event.payload.total_chunks,
          );
          const context = hudContextFromEvent(next, current, event.payload);
          const contextSource = hudContextSourceFromEvent(
            next,
            current.contextSource,
            event.payload,
            eventGeneration > current.sessionGeneration,
          );
          const nextHud = {
            sessionGeneration: eventGeneration,
            state: next,
            recordingMode: event.payload.recording_mode ?? null,
            recordingHotkey: event.payload.recording_hotkey ?? null,
            phase,
            retryAfterSecs: event.payload.retry_after_secs ?? null,
            undoAvailable: event.payload.undo_available ?? false,
            contextApp: context.contextApp,
            contextStyle: context.contextStyle,
            contextLabel: context.contextLabel,
            contextSource,
            cleanupIntensity: context.cleanupIntensity,
            translationTargetLanguage: hudTranslationFromEvent(next, current.translationTargetLanguage, event.payload.translation_target_language),
            fallbackReason: next === "idle" ? null : event.payload.fallback_reason ?? null,
            // Audio starts before the final recording state is committed. Keep
            // the live waveform continuous through that short starting phase.
            waveformLevels: next === "starting" || next === "recording" ? current.waveformLevels : emptyWaveform(),
            progress: hudProgressForState(next, phase, current.progress),
            completedChunks: next === "processing" ? chunkProgress?.completed ?? null : null,
            totalChunks: next === "processing" ? chunkProgress?.total ?? null : null,
            selectedActionState: selectedActionStateForDictation(current.selectedActionState, next),
            partialText: hudPartialAfterState(
              current.sessionGeneration,
              eventGeneration,
              next,
              current.partialText,
            ),
          };
          return current.sessionGeneration === nextHud.sessionGeneration
            && current.state === nextHud.state
            && current.recordingMode === nextHud.recordingMode
            && current.recordingHotkey === nextHud.recordingHotkey
            && current.phase === nextHud.phase
            && current.retryAfterSecs === nextHud.retryAfterSecs
            && current.undoAvailable === nextHud.undoAvailable
            && current.contextApp === nextHud.contextApp
            && current.contextStyle === nextHud.contextStyle
            && current.contextLabel === nextHud.contextLabel
            && current.contextSource === nextHud.contextSource
            && current.cleanupIntensity === nextHud.cleanupIntensity
            && current.translationTargetLanguage === nextHud.translationTargetLanguage
            && current.fallbackReason === nextHud.fallbackReason
            && current.waveformLevels === nextHud.waveformLevels
            && current.progress === nextHud.progress
            && current.completedChunks === nextHud.completedChunks
            && current.totalChunks === nextHud.totalChunks
            && current.selectedActionState === nextHud.selectedActionState
            && current.partialText === nextHud.partialText
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
    register<{
      session_generation?: number;
      text?: string;
    }>("dictation://partial", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      setHud((current) => {
        const nextText = hudPartialFromEvent(
          current.sessionGeneration,
          eventGeneration,
          current.state,
          event.payload.text,
        );
        if (nextText === undefined) return current;
        return current.partialText === nextText && current.sessionGeneration === eventGeneration
          ? current
          : {
            ...current,
            sessionGeneration: eventGeneration,
            partialText: nextText,
          };
      });
    });
    register<{
      progress?: number;
      chunks_done?: number;
      total?: number;
      session_generation?: number;
    }>("dictation://progress", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      const nextProgress = Math.max(0, Math.min(1, event.payload.progress ?? 0));
      setHud((current) => {
        if (!acceptsSessionGeneration(current.sessionGeneration, eventGeneration)) return current;
        const chunkProgress = normalizeChunkProgress(event.payload.chunks_done, event.payload.total);
        const nextCompleted = chunkProgress?.completed ?? current.completedChunks;
        const nextTotal = chunkProgress?.total ?? current.totalChunks;
        return current.progress === nextProgress
          && current.completedChunks === nextCompleted
          && current.totalChunks === nextTotal
          ? current
          : {
            ...current,
            progress: nextProgress,
            completedChunks: nextCompleted,
            totalChunks: nextTotal,
          };
      });
    });
    register<{ max_recording_secs: number; session_generation?: number }>("audio://limit", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : {
          ...current,
          sessionGeneration: eventGeneration,
          state: "recording_limited",
          phase: "idle",
          retryAfterSecs: null,
          fallbackReason: null,
          waveformLevels: emptyWaveform(),
          completedChunks: null,
          totalChunks: null,
        });
    });
    register<{ retry_after_secs: number; session_generation?: number }>("quota://rate_limited", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(0, eventGeneration)) return;
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : {
          ...current,
          sessionGeneration: eventGeneration,
          state: "rate_limited",
          phase: "waiting_retry",
          retryAfterSecs: event.payload.retry_after_secs,
          fallbackReason: null,
          waveformLevels: emptyWaveform(),
          completedChunks: null,
          totalChunks: null,
        });
    });
    register<{ state?: string }>("selected-action://state", (event) => {
      setHud((current) => ({
        ...current,
        selectedActionState: event.payload.state && event.payload.state !== "idle" ? event.payload.state : null,
      }));
    });
    register<LearnToast>("learn_pairs://promoted", (event) => {
      learnToastRef.current = event.payload;
      setLearnToast(event.payload);
      setLearnUndoError(null);
    });

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
      unlisteners.clear();
    };
  }, []);

  useEffect(() => {
    if (!hasLearnToast) return;
    void invoke("set_island_learn_interactive", { interactive: true }).catch(() => undefined);
    return () => {
      void invoke("set_island_learn_interactive", { interactive: false }).catch(() => undefined);
    };
  }, [hasLearnToast]);

  const dismissLearnToast = () => {
    setLearnToast(null);
    setLearnUndoError(null);
    void invoke("hide_island_if_idle");
  };

  const undoLearning = async () => {
    if (!learnToast || learnUndoBusy) return;
    const toast = learnToast;
    setLearnUndoBusy(true);
    setLearnUndoError(null);
    try {
      await Promise.all((toast.pair_keys?.length ? toast.pair_keys : [toast.pair_key]).map((pairKey) => invoke("undo_learn_pair", { pairKey })));
      if (learnToastRef.current === toast) dismissLearnToast();
    } catch {
      if (learnToastRef.current === toast) setLearnUndoError(t("撤销未完成，请重试。"));
    } finally {
      setLearnUndoBusy(false);
    }
  };

  return (
    <div className="voice-pill-stage">
      <VoicePill recordingHint={hud.recordingHotkey ? tryItHint(hud.recordingMode ?? "tap", formatHotkeyDisplay(hud.recordingHotkey), t) : null} state={hud.state} phase={hud.phase} retryAfterSecs={hud.retryAfterSecs} undoAvailable={hud.undoAvailable} contextLabel={formatHudTranslationLabel(hud.translationTargetLanguage, t) ?? formatHudIntensityLabel(hud.contextApp, hud.cleanupIntensity, t) ?? formatHudContextLabel(hud.contextApp, hud.contextStyle, hud.contextLabel, t)} contextSourceLabel={formatHudContextSource(hud.contextSource, t)} fallbackReason={hud.fallbackReason} selectedActionState={hud.selectedActionState} waveformLevels={hud.waveformLevels} progress={hud.progress} chunkProgress={hud.completedChunks != null && hud.totalChunks != null ? { completed: hud.completedChunks, total: hud.totalChunks } : null} partialText={hud.partialText} reduced={reduced} />
      {learnToast && (
        <div role="status" aria-label={learnUndoError ?? undefined} className="island-learn-toast" style={{ height: voicePillLearnToastHeight }}>
          <span className="island-learn-toast__text" title={learnUndoError ?? `${t("已学")} ${learnToast.before}→${learnToast.after}`}>
            {learnUndoError ? t("撤销未完成") : `${t("已学")} ${learnToast.before}→${learnToast.after}`}
          </span>
          <button
            type="button"
            className="island-learn-toast__action"
            disabled={learnUndoBusy}
            onClick={() => void undoLearning()}
          >
            {t(learnUndoError ? "重试撤销" : "撤销")}
          </button>
          <button type="button" disabled={learnUndoBusy} className="island-learn-toast__action" onClick={dismissLearnToast}>{t("关闭")}</button>
        </div>
      )}
    </div>
  );
}

type DictationStatePayload = { recording_mode?: string | null; recording_hotkey?: string | null; translation_target_language?: string | null; state: DictationState; session_generation?: number; phase?: ProcessingPhase; retry_after_secs?: number; completed_chunks?: number; total_chunks?: number; cleanup_status?: string | null; undo_available?: boolean; context_id?: string; context_app?: string | null; context_style?: string | null; context_label?: string; context_source?: ContextSourcePayload | null; cleanup_intensity?: "off" | "light" | "standard" | "heavy" | null; delivery_method?: string; fallback_reason?: string | null };
