import { useLayoutEffect, useRef, type RefObject } from "react";

/** Matches border-beam `size="line"` travel keyframes in generateLineVariantCSS. */
const LINE_TRAVEL = [
  { t: 0, x: 0.06, w: 0.5 },
  { t: 0.1, x: 0.15, w: 0.8 },
  { t: 0.2, x: 0.25, w: 1.1 },
  { t: 0.3, x: 0.35, w: 1.3 },
  { t: 0.4, x: 0.44, w: 1.45 },
  { t: 0.5, x: 0.5, w: 1.5 },
  { t: 0.6, x: 0.56, w: 1.45 },
  { t: 0.7, x: 0.65, w: 1.3 },
  { t: 0.8, x: 0.75, w: 1.1 },
  { t: 0.9, x: 0.85, w: 0.8 },
  { t: 1, x: 0.94, w: 0.5 },
] as const;

/** Line preset duration in border-beam 1.3. */
const LINE_DURATION_SEC = 3.1;

export const THINKING_HOLD = 0.9;
export const THINKING_CREEP_PER_SEC = 0.16;
export const THINKING_FOLLOW_TAU_SEC = 0.55;
export const THINKING_FINISH_TAU_SEC = 0.28;

export type LineBeamMode = "off" | "scrub";

export function lineBeamFrame(progress: number): { x: number; w: number } {
  const t = Math.max(0, Math.min(1, progress));
  let index = 0;
  while (index < LINE_TRAVEL.length - 1 && LINE_TRAVEL[index + 1].t <= t) {
    index += 1;
  }
  const from = LINE_TRAVEL[index];
  const to = LINE_TRAVEL[Math.min(index + 1, LINE_TRAVEL.length - 1)];
  if (from.t === to.t) return { x: from.x, w: from.w };
  const mix = (t - from.t) / (to.t - from.t);
  return {
    x: from.x + (to.x - from.x) * mix,
    w: from.w + (to.w - from.w) * mix,
  };
}

export function stepSmoothThinkingProgress(visual: number, target: number, dtSec: number): number {
  const dt = Math.max(0, Math.min(0.05, dtSec));
  const goal = Math.max(0, Math.min(1, target));
  const tau = goal >= 0.999 ? THINKING_FINISH_TAU_SEC : THINKING_FOLLOW_TAU_SEC;
  const follow = 1 - Math.exp(-dt / tau);
  const toward = visual + (goal - visual) * follow;
  if (goal >= 0.999) return Math.min(1, Math.max(visual, toward));
  return Math.min(THINKING_HOLD, Math.max(visual, toward + THINKING_CREEP_PER_SEC * dt));
}

export function driveLineBeamWithJs(el: HTMLElement): void {
  const id = el.getAttribute("data-beam");
  if (!id || el.dataset.hudLineDrive === "1") return;
  el.dataset.hudLineDrive = "1";
  const breathe = (LINE_DURATION_SEC * 1.3).toFixed(1);
  const spike = (LINE_DURATION_SEC * 1.33).toFixed(1);
  const spike2 = (LINE_DURATION_SEC * 1.7).toFixed(1);
  el.style.setProperty(
    "animation",
    [
      `beam-breathe-${id} ${breathe}s ease-in-out infinite`,
      `beam-spike-${id} ${spike}s ease-in-out infinite`,
      `beam-spike2-${id} ${spike2}s ease-in-out infinite`,
      `beam-fade-in-${id} 0.6s ease forwards`,
    ].join(", "),
    "important",
  );
}

export function releaseLineBeamDrive(el: HTMLElement): void {
  delete el.dataset.hudLineDrive;
  el.style.removeProperty("animation");
}

export function applyLineBeamProgress(el: HTMLElement, progress: number | null): void {
  const id = el.getAttribute("data-beam");
  if (!id) return;
  if (progress == null) {
    el.style.removeProperty(`--beam-x-${id}`);
    el.style.removeProperty(`--beam-w-${id}`);
    el.style.removeProperty(`--beam-edge-${id}`);
    return;
  }
  const frame = lineBeamFrame(progress);
  const nextX = frame.x.toFixed(4);
  const nextW = frame.w.toFixed(4);
  if (
    el.style.getPropertyValue(`--beam-x-${id}`) === nextX
    && el.style.getPropertyValue(`--beam-w-${id}`) === nextW
  ) {
    return;
  }
  el.style.setProperty(`--beam-x-${id}`, nextX, "important");
  el.style.setProperty(`--beam-w-${id}`, nextW, "important");
  el.style.setProperty(`--beam-edge-${id}`, "1", "important");
}

export function useSmoothLineBeam(
  elRef: RefObject<HTMLDivElement | null>,
  mode: LineBeamMode,
  target: number,
): void {
  const visualRef = useRef(0);
  const targetRef = useRef(target);
  targetRef.current = target;

  useLayoutEffect(() => {
    const el = elRef.current;
    if (!el || mode === "off") {
      visualRef.current = 0;
      if (el) {
        releaseLineBeamDrive(el);
        applyLineBeamProgress(el, null);
      }
      return undefined;
    }

    driveLineBeamWithJs(el);
    let frame = 0;
    let last = performance.now();
    const tick = (now: number) => {
      const rawDt = (now - last) / 1000;
      const dt = Math.min(0.05, rawDt > 0 ? rawDt : 1 / 60);
      last = now;
      visualRef.current = stepSmoothThinkingProgress(visualRef.current, targetRef.current, dt);
      applyLineBeamProgress(el, visualRef.current);
      frame = window.requestAnimationFrame(tick);
    };
    applyLineBeamProgress(el, visualRef.current);
    frame = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frame);
  }, [mode, elRef]);
}
