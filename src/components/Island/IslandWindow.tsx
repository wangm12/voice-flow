import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { tryItHint } from "../../lib/activationCopy";
import { formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { VoicePill } from "./VoicePill";
import { DictationCompletionStatus } from "./DictationCompletionStatus";
import type { VoicePillAction } from "./VoicePill";
import type { HudDestination } from "./hudPresentation";
import { hudStageTimerKey, shouldHideOnConfirmedPaste } from "./hudOrb";
import { integerRetrySeconds, voicePillLearnToastHeight } from "./voicePillTokens";
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
  quietCompletion: boolean;
  phase: ProcessingPhase;
  phaseConfirmed: boolean;
  retryAfterSecs: number | null;
  undoAvailable: boolean;
  contextApp: string | null;
  contextStyle: string | null;
  contextLabel: string | null;
  contextSource: HudContextSource | null;
  cleanupIntensity: "off" | "light" | "standard" | "heavy" | null;
  translationTargetLanguage: string | null;
  deliveryMethod: string | null;
  fallbackReason: string | null;
  cleanupStatus: string | null;
  waveformLevels: number[];
  lastAudioLevelAtMs: number | null;
  selectedActionState: string | null;
  partialText: string | null;
  recordingMode: string | null;
  recordingHotkey: string | null;
  audioCapture: AudioCapturePayload | null;
  audioRecovery: AudioRecoveryPayload | null;
  historyRecovery: HistoryRecoveryPayload | null;
  recordingTargetIcon: string | null;
};

type AudioCapturePayload = {
  session_generation: number;
  status: "waiting_first_frame" | "receiving_audio" | "no_audio_yet" | "no_recent_audio" | "device_error" | "unavailable" | "no_speech";
  device_name?: string | null;
  input_selection?: string;
  requested_selection?: string | null;
  clamshell_substituted?: boolean;
  system_default_name?: string | null;
};

type AudioRecoveryPayload = {
  ticket_revision: number;
  session_generation: number;
  outcome: "history_saved" | "audio_retained" | "unknown_failure" | "no_audio";
  recoverable_session_id?: string | null;
  save_error: boolean;
};

type HistoryRecoveryPayload = {
  ticket_revision: number;
  session_generation: number;
  delivery_transaction_id: string;
  history_transaction_id: string;
  history_state: "pending" | "saved" | "failed";
  copy_status: "ready" | "copying" | "copied" | "unknown" | "failed";
};

type HistoryRecoveryCopyReceipt = {
  ticket_revision: number;
  session_generation: number;
  delivery_transaction_id: string;
  history_transaction_id: string;
  copy_status: HistoryRecoveryPayload["copy_status"];
  reason_code: string | null;
};

function historyRecoveryTitle(recovery: HistoryRecoveryPayload): string {
  switch (recovery.history_state) {
    case "pending": return "听写文字已保留";
    case "saved": return "已保存到历史";
    case "failed": return "历史保存未确认";
  }
}

function historyRecoveryMessage(recovery: HistoryRecoveryPayload): string {
  if (recovery.copy_status === "unknown") {
    return "复制结果未确认；请先检查剪贴板，不要重复复制。";
  }
  if (recovery.copy_status === "copied") {
    return "听写文字已复制到剪贴板，请手动粘贴。";
  }
  if (recovery.history_state === "pending") {
    return "历史记录仍在保存；你也可以复制这条听写。";
  }
  if (recovery.history_state === "saved") {
    return "历史记录已确认保存；你可以复制这条听写。";
  }
  return "历史保存未确认；恢复文字仍可从本机记录中复制。";
}

function historyRecoveryActionLabel(recovery: HistoryRecoveryPayload): string {
  switch (recovery.copy_status) {
    case "copying": return "正在复制…";
    case "copied": return "已复制到剪贴板";
    case "unknown": return "复制状态未确认";
    case "failed": return "重试复制文字";
    case "ready": return "复制听写文字";
  }
}

export function recoveryDestinationForOutcome(
  outcome: AudioRecoveryPayload["outcome"],
): "history" | "microphone" {
  return outcome === "no_audio" ? "microphone" : "history";
}

export function recoveryMessageForOutcome(
  outcome: AudioRecoveryPayload["outcome"],
  saveError = false,
): string {
  switch (outcome) {
    case "history_saved": return "录音已保存到历史记录。";
    case "audio_retained": return saveError
      ? "未能保存到历史，恢复音频仍已保留。请重试。"
      : "恢复音频已保留，但未能确认历史保存。";
    case "unknown_failure": return "录音恢复状态未知，请检查历史记录。";
    case "no_audio": return "麦克风无法打开；更改输入设备后重试。";
  }
}

export function recoveryActionLabelForOutcome(outcome: AudioRecoveryPayload["outcome"]): string {
  switch (outcome) {
    case "history_saved": return "查看历史";
    case "audio_retained": return "保存到历史";
    case "unknown_failure": return "检查历史";
    case "no_audio": return "选择麦克风";
  }
}

export function recoveryHudTitleForOutcome(outcome: AudioRecoveryPayload["outcome"]): string {
  switch (outcome) {
    case "history_saved": return "录音已保存";
    case "audio_retained": return "录音仍可恢复";
    case "unknown_failure": return "录音恢复状态未知";
    case "no_audio": return "麦克风无法打开";
  }
}

function recordingIconFromEvent(
  state: DictationState,
  current: string | null,
  incoming: string | null | undefined,
  newGeneration: boolean,
): string | null {
  if (state !== "recording") return null;
  if (typeof incoming === "string"
    && incoming.startsWith("data:image/png;base64,")
    && incoming.length <= 350_000) return incoming;
  return newGeneration ? null : current;
}

function audioCaptureCaption(
  status: AudioCapturePayload["status"] | null,
  state: DictationState,
  translate: (source: string) => string,
): string | null {
  if (state !== "starting" && state !== "recording" && state !== "recording_limited") return null;
  if (!status && state === "starting") return null;
  if (!status) return null;
  const label = {
    waiting_first_frame: "已打开麦克风，等待音频",
    receiving_audio: "正在接收麦克风音频",
    no_audio_yet: "尚未收到麦克风音频",
    no_recent_audio: "最近未收到新的麦克风音频",
    device_error: "麦克风输入中断",
    unavailable: "无法打开麦克风",
    no_speech: "未检测到声音",
  }[status];
  return translate(label);
}

type AudioDeviceSource = "effective" | "selected" | "unknown";

