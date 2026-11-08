import type { OrbState } from "thinking-orbs";

export type DictationState =
  | "idle"
  | "starting"
  | "recording"
  | "recording_limited"
  | "processing"
  | "rate_limited"
  | "done"
  | "unverified"
  | "copied"
  | "degraded"
  | "history"
  | "error";

export type ProcessingPhase =
  | "finalizing_audio"
  | "asr"
  | "soniox_recovery"
  | "cascade_accurate"
  | "cleanup"
  | "delivery"
  | "waiting_retry"
  | "idle";

export function activeHudStatusLabel(
  state: DictationState,
  _phase: ProcessingPhase,
  _phaseConfirmed: boolean,
  translate: (source: string) => string,
): string | null {
  if (state === "starting") return translate("准备录音");
  if (state === "recording") return translate("录音中");
  if (state === "recording_limited") return translate("录音已达上限");
  if (state === "processing" || state === "rate_limited") return translate("处理中");
  return null;
}

export function shouldHideOnConfirmedPaste(payload: {
  state: DictationState;
  delivery_method?: string;
  fallback_reason?: string | null;
}): boolean {
  return payload.state === "done"
    && payload.delivery_method === "paste"
    && payload.fallback_reason == null;
}

export function hudStageTimerKey(
  generation: number,
  state: DictationState,
  phase: ProcessingPhase,
  phaseConfirmed: boolean,
): string {
  return `${generation}:${state}:${phase}:${phaseConfirmed ? "confirmed" : "unconfirmed"}`;
}

export function formatHudElapsed(elapsedMs: number): string {
  const seconds = Math.max(0, Math.floor(elapsedMs / 1_000));
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remaining = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remaining).padStart(2, "0")}`
    : `${String(minutes).padStart(2, "0")}:${String(remaining).padStart(2, "0")}`;
}

export type HudOrbConfig = {
  state: OrbState;
  paused: boolean;
  speed: number;
  dim: boolean;
};

export const HUD_ORB_SIZE = 20 as const;
/** Shared capsule motion stays inside the product's 150–300ms transition window. */
export const HUD_MOTION_MS = 220;
/** Exits finish before the shared enter/fade duration. */
export const HUD_LABEL_OUT_MS = 150;
export const HUD_LABEL_IN_DELAY_MS = 0;
export const HUD_LABEL_IN_DURATION_MS = HUD_MOTION_MS;
/** Incoming `--in` class must last this long (delay + fade), not just while outgoing exists. */
export const HUD_LABEL_FADE_MS = HUD_LABEL_IN_DELAY_MS + HUD_LABEL_IN_DURATION_MS;
export const HUD_THINK_BEAM_FADE_MS = HUD_MOTION_MS;
export const HUD_PILL_RADIUS_PX = 24;

const HUD_FINAL_STATES = new Set<DictationState>([
  "done",
  "copied",
  "history",
  "error",
  "unverified",
  "degraded",
]);

export type HudBorderBeamColor = "colorful" | "mono";
export type HudBorderBeamSize = "md" | "line";
export type HudBorderBeamConfig = {
  size: HudBorderBeamSize;
  colorVariant: HudBorderBeamColor;
  strength: number;
  duration?: number;
  brightness: number;
};

export const HUD_LISTENING_BEAM: HudBorderBeamConfig = {
  size: "md",
  colorVariant: "mono",
  strength: 1,
  duration: 1.96,
  brightness: 1.3,
};

export const HUD_THINKING_BEAM: HudBorderBeamConfig = {
  size: "line",
  colorVariant: "mono",
  strength: 0.65,
  duration: 3.1,
  brightness: 1.3,
};

export function hudBorderBeamFor(state: DictationState): HudBorderBeamConfig | null {
  if (state === "starting" || state === "recording" || state === "recording_limited") {
    return HUD_LISTENING_BEAM;
  }
  if (state === "processing" || state === "rate_limited") {
    return HUD_THINKING_BEAM;
  }
  return null;
}

export function hudStatusLabel(
  state: DictationState,
  orb: HudOrbConfig | null,
  options?: {
    doneCheck?: boolean;
    phase?: ProcessingPhase;
    phaseConfirmed?: boolean;
    translate?: (source: string) => string;
  },
): string | null {
  const translate = options?.translate ?? ((source: string) => source);
  if (state === "degraded") return translate("部分结果");
  if (options?.doneCheck || state === "done") return translate("已完成");
  if (["starting", "recording", "recording_limited", "processing", "rate_limited"].includes(state)) {
    return activeHudStatusLabel(
      state,
      options?.phase ?? "idle",
      options?.phaseConfirmed ?? false,
      translate,
    );
  }
  if (state === "copied") return translate("已复制");
  if (state === "unverified") return translate("结果未确认");
  if (state === "history") return translate("已保存到历史");
  if (state === "error") return translate("错误");
  if (orb) return translate("处理中");
  return null;
}

export function perceptualLevel(levels: number[]): number {
  const sample = levels.reduce((max, value) => Math.max(max, value), 0);
  const clamped = Math.max(0, Math.min(1, sample));
  return Math.min(1, Math.pow(clamped, 0.58));
}

export function hudAudioLevel(
  levels: number[],
  lastAudioLevelAtMs: number | null,
  clockNowMs: number,
): { fresh: boolean; level: number } {
  const fresh = lastAudioLevelAtMs != null
    && clockNowMs >= lastAudioLevelAtMs
    && clockNowMs - lastAudioLevelAtMs <= 750;
  return { fresh, level: fresh ? perceptualLevel(levels) : 0 };
}

export const HUD_LISTENING_ORB_SPEED = 1.35;
// Keep the approved 20px orb on the library's native speed curve.
export const HUD_THINKING_ORB_SPEED = 1;

export function hudOrbFor(
  state: DictationState,
  _phase: ProcessingPhase,
  options: {
    reduced: boolean;
    pageVisible?: boolean;
    level: number;
    levelFresh?: boolean;
    selectedActionState?: string | null;
  },
): HudOrbConfig | null {
  void options.selectedActionState;
  if (HUD_FINAL_STATES.has(state)) return null;
  const listening = ["starting", "recording", "recording_limited"].includes(state);
  const thinking = state === "processing" || state === "rate_limited";
  if (!listening && !thinking && state !== "idle") return null;
  const pauseMotion = options.reduced
    || options.pageVisible === false
    || state === "idle"
    || (listening && !options.levelFresh);
  return {
    state: thinking ? "working" : "breathing",
    paused: pauseMotion,
    speed: thinking ? HUD_THINKING_ORB_SPEED : HUD_LISTENING_ORB_SPEED,
    dim: state === "recording_limited",
  };
}
