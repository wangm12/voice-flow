import { memo, type CSSProperties } from "react";

const BAR_ENVELOPE = [0.35, 0.62, 0.48, 1, 0.65, 0.85, 0.58, 0.35];
const MIN_BAR_SCALE = 0.18;
const LEVEL_GAIN = 1.35;
export const WAVEFORM_BAR_COUNT = BAR_ENVELOPE.length;

export function waveformScaleForLevel(level: number, envelope: number): number {
  // RMS values from a microphone are usually much lower than 1. Apply a
  // perceptual curve and a small gain so normal speech reaches the useful
  // visual range without making the bars jump at the noise floor.
  const clamped = Math.max(0, Math.min(1, level));
  const normalized = Math.min(1, Math.pow(clamped, 0.58) * LEVEL_GAIN);
  return MIN_BAR_SCALE + normalized * (0.22 + envelope * 0.78);
}

export const VoiceWaveform = memo(function VoiceWaveform({
  dim = false,
  active = false,
  level = 0,
  levels,
  preview = false,
}: {
  dim?: boolean;
  active?: boolean;
  level?: number;
  levels?: number[];
  preview?: boolean;
}) {
  const bars = Array.from({ length: WAVEFORM_BAR_COUNT }, (_, index) => {
    // Each bar represents a different moment from the rolling amplitude
    // history. A single live scalar makes the entire waveform breathe as one
    // shape, which reads as movement of the group rather than independent bars.
    const sample = levels?.[index] ?? level;
    const scale = dim ? 0.32 : waveformScaleForLevel(sample, BAR_ENVELOPE[index]);
    return {
      className: `voice-pill__wave-bar voice-pill__wave-bar--${index + 1}`,
      style: { transform: `scaleY(${scale})` } as CSSProperties,
    };
  });

  return (
    <span className={`voice-pill__wave${dim ? " voice-pill__wave--dim" : ""}${active ? " voice-pill__wave--active" : ""}${preview ? " voice-pill__wave--preview" : ""}`} aria-hidden="true">
      {bars.map((bar) => <span key={bar.className} className={bar.className} style={bar.style} />)}
    </span>
  );
});
