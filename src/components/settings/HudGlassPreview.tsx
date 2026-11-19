import { useEffect, useRef, useState } from "react";
import {
  adapt,
  hexRgb,
  luma,
  sampler,
  tintFloor,
  type GlassInk,
} from "./hud-glasswork/GlassworkAdapt";
import {
  isLensSupported,
  liquidGlass,
  type LiquidGlassHandle,
} from "./hud-glasswork/GlassworkLens";
import {
  createGlassworkRefraction,
  drawGlassworkScene,
  type GlassworkRefraction,
} from "./hud-glasswork/GlassworkRenderer";

type HudGlassPreviewBackdrop = "light" | "dark";
type GlassMode = "off" | "webgl" | "css";

const REDUCED_TRANSPARENCY_QUERY = "(prefers-reduced-transparency: reduce)";
const BACKDROP_TRANSITION_MS = 620;
const SAMPLE_INTERVAL_MS = 120;
const LENS_CONFIG = {
  rim: 40,
  edge: 1.1,
  power: 2.6,
  dispersion: 0.15,
} as const;

function setGlassVar(el: HTMLElement, name: string, value: number, force: boolean) {
  const previous = Number.parseFloat(el.style.getPropertyValue(name));
  if (Number.isFinite(previous) && Math.abs(value - previous) < (force ? 0.0005 : 0.04)) return;
  el.style.setProperty(name, value.toFixed(3));
}

function easeProgress(t: number): number {
  const clamped = Math.min(1, Math.max(0, t));
  return clamped * clamped * (3 - 2 * clamped);
}

