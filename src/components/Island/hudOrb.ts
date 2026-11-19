import type { OrbState } from "thinking-orbs";
import type { VoiceBeamColorVariant, VoiceBeamType } from "voice-glow";
import type {
  HudColorVariant,
  HudProcessingStyle,
  HudVoiceStyle,
} from "../../types/settings";

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

export type ChunkProgress = { done: number; total: number } | null;

export const HUD_STARTUP_NOTICE_DELAY_MS = 250;

export function activeHudStatusLabel(
  state: DictationState,
  phase: ProcessingPhase,
  phaseConfirmed: boolean,
  translate: (source: string) => string,
  options?: { startupElapsedMs?: number },
): string | null {
  if (state === "starting") {
    return (options?.startupElapsedMs ?? 0) >= HUD_STARTUP_NOTICE_DELAY_MS
      ? translate("准备录音")
      : translate("录音中");
  }
  if (state === "recording") return translate("录音中");
  if (state === "recording_limited") return translate("录音已达上限");
  if (state === "rate_limited") return translate("限流重试");
  if (state === "processing") {
    if (phaseConfirmed && phase === "waiting_retry") {
      return translate("限流重试");
    }
    return translate("处理中");
  }
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
  if (state === "recording" || state === "recording_limited") {
    return `${generation}:recording`;
  }
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
export const HUD_LABEL_IN_DELAY_MS = 0;
export const HUD_LABEL_IN_DURATION_MS = HUD_MOTION_MS;
/** Incoming `--in` class must last this long (delay + fade), not just while outgoing exists. */
export const HUD_LABEL_FADE_MS = HUD_LABEL_IN_DELAY_MS + HUD_LABEL_IN_DURATION_MS;
export const HUD_PILL_RADIUS_PX = 24;

const HUD_FINAL_STATES = new Set<DictationState>([
  "done",
  "copied",
  "history",
  "error",
  "unverified",
  "degraded",
]);

export type HudBorderBeamColor = "colorful" | "mono" | "ocean" | "sunset";
export type HudBorderBeamSize = "line";
export type HudBorderBeamConfig = {
  size: HudBorderBeamSize;
  colorVariant: HudBorderBeamColor;
  strength: number;
  duration?: number;
  brightness: number;
};

export type HudVoiceBandColors = {
  core: string;
  above: string;
  mid: string;
  below: string;
};

export type HudVoiceBeamConfig = {
  type: VoiceBeamType;
  colorVariant: VoiceBeamColorVariant;
  colors?: [string, string, string];
  bandColors?: HudVoiceBandColors;
  reach: number;
  spread: number;
  flow: number;
  bend: number;
  distortion: number;
  attack: number;
  release: number;
  threshold: number;
  idle: number;
  innerOpacity: number;
  bloomOpacity: number;
  bloomHeight: number;
  rangeWidth: number;
  rangeHeight: number;
  bandStrength: number;
  bandTail: number;
  brightness: number;
  saturation: number;
};

export type HudVisualSettings = {
  colorVariant: HudColorVariant;
  processingColorVariant: HudColorVariant;
  voiceStyle: HudVoiceStyle;
  processingStyle: HudProcessingStyle;
  voiceIntensity: number;
};

export const DEFAULT_HUD_VISUALS: HudVisualSettings = {
  colorVariant: "colorful",
  processingColorVariant: "colorful",
  voiceStyle: "ripple",
  processingStyle: "flow_line",
  voiceIntensity: 1,
};

export type HudColorVariantOption = {
  id: HudColorVariant;
  label: string;
  swatches: readonly [string, string, string];
  colors?: readonly [string, string, string];
  bandColors?: HudVoiceBandColors;
  borderBeamColor: HudBorderBeamColor;
  borderHueRotateDeg: number;
};

export const HUD_COLOR_VARIANTS: readonly HudColorVariantOption[] = [
  {
    id: "colorful",
    label: "全彩流光",
    swatches: ["#f9d84a", "#e84a9b", "#2fb8ff"],
    borderBeamColor: "colorful",
    borderHueRotateDeg: 0,
  },
  {
    id: "ocean",
    label: "深海蔚蓝",
    swatches: ["#56ccf2", "#2f80ed", "#4c51bf"],
    colors: ["#56ccf2", "#2f80ed", "#4c51bf"],
    bandColors: { core: "#e0f7ff", above: "#56ccf2", mid: "#2f80ed", below: "#4c51bf" },
    borderBeamColor: "ocean",
    borderHueRotateDeg: 0,
  },
  {
    id: "sunset",
    label: "日落琥珀",
    swatches: ["#ff9a5a", "#ff5e7e", "#9b4dca"],
    colors: ["#ff9a5a", "#ff5e7e", "#9b4dca"],
    bandColors: { core: "#fff0db", above: "#ff9a5a", mid: "#ff5e7e", below: "#9b4dca" },
    borderBeamColor: "sunset",
    borderHueRotateDeg: 0,
  },
  {
    id: "forest",
    label: "森屿翠绿",
    swatches: ["#84fab0", "#2ecc71", "#11998e"],
    colors: ["#84fab0", "#2ecc71", "#11998e"],
    bandColors: { core: "#e6fff2", above: "#84fab0", mid: "#2ecc71", below: "#11998e" },
    borderBeamColor: "ocean",
    borderHueRotateDeg: -95,
  },
  {
    id: "candy",
    label: "糖果绯粉",
    swatches: ["#ff9a9e", "#f770c4", "#a18cd1"],
    colors: ["#ff9a9e", "#f770c4", "#a18cd1"],
    bandColors: { core: "#fff0fa", above: "#ff9a9e", mid: "#f770c4", below: "#a18cd1" },
    borderBeamColor: "ocean",
    borderHueRotateDeg: 80,
  },
  {
    id: "ice",
    label: "冰晶浅蓝",
    swatches: ["#e0f7fa", "#80deea", "#5b9bd5"],
    colors: ["#e0f7fa", "#80deea", "#5b9bd5"],
    bandColors: { core: "#ffffff", above: "#e0f7fa", mid: "#80deea", below: "#5b9bd5" },
    borderBeamColor: "ocean",
    borderHueRotateDeg: -35,
  },
  {
    id: "gold",
    label: "琥珀鎏金",
    swatches: ["#fce38a", "#f5af19", "#d97706"],
    colors: ["#fce38a", "#f5af19", "#d97706"],
    bandColors: { core: "#fffbeb", above: "#fce38a", mid: "#f5af19", below: "#d97706" },
    borderBeamColor: "sunset",
    borderHueRotateDeg: 22,
  },
  {
    id: "mono",
    label: "极简单色",
    swatches: ["#f5f5f5", "#a3a3a3", "#525252"],
    colors: ["#ffffff", "#a3a3a3", "#525252"],
    bandColors: { core: "#ffffff", above: "#e5e5e5", mid: "#a3a3a3", below: "#737373" },
    borderBeamColor: "mono",
    borderHueRotateDeg: 0,
  },
] as const;

export type HudVoiceStylePreset = {
  id: HudVoiceStyle;
  label: string;
  description: string;
  config: Omit<HudVoiceBeamConfig, "colorVariant">;
};

export const HUD_VOICE_STYLES: readonly HudVoiceStylePreset[] = [
  {
    id: "ripple",
    label: "灵动波纹",
    description: "贴合音节起伏，清晰饱满",
    config: {
      type: "pill",
      reach: 1.28,
      spread: 1.15,
      flow: 56,
      bend: 38,
      distortion: 0.55,
      attack: 0.045,
      release: 0.16,
      threshold: 0.02,
      idle: 0.26,
      innerOpacity: 0.48,
      bloomOpacity: 0.68,
      bloomHeight: 0.9,
      rangeWidth: 0.86,
      rangeHeight: 0.68,
      bandStrength: 1.85,
      bandTail: 0.35,
      brightness: 1.3,
      saturation: 1.4,
    },
  },
  {
    id: "aurora",
    label: "极光涌动",
    description: "更大振幅与色彩横向流动",
    config: {
      type: "pill",
      reach: 1.5,
      spread: 1.28,
      flow: 72,
      bend: 48,
      distortion: 0.78,
      attack: 0.055,
      release: 0.2,
      threshold: 0.02,
      idle: 0.3,
      innerOpacity: 0.56,
      bloomOpacity: 0.78,
      bloomHeight: 1.02,
      rangeWidth: 0.95,
      rangeHeight: 0.76,
      bandStrength: 1.55,
      bandTail: 0.52,
      brightness: 1.42,
      saturation: 1.55,
    },
  },
  {
    id: "prism",
    label: "棱镜脉冲",
    description: "利落亮边与快速回落反馈",
    config: {
      type: "pill",
      reach: 1.38,
      spread: 1.05,
      flow: 84,
      bend: 52,
      distortion: 0.38,
      attack: 0.035,
      release: 0.13,
      threshold: 0.025,
      idle: 0.24,
      innerOpacity: 0.44,
      bloomOpacity: 0.64,
      bloomHeight: 0.86,
      rangeWidth: 0.82,
      rangeHeight: 0.66,
      bandStrength: 2.2,
      bandTail: 0.65,
      brightness: 1.35,
      saturation: 1.45,
    },
  },
  {
    id: "whisper",
    label: "柔雾微光",
    description: "低打扰轻量呼吸感",
    config: {
      type: "pill",
      reach: 1.05,
      spread: 0.92,
      flow: 30,
      bend: 24,
      distortion: 0.32,
      attack: 0.075,
      release: 0.24,
      threshold: 0.025,
      idle: 0.2,
      innerOpacity: 0.34,
      bloomOpacity: 0.5,
      bloomHeight: 0.76,
      rangeWidth: 0.76,
      rangeHeight: 0.56,
      bandStrength: 1.05,
      bandTail: 0.22,
      brightness: 1.15,
      saturation: 1.2,
    },
  },
] as const;

export type HudProcessingStylePreset = {
  id: HudProcessingStyle;
  label: string;
  description: string;
  config: Omit<HudBorderBeamConfig, "colorVariant">;
};

export const HUD_PROCESSING_STYLES: readonly HudProcessingStylePreset[] = [
  {
    id: "flow_line",
    label: "流动光束",
    description: "平衡流畅的底部巡航光线",
    config: {
      size: "line",
      strength: 1,
      duration: 2.4,
      brightness: 1.3,
    },
  },
  {
    id: "brilliant_sweep",
    label: "流光疾扫",
    description: "更高亮度与紧凑扫光节奏",
    config: {
      size: "line",
      strength: 1.25,
      duration: 1.45,
      brightness: 1.55,
    },
  },
  {
    id: "quiet_trace",
    label: "静谧微光",
    description: "轻柔克制的慢速光痕",
    config: {
      size: "line",
      strength: 0.72,
      duration: 3.4,
      brightness: 1.05,
    },
  },
] as const;

export function normalizeHudVisuals(visuals?: {
  hud_color_variant?: string | null;
  hud_processing_color_variant?: string | null;
  hud_voice_style?: string | null;
  hud_processing_style?: string | null;
  hud_voice_intensity?: number | null;
} | Partial<HudVisualSettings>): HudVisualSettings {
  const rawColor = visuals && "hud_color_variant" in visuals
    ? visuals.hud_color_variant
    : visuals && "colorVariant" in visuals
      ? visuals.colorVariant
      : undefined;
  const rawProcColor = visuals && "hud_processing_color_variant" in visuals
    ? visuals.hud_processing_color_variant
    : visuals && "processingColorVariant" in visuals
      ? visuals.processingColorVariant
      : visuals && "colorVariant" in visuals && !("hud_color_variant" in visuals)
        ? visuals.colorVariant
        : undefined;
  const rawVoice = visuals && "hud_voice_style" in visuals
    ? visuals.hud_voice_style
    : visuals && "voiceStyle" in visuals
      ? visuals.voiceStyle
      : undefined;
  const rawProc = visuals && "hud_processing_style" in visuals
    ? visuals.hud_processing_style
    : visuals && "processingStyle" in visuals
      ? visuals.processingStyle
      : undefined;
  const rawIntensity = visuals && "hud_voice_intensity" in visuals
    ? visuals.hud_voice_intensity
    : visuals && "voiceIntensity" in visuals
      ? visuals.voiceIntensity
      : undefined;

  const colorVariant = HUD_COLOR_VARIANTS.some((item) => item.id === rawColor)
    ? (rawColor as HudColorVariant)
    : DEFAULT_HUD_VISUALS.colorVariant;
  const processingColorVariant = HUD_COLOR_VARIANTS.some((item) => item.id === rawProcColor)
    ? (rawProcColor as HudColorVariant)
    : DEFAULT_HUD_VISUALS.processingColorVariant;
  const voiceStyle = HUD_VOICE_STYLES.some((item) => item.id === rawVoice)
    ? (rawVoice as HudVoiceStyle)
    : DEFAULT_HUD_VISUALS.voiceStyle;
  const processingStyle = HUD_PROCESSING_STYLES.some((item) => item.id === rawProc)
    ? (rawProc as HudProcessingStyle)
    : DEFAULT_HUD_VISUALS.processingStyle;
  const voiceIntensity = typeof rawIntensity === "number" && Number.isFinite(rawIntensity)
    ? Math.max(0.5, Math.min(1.5, rawIntensity))
    : DEFAULT_HUD_VISUALS.voiceIntensity;

  return {
    colorVariant,
    processingColorVariant,
    voiceStyle,
    processingStyle,
    voiceIntensity,
  };
}

function colorVariantMeta(colorVariant: HudColorVariant): HudColorVariantOption {
  return HUD_COLOR_VARIANTS.find((item) => item.id === colorVariant) ?? HUD_COLOR_VARIANTS[0];
}

export function resolveVoiceBeamConfig(visuals?: Partial<HudVisualSettings>): HudVoiceBeamConfig {
  const normalized = normalizeHudVisuals(visuals);
  const colorMeta = colorVariantMeta(normalized.colorVariant);
  const preset = HUD_VOICE_STYLES.find((item) => item.id === normalized.voiceStyle) ?? HUD_VOICE_STYLES[0];
  return {
    ...preset.config,
    colorVariant: normalized.colorVariant,
    ...(colorMeta.colors ? { colors: [...colorMeta.colors] as [string, string, string] } : {}),
    ...(colorMeta.bandColors ? { bandColors: colorMeta.bandColors } : {}),
  };
}

export function resolveProcessingBeamConfig(visuals?: Partial<HudVisualSettings>): HudBorderBeamConfig {
  const normalized = normalizeHudVisuals(visuals);
  const colorMeta = colorVariantMeta(normalized.processingColorVariant);
  const preset = HUD_PROCESSING_STYLES.find((item) => item.id === normalized.processingStyle) ?? HUD_PROCESSING_STYLES[0];
  return {
    ...preset.config,
    colorVariant: colorMeta.borderBeamColor,
  };
}

export function hudProcessingHueRotateDeg(colorVariant: HudColorVariant = "colorful"): number {
  return colorVariantMeta(colorVariant).borderHueRotateDeg;
}

export const HUD_VOICE_BEAM: HudVoiceBeamConfig = resolveVoiceBeamConfig(DEFAULT_HUD_VISUALS);

export function hudVoiceBeamFor(
  state: DictationState,
  visuals?: Partial<HudVisualSettings>,
): HudVoiceBeamConfig | null {
  if (state === "starting" || state === "recording" || state === "recording_limited") {
    return visuals ? resolveVoiceBeamConfig(visuals) : HUD_VOICE_BEAM;
  }
  return null;
}

export const HUD_PROCESSING_BEAM: HudBorderBeamConfig = resolveProcessingBeamConfig(DEFAULT_HUD_VISUALS);

export function hudBorderBeamFor(
  state: DictationState,
  visuals?: Partial<HudVisualSettings>,
): HudBorderBeamConfig | null {
  if (state === "processing") {
    return visuals ? resolveProcessingBeamConfig(visuals) : HUD_PROCESSING_BEAM;
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
    startupElapsedMs?: number;
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
      { startupElapsedMs: options?.startupElapsedMs },
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

/**
 * Uncompressed latest-sample level drive for VoiceBeam so consecutive speech
 * syllables rise and dip cleanly instead of pinning against voice-glow's
 * exponential saturation ceiling.
 */
export function hudVoiceDriveLevel(
  levels: number[],
  lastAudioLevelAtMs: number | null,
  clockNowMs: number,
  intensity = 1,
): number {
  const fresh = lastAudioLevelAtMs != null
    && clockNowMs >= lastAudioLevelAtMs
    && clockNowMs - lastAudioLevelAtMs <= 750;
  if (!fresh || levels.length === 0) return 0;
  const raw = levels[levels.length - 1];
  const clamped = Number.isFinite(raw) ? Math.max(0, Math.min(1, raw)) : 0;
  const gate = 0.02;
  if (clamped <= gate) return 0;
  const gated = (clamped - gate) / (1 - gate);
  const clampedIntensity = Number.isFinite(intensity)
    ? Math.max(0.5, Math.min(1.5, intensity))
    : 1;
  return Math.max(0, Math.min(1, Math.pow(gated, 1.55) * clampedIntensity));
}

const DEMO_VOICE_SYLLABLES: readonly [number, number, number][] = [
  [0.11, 0.052, 0.58],
  [0.22, 0.055, 0.86],
  [0.34, 0.052, 0.48],
  [0.65, 0.055, 0.94],
  [0.78, 0.055, 0.66],
];

/** Deterministic multi-syllable speech curve (0–1) for the Settings preview toggle. */
export function sampleDemoVoiceLevel(elapsedMs: number): number {
  const cycleMs = 3600;
  const phase = (((elapsedMs % cycleMs) + cycleMs) % cycleMs) / cycleMs;
  let peak = 0;
  for (const [center, width, amplitude] of DEMO_VOICE_SYLLABLES) {
    const distance = (phase - center) / width;
    const envelope = amplitude * Math.exp(-(distance * distance));
    if (envelope > peak) peak = envelope;
  }
  return peak < 0.015 ? 0 : Math.min(1, peak);
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
