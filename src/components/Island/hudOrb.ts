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
  | "cleanup"
  | "delivery"
  | "waiting_retry"
  | "idle";

export type HudOrbConfig = {
  state: OrbState;
  paused: boolean;
  speed: number;
  dim: boolean;
};

export const HUD_ORB_SIZE = 64 as const;
/** Thinking stays visible this long on Copied before the outgoing node unmounts. */
export const HUD_LABEL_OUT_MS = 320;
/** Copied stays at opacity 0 this long so Thinking can leave first. */
export const HUD_LABEL_IN_DELAY_MS = 120;
export const HUD_LABEL_IN_DURATION_MS = 400;
/** Incoming `--in` class must last this long (delay + fade), not just while outgoing exists. */
export const HUD_LABEL_FADE_MS = HUD_LABEL_IN_DELAY_MS + HUD_LABEL_IN_DURATION_MS;
export const HUD_CAPTION_REVEAL_DELAY_MS = 160;
export const HUD_CAPTION_REVEAL_MS = 420;
export const HUD_ORB_FADE_MS = 480;
export const HUD_THINK_BEAM_FADE_MS = 480;
export const HUD_PILL_RADIUS_PX = 20;

const HUD_FINAL_STATES = new Set<DictationState>([
  "done",
  "copied",
  "history",
  "error",
  "unverified",
  "degraded",
]);

export type HudBorderBeamColor = "colorful" | "sunset";
export type HudBorderBeamSize = "md" | "line";
export type HudBorderBeamConfig = {
  size: HudBorderBeamSize;
  colorVariant: HudBorderBeamColor;
  strength: number;
  duration?: number;
};

export const HUD_LISTENING_BEAM: HudBorderBeamConfig = {
  size: "md",
  colorVariant: "colorful",
  strength: 1,
  duration: 3.2,
};

export const HUD_THINKING_BEAM: HudBorderBeamConfig = {
  size: "line",
  colorVariant: "colorful",
  strength: 0.7,
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
  options?: { doneCheck?: boolean },
): string | null {
  if (options?.doneCheck || state === "done" || state === "degraded") return "Done";
  if (state === "starting" || state === "recording" || state === "recording_limited") {
    return "Listening…";
  }
  if (state === "copied" || state === "unverified") return "Copied";
  if (state === "history") return "Saved";
  if (state === "error") return "Error";
  if (orb) return "Thinking…";
  return null;
}

export function perceptualLevel(levels: number[]): number {
  const sample = levels.reduce((max, value) => Math.max(max, value), 0);
  const clamped = Math.max(0, Math.min(1, sample));
  return Math.min(1, Math.pow(clamped, 0.58));
}

export function shouldMountHudOrb(state: DictationState, hasCaption: boolean): boolean {
  if (HUD_FINAL_STATES.has(state)) return false;
  if (state === "idle") return hasCaption;
  return true;
}

export const HUD_LISTENING_ORB_SPEED = 1.35;
export const HUD_THINKING_ORB_SPEED = 1.2;

export function hudOrbFor(
  state: DictationState,
  _phase: ProcessingPhase,
  options: {
    reduced: boolean;
    level: number;
    selectedActionState?: string | null;
  },
): HudOrbConfig | null {
  void options.level;
  void options.selectedActionState;
  if (HUD_FINAL_STATES.has(state)) return null;
  const pauseMotion = options.reduced || state === "idle";
  const thinking = state === "processing" || state === "rate_limited";
  return {
    state: thinking ? "shaping" : "breathing",
    paused: pauseMotion,
    speed: thinking ? HUD_THINKING_ORB_SPEED : HUD_LISTENING_ORB_SPEED,
    dim: state === "recording_limited",
  };
}