/** Local, deterministic Glasswork material and solid-backdrop sample for the HUD preview. */
export function HudGlassPreview({
  backdrop,
  refract,
  reducedMotion = false,
}: {
  backdrop: HudGlassPreviewBackdrop;
  refract: boolean;
  reducedMotion?: boolean;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const sourceRef = useRef<HTMLCanvasElement>(null);
  const refractionRef = useRef<HTMLCanvasElement>(null);
  const materialRef = useRef<HTMLDivElement>(null);
  const darkProgressRef = useRef(backdrop === "dark" ? 1 : 0);
  const keepRef = useRef(1);
  const lastSampleAtRef = useRef(0);

  const [mode, setMode] = useState<GlassMode>(refract ? "css" : "off");
  const [reducedTransparency, setReducedTransparency] = useState(() =>
    typeof window !== "undefined" && (window.matchMedia?.(REDUCED_TRANSPARENCY_QUERY).matches ?? false),
  );

  useEffect(() => {
    const root = rootRef.current;
    const sourceCanvas = sourceRef.current;
    const refractionCanvas = refractionRef.current;
    const material = materialRef.current;
    if (!root || !sourceCanvas || !refractionCanvas) return;

    let active = true;
    let renderer: GlassworkRefraction | null = null;
    let lensHandle: LiquidGlassHandle | null = null;
    let fadeFrame = 0;
    const wallpaperSampler = sampler();
    const lensSupported = isLensSupported();
    const preference = window.matchMedia?.(REDUCED_TRANSPARENCY_QUERY);
    let reduced = preference?.matches ?? false;
    setReducedTransparency(reduced);

    const setGlassMode = (next: GlassMode) => {
      if (active) setMode((current) => (current === next ? current : next));
    };

    const sampleBackdrop = (force: boolean) => {
      if (!refract || reduced || !material) {
        keepRef.current = 1;
        return;
      }
      const now = performance.now();
      if (!force && now - lastSampleAtRef.current < SAMPLE_INTERVAL_MS) return;
      lastSampleAtRef.current = now;

      const sourceRect = sourceCanvas.getBoundingClientRect();
      const materialRect = material.getBoundingClientRect();
      const stageWidth = sourceRect.width || root.getBoundingClientRect().width;
      const stageHeight = sourceRect.height || root.getBoundingClientRect().height;
      const captured = stageWidth > 0 && stageHeight > 0
        ? wallpaperSampler.capture(sourceCanvas, stageWidth, stageHeight)
        : false;
      const patch = captured && materialRect.width > 0 && materialRect.height > 0
        ? wallpaperSampler.sample({
            x: materialRect.left - sourceRect.left,
            y: materialRect.top - sourceRect.top,
            w: materialRect.width,
            h: materialRect.height,
          })
        : null;

      const ink: GlassInk = backdrop === "dark" ? "light" : "dark";
      const tintHex = backdrop === "dark" ? "#1e1e22" : "#ffffff";
      const adaptive = patch ? adapt(patch, ink) : { density: 0.35, saturate: 1 };
      const floor = patch
        ? tintFloor(patch.luminance, patch.texture, ink, luma(hexRgb(tintHex)))
        : 0;
      keepRef.current = adaptive.saturate;
      setGlassVar(material, "--density", adaptive.density, force);
      setGlassVar(material, "--keep", adaptive.saturate, force);
      setGlassVar(material, "--floor", floor, force);
    };

    const drawGlassFrame = (forceSample: boolean) => {
      const rootRect = root.getBoundingClientRect();
      const scale = Math.min(Math.max(window.devicePixelRatio || 1, 1), 2);
      if (rootRect.width > 0 && rootRect.height > 0) {
        drawGlassworkScene(
          sourceCanvas,
          rootRect.width,
          rootRect.height,
          scale,
          backdrop,
          darkProgressRef.current,
        );
      }

      if (!refract) {
        lensHandle?.destroy();
        lensHandle = null;
        renderer?.dispose();
        renderer = null;
        setGlassMode("off");
        return;
      }

      if (!material) {
        setGlassMode("css");
        return;
      }

      if (reduced) {
        lensHandle?.destroy();
        lensHandle = null;
        renderer?.dispose();
        renderer = null;
        setGlassMode("css");
        return;
      }

      sampleBackdrop(forceSample);

      if (lensSupported) {
        renderer?.dispose();
        renderer = null;
        if (!lensHandle) {
          lensHandle = liquidGlass(material, LENS_CONFIG);
        }
        setGlassMode("css");
        return;
      }

      if (rootRect.width <= 0 || rootRect.height <= 0) {
        setGlassMode("css");
        return;
      }

      if (!renderer) renderer = createGlassworkRefraction(refractionCanvas);
      if (!renderer) {
        setGlassMode("css");
        return;
      }

      const sourceRect = sourceCanvas.getBoundingClientRect();
      const materialRect = material.getBoundingClientRect();
      const refractionRect = refractionCanvas.getBoundingClientRect();
      const width = refractionRect.width;
      const height = refractionRect.height;
      const pixelWidth = Math.max(1, Math.round(width * scale));
      const pixelHeight = Math.max(1, Math.round(height * scale));
      if (refractionCanvas.width !== pixelWidth || refractionCanvas.height !== pixelHeight) {
        refractionCanvas.width = pixelWidth;
        refractionCanvas.height = pixelHeight;
      }

      const rendered = renderer.render({
        source: sourceCanvas,
        scale,
        offsetX: refractionRect.left - sourceRect.left,
        offsetY: refractionRect.top - sourceRect.top,
        sceneWidth: sourceRect.width,
        sceneHeight: sourceRect.height,
        width,
        height,
        kind: backdrop,
        keep: keepRef.current,
        ...LENS_CONFIG,
      });
      setGlassMode(materialRect.width > 0 && materialRect.height > 0 && rendered ? "webgl" : "css");
    };

    const targetDark = backdrop === "dark" ? 1 : 0;
    const startDark = darkProgressRef.current;
    if (reducedMotion || !refract || startDark === targetDark) {
      darkProgressRef.current = targetDark;
      drawGlassFrame(true);
    } else {
      const startTime = performance.now();
      const stepFade = (now: number) => {
        fadeFrame = 0;
        if (!active) return;
        const progress = Math.min(1, (now - startTime) / BACKDROP_TRANSITION_MS);
        darkProgressRef.current = startDark + (targetDark - startDark) * easeProgress(progress);
        drawGlassFrame(progress >= 1);
        if (progress < 1 && !document.hidden) {
          fadeFrame = requestAnimationFrame(stepFade);
        } else {
          darkProgressRef.current = targetDark;
          drawGlassFrame(true);
        }
      };
      drawGlassFrame(true);
      fadeFrame = requestAnimationFrame(stepFade);
    }

    const handleResize = () => drawGlassFrame(true);
    const handleContextLost = (event: Event) => {
      event.preventDefault();
      renderer?.dispose();
      renderer = null;
      setGlassMode(refract ? "css" : "off");
    };
    const handleContextRestored = () => drawGlassFrame(true);
    const handlePreferenceChange = (event: MediaQueryListEvent) => {
      reduced = event.matches;
      setReducedTransparency(reduced);
      drawGlassFrame(true);
    };

    refractionCanvas.addEventListener("webglcontextlost", handleContextLost);
    refractionCanvas.addEventListener("webglcontextrestored", handleContextRestored);
    preference?.addEventListener("change", handlePreferenceChange);

    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(handleResize);
    resizeObserver?.observe(root);
    window.addEventListener("resize", handleResize);

    return () => {
      active = false;
      cancelAnimationFrame(fadeFrame);
      resizeObserver?.disconnect();
      window.removeEventListener("resize", handleResize);
      preference?.removeEventListener("change", handlePreferenceChange);
      refractionCanvas.removeEventListener("webglcontextlost", handleContextLost);
      refractionCanvas.removeEventListener("webglcontextrestored", handleContextRestored);
      lensHandle?.destroy();
      lensHandle = null;
      renderer?.dispose();
      renderer = null;
      wallpaperSampler.clear();
    };
  }, [backdrop, refract, reducedMotion]);

  const glassMode = refract ? mode : "off";

  return (
    <div
      ref={rootRef}
      className="vf-hud-glasswork-preview"
      data-backdrop={backdrop}
      data-glass-mode={glassMode}
      data-glass-reduced-transparency={reducedTransparency ? "true" : undefined}
      aria-hidden="true"
    >
      <canvas
        ref={sourceRef}
        className="vf-hud-preview-source"
        data-glasswork-source=""
        width={1}
        height={1}
      />
      <canvas
        ref={refractionRef}
        className="vf-hud-preview-refract"
        data-glasswork-refract=""
        width={1}
        height={1}
      />
      {refract && (
        <div
          ref={materialRef}
          className="vf-hud-glasswork-material vf-glasswork-surface"
          data-glasswork-material=""
          data-kind={backdrop}
          data-refract={glassMode === "webgl" ? "webgl" : undefined}
          data-reduced-transparency={reducedTransparency ? "true" : undefined}
        />
      )}
    </div>
  );
}