export function audioCapturePresentation(
  capture: AudioCapturePayload | null,
  state: DictationState,
  translate: (source: string) => string,
): {
  status: string | null;
  deviceName: string | null;
  deviceSource: AudioDeviceSource;
  deviceSourceLabel: string;
  selectionLabel: string;
  deviceTitle: string;
} {
  const effectiveName = capture?.device_name?.trim() || null;
  const requestedSelection = typeof capture?.requested_selection === "string"
    ? capture.requested_selection
    : capture?.input_selection;
  const selectedName = requestedSelection?.trim() || null;
  const followsSystemDefault = typeof capture?.requested_selection === "string"
    ? capture.requested_selection.length === 0
    : !selectedName;
  const systemDefaultName = capture?.system_default_name?.trim() || null;
  const deviceSource: AudioDeviceSource = effectiveName
    ? "effective"
    : selectedName
      ? "selected"
      : "unknown";
  const deviceSourceLabel = deviceSource === "effective"
    ? translate("实际")
    : deviceSource === "selected"
      ? translate("所选")
      : translate("未知");
  const selectionLabel = selectedName
    ? `${translate("所选")}：${selectedName}`
    : translate("默认输入");
  const selectionDescription = selectedName
    ? `${translate("所选输入设备")}：${selectedName}`
    : followsSystemDefault && systemDefaultName
      ? `${translate("默认输入")}：${systemDefaultName}`
      : translate("默认输入");
  const deviceTitle = [
    effectiveName ? `${translate("实际录音设备")}：${effectiveName}` : null,
    !effectiveName && !selectedName ? translate("录音设备未知") : null,
    selectionDescription,
  ].filter((part): part is string => Boolean(part)).join(" · ") || translate("设备名称不可用");
  return {
    status: audioCaptureCaption(capture?.status ?? null, state, translate),
    deviceName: effectiveName ?? selectedName,
    deviceSource,
    deviceSourceLabel,
    selectionLabel,
    deviceTitle,
  };
}

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

function LearningHudGlyph() {
  return (
    <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 3.5h9.5A2.5 2.5 0 0 1 16 6v10.5H6.5A2.5 2.5 0 0 1 4 14V3.5Z" />
      <path d="M4 14a2.5 2.5 0 0 1 2.5-2.5H16M8 7h5" />
    </svg>
  );
}

