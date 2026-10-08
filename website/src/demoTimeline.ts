import type { Phase } from "./content";

export const demoTiming = {
  listening: 400,
  processing: 4600,
  writing: 5600,
  complete: 8000,
} as const;

export function demoFrame(elapsed: number) {
  const progress = (start: number, end: number) =>
    Math.max(0, Math.min(1, (elapsed - start) / (end - start)));
  const phase: Phase =
    elapsed < demoTiming.listening
      ? "idle"
      : elapsed < demoTiming.processing
        ? "listening"
        : elapsed < demoTiming.writing
          ? "processing"
          : elapsed < demoTiming.complete
            ? "writing"
            : "complete";

  return {
    phase,
    spokenProgress: progress(demoTiming.listening, demoTiming.processing),
    resultProgress: progress(demoTiming.writing, demoTiming.complete),
  };
}