export function IslandWindow() {
  const { t } = useI18n();
  const [learnToast, setLearnToast] = useState<LearnToast | null>(null);
  const hasLearnToast = learnToast != null;
  const [learnUndoError, setLearnUndoError] = useState<string | null>(null);
  const [learnUndoBusy, setLearnUndoBusy] = useState(false);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [recoveryError, setRecoveryError] = useState<"save" | "open" | "dismiss" | "history_copy" | "history_dismiss" | null>(null);
  const [historyCopyBusy, setHistoryCopyBusy] = useState(false);
  const [hudAction, setHudAction] = useState<{ generation: number; busy: boolean; error: string | null }>({ generation: 0, busy: false, error: null });
  const [detailsAction, setDetailsAction] = useState<{ generation: number; busy: boolean; error: string | null }>({ generation: 0, busy: false, error: null });
  const [detailsOwner, setDetailsOwner] = useState<{ generation: number; open: boolean }>({ generation: 0, open: false });
  const learnToastRef = useRef(learnToast);
  learnToastRef.current = learnToast;
  const [hud, setHud] = useState<HudState>({
    sessionGeneration: 0,
    state: "idle",
    quietCompletion: false,
    phase: "idle",
    phaseConfirmed: false,
    retryAfterSecs: null,
    undoAvailable: false,
    contextApp: null,
    contextStyle: null,
    contextLabel: null,
    contextSource: null,
    cleanupIntensity: null,
    translationTargetLanguage: null,
    deliveryMethod: null,
    fallbackReason: null,
    cleanupStatus: null,
    waveformLevels: emptyWaveform(),
    lastAudioLevelAtMs: null,
    selectedActionState: null,
    partialText: null,
    recordingMode: null,
    recordingHotkey: null,
    audioCapture: null,
    audioRecovery: null,
    historyRecovery: null,
    recordingTargetIcon: null,
  });
  const hudRef = useRef(hud);
  useEffect(() => {
    hudRef.current = hud;
  }, [hud]);
  const admittedHudStateRef = useRef({ sessionGeneration: hud.sessionGeneration, state: hud.state });
  const recoveryGenerationRef = useRef(0);
  const recoveryClearedThroughRef = useRef({ sessionGeneration: 0, ticketRevision: 0 });
  const latestRecoveryRevisionRef = useRef({ sessionGeneration: 0, ticketRevision: 0 });
  const latestHistoryRecoveryRef = useRef({
    sessionGeneration: 0,
    ticketRevision: 0,
    deliveryTransactionId: null as string | null,
    historyTransactionId: null as string | null,
  });
  const dismissedHistoryRecoveryGenerationRef = useRef(0);
  const hiddenHistoryRecoveryGenerationRef = useRef(0);
  const openedRecoveryRevisionRef = useRef({ sessionGeneration: 0, ticketRevision: 0 });
  const openedHistoryRecoveryRevisionRef = useRef({ sessionGeneration: 0, ticketRevision: 0 });
  const selectedActionEventRef = useRef({ sessionGeneration: 0, actionSequence: 0 });
  const selectedActionStateRef = useRef<{ sessionGeneration: number; state: string | null }>({
    sessionGeneration: 0,
    state: null,
  });
  const reduced = useReducedMotionPreference();
  const timedHudState = ["starting", "recording", "recording_limited"].includes(hud.state);
  const stageTimerKey = hudStageTimerKey(
    hud.sessionGeneration,
    hud.state,
    hud.phase,
    hud.phaseConfirmed,
  );
  const [stageElapsedMs, setStageElapsedMs] = useState(0);
  const [clockNowMs, setClockNowMs] = useState(() => Date.now());
  const audioDisplay = audioCapturePresentation(hud.audioCapture, hud.state, t);

  useEffect(() => {
    if (!timedHudState) {
      setStageElapsedMs(0);
      setClockNowMs(Date.now());
      return undefined;
    }
    const startedAt = Date.now();
    setStageElapsedMs(0);
    setClockNowMs(startedAt);
    const timer = window.setInterval(() => {
      const now = Date.now();
      setStageElapsedMs(now - startedAt);
      setClockNowMs(now);
    }, 250);
    return () => window.clearInterval(timer);
  }, [stageTimerKey, timedHudState]);

  useEffect(() => {
    if (hud.state !== "processing" && hud.state !== "rate_limited") return undefined;
    const generation = hud.sessionGeneration;
    const timer = window.setTimeout(() => {
      setHud((current) => current.sessionGeneration !== generation
        || (current.state !== "processing" && current.state !== "rate_limited")
        ? current
        : {
          ...current,
          contextApp: null,
          contextStyle: null,
          contextLabel: null,
          contextSource: null,
          cleanupIntensity: null,
          translationTargetLanguage: null,
        });
    }, 2_500);
    return () => window.clearTimeout(timer);
  }, [hud.sessionGeneration, hud.state]);

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

    function adoptRecoveryGeneration(generation: number, state = admittedHudStateRef.current.state) {
      if (generation <= recoveryGenerationRef.current) return;
      recoveryGenerationRef.current = generation;
      admittedHudStateRef.current = { sessionGeneration: generation, state };
      recoveryClearedThroughRef.current = { sessionGeneration: generation, ticketRevision: 0 };
      latestRecoveryRevisionRef.current = { sessionGeneration: generation, ticketRevision: 0 };
      latestHistoryRecoveryRef.current = {
        sessionGeneration: generation,
        ticketRevision: 0,
        deliveryTransactionId: null,
        historyTransactionId: null,
      };
      dismissedHistoryRecoveryGenerationRef.current = 0;
      hiddenHistoryRecoveryGenerationRef.current = 0;
      setDetailsOwner((current) => current.generation < generation ? { generation, open: false } : current);
      setHudAction((current) => current.generation < generation ? { generation, busy: false, error: null } : current);
      setDetailsAction((current) => current.generation < generation ? { generation, busy: false, error: null } : current);
      setRecoveryBusy(false);
      setHistoryCopyBusy(false);
      setRecoveryError(null);
    }

    register<DictationStatePayload>("dictation://state", (event) => {
        const eventGeneration = event.payload.session_generation;
        if (!acceptsSessionGeneration(0, eventGeneration)) return;
        if (!acceptsSessionGeneration(hudRef.current.sessionGeneration, eventGeneration)) return;
        if (!acceptsSessionGeneration(recoveryGenerationRef.current, eventGeneration)) return;
        const selectedActionVersion = selectedActionEventRef.current;
        if (eventGeneration < selectedActionVersion.sessionGeneration) return;
        if (eventGeneration > selectedActionVersion.sessionGeneration) {
          selectedActionEventRef.current = { sessionGeneration: eventGeneration, actionSequence: 0 };
          selectedActionStateRef.current = { sessionGeneration: eventGeneration, state: null };
        }
        if (eventGeneration > recoveryGenerationRef.current) {
          recoveryGenerationRef.current = eventGeneration;
          recoveryClearedThroughRef.current = { sessionGeneration: eventGeneration, ticketRevision: 0 };
          latestRecoveryRevisionRef.current = { sessionGeneration: eventGeneration, ticketRevision: 0 };
          latestHistoryRecoveryRef.current = {
            sessionGeneration: eventGeneration,
            ticketRevision: 0,
            deliveryTransactionId: null,
            historyTransactionId: null,
          };
          dismissedHistoryRecoveryGenerationRef.current = 0;
          hiddenHistoryRecoveryGenerationRef.current = 0;
        }
        const next = event.payload.state;
        const quietSuccessCandidate = shouldHideOnConfirmedPaste(event.payload);
        const currentHud = hudRef.current;
        const selectedActionState = selectedActionStateRef.current.sessionGeneration === eventGeneration
          ? selectedActionStateRef.current.state
          : eventGeneration === currentHud.sessionGeneration ? currentHud.selectedActionState : null;
        const uncertainPasteAttempt = next === "unverified"
          && event.payload.delivery_method === "paste_unverified"
          && selectedActionState == null;
        const terminalPasteOutcome = quietSuccessCandidate || uncertainPasteAttempt;
        const nextAudioRecovery = eventGeneration > currentHud.sessionGeneration
          ? null
          : currentHud.audioRecovery;
        const nextHistoryRecovery = terminalPasteOutcome
          || eventGeneration > currentHud.sessionGeneration
          ? null
          : currentHud.historyRecovery;
        const quietCompletion = (next === "idle"
          && eventGeneration === currentHud.sessionGeneration
          && currentHud.quietCompletion)
          || (quietSuccessCandidate
            && nextAudioRecovery == null
            && nextHistoryRecovery == null);
        admittedHudStateRef.current = {
          sessionGeneration: eventGeneration,
          state: quietCompletion ? "idle" : next,
        };
        if (terminalPasteOutcome) {
          hiddenHistoryRecoveryGenerationRef.current = eventGeneration;
          setHistoryCopyBusy(false);
          setRecoveryError((current) => current === "history_copy" || current === "history_dismiss"
            ? null
            : current);
          if (nextAudioRecovery == null) {
            setRecoveryBusy(false);
            setDetailsOwner((current) => current.generation === eventGeneration
              ? { generation: eventGeneration, open: false }
              : current);
          }
        }
        const phase = event.payload.phase ?? (next === "processing" ? "cleanup" : next === "idle" ? "idle" : "finalizing_audio");
        const phaseConfirmed = event.payload.phase_confirmed === true;
        setHud((current) => {
          if (!acceptsSessionGeneration(current.sessionGeneration, eventGeneration)) return current;
          const preserveImmediateHide = next === "idle"
            && eventGeneration === current.sessionGeneration
            && current.quietCompletion;
          const nextAudioRecovery = eventGeneration > current.sessionGeneration
            ? null
            : current.audioRecovery;
          const nextHistoryRecovery = terminalPasteOutcome
            || eventGeneration > current.sessionGeneration
            ? null
            : current.historyRecovery;
          const quietCompletion = preserveImmediateHide
            || (quietSuccessCandidate
              && nextAudioRecovery == null
              && nextHistoryRecovery == null);
          const displayState = quietCompletion ? "idle" : next;
          const context = hudContextFromEvent(displayState, current, event.payload);
          const contextSource = hudContextSourceFromEvent(
            displayState,
            current.contextSource,
            event.payload,
            eventGeneration > current.sessionGeneration,
          );
          const nextHud = {
            sessionGeneration: eventGeneration,
            state: displayState,
            quietCompletion,
            recordingMode: event.payload.recording_mode ?? null,
            recordingHotkey: event.payload.recording_hotkey ?? null,
            recordingTargetIcon: recordingIconFromEvent(
              displayState,
              current.recordingTargetIcon,
              event.payload.recording_target_icon,
              eventGeneration > current.sessionGeneration,
            ),
            audioCapture: displayState === "idle" || eventGeneration > current.sessionGeneration
              ? null
              : current.audioCapture,
            audioRecovery: nextAudioRecovery,
            historyRecovery: nextHistoryRecovery,
            phase,
            phaseConfirmed,
            retryAfterSecs: integerRetrySeconds(event.payload.retry_after_secs),
            undoAvailable: event.payload.undo_available ?? false,
            contextApp: context.contextApp,
            contextStyle: context.contextStyle,
            contextLabel: context.contextLabel,
            contextSource,
            cleanupIntensity: context.cleanupIntensity,
            translationTargetLanguage: hudTranslationFromEvent(displayState, current.translationTargetLanguage, event.payload.translation_target_language),
            deliveryMethod: displayState === "idle" ? null : event.payload.delivery_method ?? null,
            fallbackReason: displayState === "idle" ? null : event.payload.fallback_reason ?? null,
            cleanupStatus: displayState === "idle" ? null : event.payload.cleanup_status ?? null,
            // Audio starts before the final recording state is committed. Keep
            // the live waveform continuous through that short starting phase.
            waveformLevels: eventGeneration > current.sessionGeneration
              ? emptyWaveform()
              : next === "starting" || next === "recording"
                ? current.waveformLevels
                : emptyWaveform(),
            lastAudioLevelAtMs: eventGeneration > current.sessionGeneration
              ? null
              : next === "starting" || next === "recording"
                ? current.lastAudioLevelAtMs
                : null,
            selectedActionState: selectedActionStateForDictation(current.selectedActionState, displayState),
            partialText: hudPartialAfterState(
              current.sessionGeneration,
              eventGeneration,
              displayState,
              current.partialText,
            ),
          };
          return current.sessionGeneration === nextHud.sessionGeneration
            && current.state === nextHud.state
            && current.quietCompletion === nextHud.quietCompletion
            && current.recordingMode === nextHud.recordingMode
            && current.recordingHotkey === nextHud.recordingHotkey
            && current.recordingTargetIcon === nextHud.recordingTargetIcon
            && current.phase === nextHud.phase
            && current.phaseConfirmed === nextHud.phaseConfirmed
            && current.retryAfterSecs === nextHud.retryAfterSecs
            && current.undoAvailable === nextHud.undoAvailable
            && current.contextApp === nextHud.contextApp
            && current.contextStyle === nextHud.contextStyle
            && current.contextLabel === nextHud.contextLabel
            && current.contextSource === nextHud.contextSource
            && current.cleanupIntensity === nextHud.cleanupIntensity
            && current.translationTargetLanguage === nextHud.translationTargetLanguage
            && current.deliveryMethod === nextHud.deliveryMethod
            && current.fallbackReason === nextHud.fallbackReason
            && current.cleanupStatus === nextHud.cleanupStatus
            && current.waveformLevels === nextHud.waveformLevels
            && current.lastAudioLevelAtMs === nextHud.lastAudioLevelAtMs
            && current.selectedActionState === nextHud.selectedActionState
            && current.partialText === nextHud.partialText
            && current.historyRecovery === nextHud.historyRecovery
            ? current
            : nextHud;
        });
    });
    register<{ session_generation?: number }>("dictation://hud-dismissed", (event) => {
      const generation = event.payload.session_generation;
      if (!Number.isSafeInteger(generation)
        || generation === 0
        || generation !== recoveryGenerationRef.current) return;
      admittedHudStateRef.current = { sessionGeneration: generation, state: "idle" };
      setHud((current) => current.sessionGeneration !== generation
        ? current
        : {
          ...current,
          state: "idle",
          quietCompletion: true,
          phase: "idle",
          phaseConfirmed: false,
          retryAfterSecs: null,
          undoAvailable: false,
          contextApp: null,
          contextStyle: null,
          contextLabel: null,
          contextSource: null,
          cleanupIntensity: null,
          translationTargetLanguage: null,
          deliveryMethod: null,
          fallbackReason: null,
          cleanupStatus: null,
          waveformLevels: emptyWaveform(),
          lastAudioLevelAtMs: null,
          selectedActionState: null,
          partialText: null,
          audioCapture: null,
          audioRecovery: null,
          historyRecovery: null,
          recordingTargetIcon: null,
        });
      setDetailsOwner((current) => current.generation === generation
        ? { generation, open: false }
        : current);
      setHudAction((current) => current.generation === generation
        ? { generation, busy: false, error: null }
        : current);
      setDetailsAction((current) => current.generation === generation
        ? { generation, busy: false, error: null }
        : current);
      setRecoveryBusy(false);
      setRecoveryError(null);
    });
    register<{ level?: number; session_generation?: number; frame_fresh?: boolean }>("audio://level", (event) => {
      const eventGeneration = event.payload.session_generation;
      const nextLevel = Math.max(0, Math.min(1, event.payload.level ?? 0));
      setHud((current) => {
        if (eventGeneration !== current.sessionGeneration) return current;
        if (current.state !== "starting" && current.state !== "recording") {
          return current;
        }
        if (event.payload.frame_fresh !== true) {
          const alreadyEmpty = current.lastAudioLevelAtMs == null
            && current.waveformLevels.every((level) => level === 0);
          return alreadyEmpty
            ? current
            : { ...current, waveformLevels: emptyWaveform(), lastAudioLevelAtMs: null };
        }
        const waveformLevels = [...current.waveformLevels.slice(1), nextLevel];
        return { ...current, waveformLevels, lastAudioLevelAtMs: Date.now() };
      });
    });
    register<AudioRecoveryPayload>("audio://recovery", (event) => {
      const payload = event.payload;
      if (!Number.isInteger(payload.ticket_revision)
        || payload.ticket_revision <= 0
        || !Number.isInteger(payload.session_generation)
        || typeof payload.save_error !== "boolean"
        || !["history_saved", "audio_retained", "unknown_failure", "no_audio"].includes(payload.outcome)) return;
      const latestRevision = latestRecoveryRevisionRef.current;
      if (payload.session_generation !== recoveryGenerationRef.current
        || (recoveryClearedThroughRef.current.sessionGeneration === payload.session_generation
          && payload.ticket_revision <= recoveryClearedThroughRef.current.ticketRevision)
        || (latestRevision.sessionGeneration === payload.session_generation
          && payload.ticket_revision < latestRevision.ticketRevision)) return;
      latestRecoveryRevisionRef.current = {
        sessionGeneration: payload.session_generation,
        ticketRevision: payload.ticket_revision,
      };
      const hasRecoverableSession = typeof payload.recoverable_session_id === "string"
        && /^[A-Za-z0-9_-]{1,128}$/.test(payload.recoverable_session_id);
      const safePayload: AudioRecoveryPayload = payload.outcome === "audio_retained" && !hasRecoverableSession
        ? { ...payload, outcome: "unknown_failure", recoverable_session_id: null, save_error: true }
        : payload.outcome === "audio_retained"
          ? payload
          : { ...payload, recoverable_session_id: null };
      setHud((current) => {
        if (safePayload.session_generation !== current.sessionGeneration
          || (recoveryClearedThroughRef.current.sessionGeneration === safePayload.session_generation
            && safePayload.ticket_revision <= recoveryClearedThroughRef.current.ticketRevision)
          || (current.audioRecovery != null
            && safePayload.ticket_revision < current.audioRecovery.ticket_revision)) return current;
        return {
          ...current,
          audioRecovery: safePayload,
        };
      });
      setRecoveryBusy(false);
      setRecoveryError(null);
    });
    register<HistoryRecoveryPayload>("dictation://history-recovery", (event) => {
      const payload = event.payload;
      if (!Number.isSafeInteger(payload.ticket_revision)
        || payload.ticket_revision <= 0
        || !Number.isSafeInteger(payload.session_generation)
        || payload.session_generation <= 0
        || !/^[0-9a-f]{32}$/.test(payload.delivery_transaction_id)
        || !/^[A-Za-z0-9-]{1,96}$/.test(payload.history_transaction_id)
        || !["pending", "saved", "failed"].includes(payload.history_state)
        || !["ready", "copying", "copied", "unknown", "failed"].includes(payload.copy_status)
        || payload.session_generation !== recoveryGenerationRef.current
        || payload.session_generation === dismissedHistoryRecoveryGenerationRef.current
        || payload.session_generation === hiddenHistoryRecoveryGenerationRef.current) return;
      const latest = latestHistoryRecoveryRef.current;
      if (latest.sessionGeneration === payload.session_generation
        && (payload.ticket_revision <= latest.ticketRevision
          || (latest.deliveryTransactionId != null
            && (latest.deliveryTransactionId !== payload.delivery_transaction_id
              || latest.historyTransactionId !== payload.history_transaction_id)))) return;
      const currentHud = hudRef.current;
      if (currentHud.sessionGeneration === payload.session_generation) {
        const previous = currentHud.historyRecovery;
        if (previous) {
          if (previous.delivery_transaction_id !== payload.delivery_transaction_id
            || previous.history_transaction_id !== payload.history_transaction_id
            || payload.ticket_revision <= previous.ticket_revision) return;
        } else if (currentHud.quietCompletion
          || !["error", "unverified", "history", "copied", "degraded", "processing"].includes(currentHud.state)) {
          return;
        }
      }
      latestHistoryRecoveryRef.current = {
        sessionGeneration: payload.session_generation,
        ticketRevision: payload.ticket_revision,
        deliveryTransactionId: payload.delivery_transaction_id,
        historyTransactionId: payload.history_transaction_id,
      };
      setHud((current) => {
        if (current.sessionGeneration !== payload.session_generation
          || payload.session_generation === hiddenHistoryRecoveryGenerationRef.current) return current;
        const previous = current.historyRecovery;
        if (previous) {
          if (previous.delivery_transaction_id !== payload.delivery_transaction_id
            || previous.history_transaction_id !== payload.history_transaction_id
            || payload.ticket_revision <= previous.ticket_revision) return current;
        } else if (current.quietCompletion
          || !["error", "unverified", "history", "copied", "degraded", "processing"].includes(current.state)) {
          // A late storage result may update a recovery surface already shown
          // for this exact transaction, but it cannot resurrect a hidden HUD.
          return current;
        }
        return { ...current, historyRecovery: payload };
      });
      setDetailsOwner((current) => current.generation === payload.session_generation
        && payload.session_generation !== hiddenHistoryRecoveryGenerationRef.current
        ? { generation: payload.session_generation, open: true }
        : current);
      setRecoveryBusy(payload.copy_status === "copying");
      setRecoveryError(null);
    });
    register<AudioCapturePayload>("audio://capture", (event) => {
      const payload = event.payload;
      if (!Number.isInteger(payload.session_generation)) return;
      if (!("waiting_first_frame" === payload.status
        || "receiving_audio" === payload.status
        || "no_audio_yet" === payload.status
        || "no_recent_audio" === payload.status
        || "device_error" === payload.status
        || "unavailable" === payload.status
        || "no_speech" === payload.status)) return;
      setHud((current) => {
        if (payload.session_generation !== current.sessionGeneration) return current;
        if (!["starting", "recording", "recording_limited", "processing", "error"].includes(current.state)) return current;
        return { ...current, audioCapture: payload };
      });
    });
    register<{
      session_generation?: number;
      text?: string;
    }>("dictation://partial", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(hudRef.current.sessionGeneration, eventGeneration)
        || !acceptsSessionGeneration(recoveryGenerationRef.current, eventGeneration)
        || hudPartialFromEvent(
          admittedHudStateRef.current.sessionGeneration,
          eventGeneration,
          admittedHudStateRef.current.state,
          event.payload.text,
        ) === undefined) return;
      if (eventGeneration > recoveryGenerationRef.current) adoptRecoveryGeneration(eventGeneration);
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
            audioRecovery: eventGeneration > current.sessionGeneration ? null : current.audioRecovery,
            historyRecovery: eventGeneration > current.sessionGeneration ? null : current.historyRecovery,
            audioCapture: eventGeneration > current.sessionGeneration ? null : current.audioCapture,
            partialText: nextText,
          };
      });
    });
    register<{ session_generation?: number }>("dictation://progress", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(hudRef.current.sessionGeneration, eventGeneration)
        || !acceptsSessionGeneration(recoveryGenerationRef.current, eventGeneration)) return;
      if (eventGeneration > recoveryGenerationRef.current) adoptRecoveryGeneration(eventGeneration);
      setHud((current) => {
        if (!acceptsSessionGeneration(current.sessionGeneration, eventGeneration)) return current;
        if (current.sessionGeneration === eventGeneration) return current;
        return {
          ...current,
          sessionGeneration: eventGeneration,
          audioRecovery: null,
          historyRecovery: null,
          audioCapture: null,
        };
      });
    });
    register<{ max_recording_secs: number; session_generation?: number }>("audio://limit", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(hudRef.current.sessionGeneration, eventGeneration)
        || !acceptsSessionGeneration(recoveryGenerationRef.current, eventGeneration)) return;
      if (eventGeneration > recoveryGenerationRef.current) adoptRecoveryGeneration(eventGeneration, "recording_limited");
      admittedHudStateRef.current = { sessionGeneration: eventGeneration!, state: "recording_limited" };
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : {
          ...current,
          sessionGeneration: eventGeneration,
          audioRecovery: eventGeneration > current.sessionGeneration ? null : current.audioRecovery,
          historyRecovery: eventGeneration > current.sessionGeneration ? null : current.historyRecovery,
          audioCapture: eventGeneration > current.sessionGeneration ? null : current.audioCapture,
          state: "recording_limited",
          phase: "idle",
          retryAfterSecs: null,
          fallbackReason: null,
          waveformLevels: emptyWaveform(),
          lastAudioLevelAtMs: null,
        });
    });
    register<{ retry_after_secs: number; session_generation?: number }>("quota://rate_limited", (event) => {
      const eventGeneration = event.payload.session_generation;
      if (!acceptsSessionGeneration(hudRef.current.sessionGeneration, eventGeneration)
        || !acceptsSessionGeneration(recoveryGenerationRef.current, eventGeneration)
        || eventGeneration < selectedActionEventRef.current.sessionGeneration) return;
      if (eventGeneration > recoveryGenerationRef.current) adoptRecoveryGeneration(eventGeneration, "rate_limited");
      admittedHudStateRef.current = { sessionGeneration: eventGeneration!, state: "rate_limited" };
      setHud((current) => !acceptsSessionGeneration(current.sessionGeneration, eventGeneration)
        ? current
        : {
          ...current,
          sessionGeneration: eventGeneration,
          audioRecovery: eventGeneration > current.sessionGeneration ? null : current.audioRecovery,
          historyRecovery: eventGeneration > current.sessionGeneration ? null : current.historyRecovery,
          audioCapture: eventGeneration > current.sessionGeneration ? null : current.audioCapture,
          state: "rate_limited",
          phase: "waiting_retry",
          retryAfterSecs: integerRetrySeconds(event.payload.retry_after_secs),
          fallbackReason: null,
          waveformLevels: emptyWaveform(),
          lastAudioLevelAtMs: null,
        });
    });
    register<{ state?: string; session_generation?: number; action_sequence?: number }>("selected-action://state", (event) => {
      const { state, session_generation: sessionGeneration, action_sequence: actionSequence } = event.payload;
      if (!Number.isSafeInteger(sessionGeneration) || sessionGeneration == null) return;
      const currentVersion = selectedActionEventRef.current;
      if (sessionGeneration < currentVersion.sessionGeneration) return;
      const sequenceIsValid = Number.isSafeInteger(actionSequence) && (actionSequence ?? 0) > 0;
      if (state !== "idle" && !sequenceIsValid) return;
      if (sessionGeneration > currentVersion.sessionGeneration) {
        selectedActionEventRef.current = { sessionGeneration, actionSequence: 0 };
      }
      const nextVersion = selectedActionEventRef.current;
      if (sequenceIsValid && actionSequence! < nextVersion.actionSequence) return;
      if (!sequenceIsValid && nextVersion.actionSequence > 0) return;
      if (sequenceIsValid) {
        selectedActionEventRef.current = { sessionGeneration, actionSequence: actionSequence! };
      }
      selectedActionStateRef.current = {
        sessionGeneration,
        state: state && state !== "idle" ? state : null,
      };
      setHud((current) => current.sessionGeneration !== sessionGeneration
        ? current
        : {
          ...current,
          selectedActionState: state && state !== "idle" ? state : null,
        });
    });
    register<LearnToast>("learn_pairs://promoted", (event) => {
      learnToastRef.current = event.payload;
      setLearnToast(event.payload);
      setLearnUndoError(null);
      setLearnUndoBusy(false);
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

  useEffect(() => {
    setRecoveryBusy(false);
    setHistoryCopyBusy(false);
    setRecoveryError(null);
    setHudAction({ generation: hud.sessionGeneration, busy: false, error: null });
    setDetailsAction({ generation: hud.sessionGeneration, busy: false, error: null });
    setDetailsOwner({ generation: hud.sessionGeneration, open: false });
  }, [hud.sessionGeneration]);

  useEffect(() => {
    const recovery = hud.audioRecovery;
    if (!recovery
      || (openedRecoveryRevisionRef.current.sessionGeneration === recovery.session_generation
        && openedRecoveryRevisionRef.current.ticketRevision === recovery.ticket_revision)) return;
    openedRecoveryRevisionRef.current = {
      sessionGeneration: recovery.session_generation,
      ticketRevision: recovery.ticket_revision,
    };
    setDetailsOwner({ generation: recovery.session_generation, open: true });
  }, [hud.audioRecovery]);

  useEffect(() => {
    const recovery = hud.historyRecovery;
    if (!recovery
      || (openedHistoryRecoveryRevisionRef.current.sessionGeneration === recovery.session_generation
        && openedHistoryRecoveryRevisionRef.current.ticketRevision === recovery.ticket_revision)) return;
    openedHistoryRecoveryRevisionRef.current = {
      sessionGeneration: recovery.session_generation,
      ticketRevision: recovery.ticket_revision,
    };
    setDetailsOwner({ generation: recovery.session_generation, open: true });
  }, [hud.historyRecovery]);

  const detailsOpen = detailsOwner.open
    && detailsOwner.generation === hud.sessionGeneration
    && !hud.quietCompletion;
  useEffect(() => {
    if (hud.sessionGeneration === 0) return;
    void Promise.resolve(invoke("set_island_details_visible", {
      expectedGeneration: detailsOwner.generation,
      visible: detailsOpen,
    })).catch(() => undefined);
  }, [detailsOpen, detailsOwner.generation, hud.sessionGeneration]);

  const handleHudAction = async (action: VoicePillAction, expectedGeneration: number) => {
    const current = hudRef.current;
    if (current.sessionGeneration !== expectedGeneration
      || !["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(current.state)
      || (hudAction.generation === expectedGeneration && hudAction.busy)) return;
    setHudAction({ generation: expectedGeneration, busy: true, error: null });
    try {
      await invoke(action === "stop" ? "stop_dictation_from_hud" : "cancel_dictation_from_hud", {
        expectedGeneration,
      });
    } catch {
      if (hudRef.current.sessionGeneration === expectedGeneration) {
        setHudAction({
          generation: expectedGeneration,
          busy: false,
          error: t(action === "stop" ? "无法停止录音，请重试" : "无法取消听写，请重试"),
        });
        setDetailsOwner({ generation: expectedGeneration, open: true });
      }
      return;
    }
    setHudAction((currentAction) => currentAction.generation === expectedGeneration
      ? { ...currentAction, busy: false }
      : currentAction);
  };

  const handleDetailsAction = async (destination: HudDestination, expectedGeneration: number) => {
    const current = hudRef.current;
    if (current.sessionGeneration !== expectedGeneration
      || current.quietCompletion
      || (current.state === "idle" && current.selectedActionState !== "accessibility_required")
      || ["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(current.state)
      || (detailsAction.generation === expectedGeneration && detailsAction.busy)) return;
    setDetailsAction({ generation: expectedGeneration, busy: true, error: null });
    try {
      await invoke("open_hud_destination", { destination, expectedGeneration });
    } catch {
      if (hudRef.current.sessionGeneration === expectedGeneration) {
        setDetailsAction({
          generation: expectedGeneration,
          busy: false,
          error: t("无法打开目标设置或历史记录，请重试。"),
        });
      }
      return;
    }
    setDetailsAction((currentAction) => currentAction.generation === expectedGeneration
      ? { ...currentAction, busy: false }
      : currentAction);
  };

  const dismissLearnToast = () => {
    setLearnToast(null);
    setLearnUndoError(null);
    void invoke("hide_island_if_idle");
  };

  const dismissRecovery = async (recovery: AudioRecoveryPayload) => {
    if (!isCurrentRecovery(recovery)) return;
    try {
      await invoke("dismiss_audio_recovery", { ticketRevision: recovery.ticket_revision });
      rememberClearedRecoveryRevision(recovery);
      setHud((current) => current.audioRecovery?.ticket_revision === recovery.ticket_revision
        ? { ...current, audioRecovery: null }
        : current);
      setRecoveryError(null);
    } catch {
      if (isCurrentRecovery(recovery)) setRecoveryError("dismiss");
    }
  };

  const rememberClearedRecoveryRevision = (recovery: AudioRecoveryPayload) => {
    if (recoveryGenerationRef.current !== recovery.session_generation) return;
    const cleared = recoveryClearedThroughRef.current;
    const ticketRevision = cleared.sessionGeneration === recovery.session_generation
      ? Math.max(cleared.ticketRevision, recovery.ticket_revision)
      : recovery.ticket_revision;
    recoveryClearedThroughRef.current = {
      sessionGeneration: recovery.session_generation,
      ticketRevision,
    };
    const latest = latestRecoveryRevisionRef.current;
    if (latest.sessionGeneration === recovery.session_generation) {
      latestRecoveryRevisionRef.current = {
        sessionGeneration: recovery.session_generation,
        ticketRevision: Math.max(latest.ticketRevision, ticketRevision),
      };
    }
  };

  const isCurrentRecovery = (recovery: AudioRecoveryPayload) => {
    const current = hudRef.current;
    return current.sessionGeneration === recovery.session_generation
      && current.audioRecovery?.ticket_revision === recovery.ticket_revision
      && current.audioRecovery?.session_generation === recovery.session_generation
      && current.audioRecovery?.outcome === recovery.outcome
      && current.audioRecovery?.recoverable_session_id === recovery.recoverable_session_id;
  };

  const openRecoveryDestination = async (
    recovery: AudioRecoveryPayload,
    fromRetry = false,
    previousRevision?: number,
  ) => {
    if (!fromRetry && !isCurrentRecovery(recovery)) return;
    const destination = recoveryDestinationForOutcome(recovery.outcome);
    try {
      await invoke("open_recovery_destination", {
        destination,
        ticketRevision: recovery.ticket_revision,
      });
      rememberClearedRecoveryRevision(recovery);
      setHud((current) => current.sessionGeneration === recovery.session_generation
        && (current.audioRecovery?.ticket_revision === recovery.ticket_revision
          || (fromRetry && current.audioRecovery?.ticket_revision === previousRevision))
        ? { ...current, audioRecovery: null }
        : current);
      setRecoveryError(null);
    } catch {
      const current = hudRef.current;
      if ((fromRetry && current.sessionGeneration === recovery.session_generation
        && (current.audioRecovery?.ticket_revision === recovery.ticket_revision
          || current.audioRecovery?.ticket_revision === previousRevision))
        || isCurrentRecovery(recovery)) setRecoveryError("open");
    }
  };

  const handleRecoveryAction = async (recovery: AudioRecoveryPayload) => {
    if (recoveryBusy || !isCurrentRecovery(recovery)) return;
    if (recovery.outcome === "audio_retained" && recovery.recoverable_session_id) {
      setRecoveryBusy(true);
      setRecoveryError(null);
      try {
        const saved = await invoke<AudioRecoveryPayload>("restore_audio_recovery_to_history", {
          sessionId: recovery.recoverable_session_id,
          ticketRevision: recovery.ticket_revision,
        });
        if (saved.session_generation !== recovery.session_generation
          || saved.ticket_revision <= recovery.ticket_revision
          || saved.outcome !== "history_saved") return;
        setHud((current) => current.sessionGeneration === recovery.session_generation
          && current.audioRecovery?.ticket_revision === recovery.ticket_revision
          ? { ...current, audioRecovery: saved }
          : current);
        await openRecoveryDestination(saved, true, recovery.ticket_revision);
      } catch {
        if (isCurrentRecovery(recovery)) setRecoveryError("save");
      } finally {
        setRecoveryBusy(false);
      }
      return;
    }
    await openRecoveryDestination(recovery);
  };

  const isCurrentHistoryRecovery = (recovery: HistoryRecoveryPayload) => {
    const current = hudRef.current;
    return current.sessionGeneration === recovery.session_generation
      && current.historyRecovery?.session_generation === recovery.session_generation
      && current.historyRecovery?.delivery_transaction_id === recovery.delivery_transaction_id
      && current.historyRecovery?.history_transaction_id === recovery.history_transaction_id
      && current.historyRecovery?.ticket_revision === recovery.ticket_revision;
  };

  const historyRecoveryIdentityIsCurrent = (recovery: HistoryRecoveryPayload) => {
    const current = hudRef.current;
    return current.sessionGeneration === recovery.session_generation
      && current.historyRecovery?.session_generation === recovery.session_generation
      && current.historyRecovery?.delivery_transaction_id === recovery.delivery_transaction_id
      && current.historyRecovery?.history_transaction_id === recovery.history_transaction_id;
  };

  const handleHistoryRecoveryAction = async (recovery: HistoryRecoveryPayload) => {
    if (!isCurrentHistoryRecovery(recovery)
      || historyCopyBusy
      || !["ready", "failed"].includes(recovery.copy_status)) return;
    setHistoryCopyBusy(true);
    setRecoveryError(null);
    try {
      const receipt = await invoke<HistoryRecoveryCopyReceipt>("copy_history_recovery", {
        request: {
          ticketRevision: recovery.ticket_revision,
          sessionGeneration: recovery.session_generation,
          deliveryTransactionId: recovery.delivery_transaction_id,
          historyTransactionId: recovery.history_transaction_id,
        },
      });
      if (!historyRecoveryIdentityIsCurrent(recovery)) return;
      const latest = latestHistoryRecoveryRef.current;
      if (latest.sessionGeneration === receipt.session_generation
        && latest.deliveryTransactionId === receipt.delivery_transaction_id
        && latest.historyTransactionId === receipt.history_transaction_id
        && receipt.ticket_revision > latest.ticketRevision) {
        latestHistoryRecoveryRef.current = { ...latest, ticketRevision: receipt.ticket_revision };
      }
      setHud((current) => {
        const active = current.historyRecovery;
        if (current.sessionGeneration !== receipt.session_generation
          || active?.delivery_transaction_id !== receipt.delivery_transaction_id
          || active?.history_transaction_id !== receipt.history_transaction_id
          || receipt.ticket_revision < active.ticket_revision) return current;
        return {
          ...current,
          historyRecovery: {
            ...active,
            ticket_revision: receipt.ticket_revision,
            copy_status: receipt.copy_status,
          },
        };
      });
      if (receipt.copy_status === "failed") setRecoveryError("history_copy");
    } catch {
      if (historyRecoveryIdentityIsCurrent(recovery)) setRecoveryError("history_copy");
    } finally {
      setHistoryCopyBusy(false);
    }
  };

  const dismissHistoryRecovery = async (recovery: HistoryRecoveryPayload) => {
    if (!isCurrentHistoryRecovery(recovery) || historyCopyBusy) return;
    try {
      await invoke("dismiss_history_recovery", {
        request: {
          ticketRevision: recovery.ticket_revision,
          sessionGeneration: recovery.session_generation,
          deliveryTransactionId: recovery.delivery_transaction_id,
          historyTransactionId: recovery.history_transaction_id,
        },
      });
      dismissedHistoryRecoveryGenerationRef.current = recovery.session_generation;
      setHud((current) => current.historyRecovery?.ticket_revision === recovery.ticket_revision
        && current.historyRecovery?.delivery_transaction_id === recovery.delivery_transaction_id
        && current.historyRecovery?.history_transaction_id === recovery.history_transaction_id
        ? { ...current, historyRecovery: null }
        : current);
      setRecoveryError(null);
    } catch {
      if (isCurrentHistoryRecovery(recovery)) setRecoveryError("history_dismiss");
    }
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
    <div className={`voice-pill-stage${hasLearnToast ? " voice-pill-stage--learning" : ""}`}>
      <VoicePill
        recordingHint={hud.recordingHotkey ? tryItHint(hud.recordingMode ?? "tap", formatHotkeyDisplay(hud.recordingHotkey), t) : null}
        audioStatus={audioDisplay.status}
        audioCaptureStatus={hud.audioCapture?.status ?? null}
        cleanupStatus={hud.cleanupStatus}
        recoveryOutcome={hud.audioRecovery?.outcome ?? null}
        deliveryMethod={hud.deliveryMethod}
        audioDeviceName={audioDisplay.deviceName}
        audioDeviceSourceLabel={audioDisplay.deviceSourceLabel}
        audioDeviceTitle={audioDisplay.deviceTitle}
        recordingTargetIcon={hud.recordingTargetIcon}
        state={hud.state}
        phase={hud.phase}
        phaseConfirmed={hud.phaseConfirmed}
        hideImmediately={hud.quietCompletion}
        stageElapsedMs={stageElapsedMs}
        clockNowMs={clockNowMs}
        lastAudioLevelAtMs={hud.lastAudioLevelAtMs}
        retryAfterSecs={hud.retryAfterSecs}
        undoAvailable={hud.undoAvailable}
        contextLabel={formatHudTranslationLabel(hud.translationTargetLanguage, t) ?? formatHudIntensityLabel(hud.contextApp, hud.cleanupIntensity, t) ?? formatHudContextLabel(hud.contextApp, hud.contextStyle, hud.contextLabel, t)}
        contextSourceLabel={formatHudContextSource(hud.contextSource, t)}
        fallbackReason={hud.fallbackReason}
        selectedActionState={hud.selectedActionState}
        waveformLevels={hud.waveformLevels}
        partialText={hud.partialText}
        reduced={reduced}
        sessionGeneration={hud.sessionGeneration}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={(open) => setDetailsOwner({ generation: hud.sessionGeneration, open })}
        onActiveAction={handleHudAction}
        actionBusy={hudAction.generation === hud.sessionGeneration && hudAction.busy}
        actionError={hudAction.generation === hud.sessionGeneration ? hudAction.error : null}
        detailsActionBusy={detailsAction.generation === hud.sessionGeneration && detailsAction.busy}
        detailsActionError={detailsAction.generation === hud.sessionGeneration ? detailsAction.error : null}
        onDetailsAction={handleDetailsAction}
        recoveryAction={hud.historyRecovery ? {
          title: t(historyRecoveryTitle(hud.historyRecovery)),
          message: t(historyRecoveryMessage(hud.historyRecovery)),
          actionLabel: t(historyRecoveryActionLabel(hud.historyRecovery)),
          busy: historyCopyBusy || hud.historyRecovery.copy_status === "copying",
          disabled: hud.historyRecovery.copy_status === "copied"
            || hud.historyRecovery.copy_status === "unknown",
          error: recoveryError === "history_copy"
            ? t("未能复制恢复文字；你可以重试。")
            : recoveryError === "history_dismiss"
              ? t("无法关闭恢复提示，请重试。")
              : null,
          onAction: () => void handleHistoryRecoveryAction(hud.historyRecovery!),
          onDismiss: () => void dismissHistoryRecovery(hud.historyRecovery!),
        } : hud.audioRecovery ? {
          title: t(recoveryHudTitleForOutcome(hud.audioRecovery.outcome)),
          message: t(recoveryMessageForOutcome(hud.audioRecovery.outcome, hud.audioRecovery.save_error)),
          actionLabel: t(recoveryActionLabelForOutcome(hud.audioRecovery.outcome)),
          busy: recoveryBusy,
          error: recoveryError === "save"
            ? t("保存到历史失败，请重试。")
            : recoveryError === "open"
            ? t("无法打开目标设置或历史记录，请重试。")
            : recoveryError === "dismiss"
              ? t("无法关闭恢复提示，请重试。")
              : null,
          onAction: () => void handleRecoveryAction(hud.audioRecovery!),
          onDismiss: () => void dismissRecovery(hud.audioRecovery!),
        } : undefined}
      />
      <DictationCompletionStatus />
      {learnToast && (
        <div role="status" aria-busy={learnUndoBusy} aria-label={learnUndoError ?? undefined} className="island-learn-toast" style={{ height: voicePillLearnToastHeight }}>
          <span className="island-learn-toast__icon"><LearningHudGlyph /></span>
          <span className="island-learn-toast__copy">
            <span className="island-learn-toast__title">
              {learnUndoError ? t("撤销尚未完成") : learnUndoBusy ? t("正在撤销学习") : t("已记住这个词")}
            </span>
            <span className="island-learn-toast__subtitle" title={learnUndoError ? t("可以再次尝试") : learnUndoBusy ? t("请稍候") : `${learnToast.before}→${learnToast.after}`}>
              {learnUndoError ? t("可以再次尝试") : learnUndoBusy ? t("请稍候") : `${learnToast.before}→${learnToast.after}`}
            </span>
          </span>
          <button
            type="button"
            className="island-learn-toast__action"
            disabled={learnUndoBusy}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => void undoLearning()}
          >
            {t(learnUndoError ? "重试撤销" : "撤销")}
          </button>
          <button type="button" disabled={learnUndoBusy} className="island-learn-toast__action" onMouseDown={(event) => event.preventDefault()} onClick={dismissLearnToast}>{t("关闭")}</button>
        </div>
      )}
    </div>
  );
}

type DictationStatePayload = { recording_mode?: string | null; recording_hotkey?: string | null; recording_target_icon?: string | null; translation_target_language?: string | null; state: DictationState; session_generation?: number; phase?: ProcessingPhase; phase_confirmed?: boolean; retry_after_secs?: number; cleanup_status?: string | null; undo_available?: boolean; context_id?: string; context_app?: string | null; context_style?: string | null; context_label?: string; context_source?: ContextSourcePayload | null; cleanup_intensity?: "off" | "light" | "standard" | "heavy" | null; delivery_method?: string; fallback_reason?: string | null };
