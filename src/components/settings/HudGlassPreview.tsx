import { useEffect, useRef, useState } from "react";
import {
  createGlassworkRefraction,
  drawGlassworkScene,
  type GlassworkRefraction,
} from "./hud-glasswork/GlassworkRenderer";

type HudGlassPreviewBackdrop = "light" | "dark";
type GlassMode = "off" | "webgl" | "css";

const reducedTransparencyQuery = "(prefers-reduced-transparency: reduce)";

/** Local, deterministic backdrop and material sample for the HUD settings preview. */
export function HudGlassPreview({
  backdrop,
  refract,
}: {
  backdrop: HudGlassPreviewBackdrop;
  refract: boolean;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const sourceRef = useRef<HTMLCanvasElement>(null);
  const refractionRef = useRef<HTMLCanvasElement>(null);
  const materialRef = useRef<HTMLDivElement>(null);
  const [mode, setMode] = useState<GlassMode>(refract ? "css" : "off");
  const [reducedTransparency, setReducedTransparency] = useState(() =>
    typeof window !== "undefined" && (window.matchMedia?.(reducedTransparencyQuery).matches ?? false),
  );

  useEffect(() => {
    const root = rootRef.current;
    const sourceCanvas = sourceRef.current;
    const refractionCanvas = refractionRef.current;
    const material = materialRef.current;
    if (!root || !sourceCanvas || !refractionCanvas) return;

    let active = true;
    let renderer: GlassworkRefraction | null = null;
    let reduced = false;
    const preference = window.matchMedia?.(reducedTransparencyQuery);
    reduced = preference?.matches ?? false;
    setReducedTransparency(reduced);

    const setGlassMode = (next: GlassMode) => {
      if (active) setMode((current) => current === next ? current : next);
    };

    const draw = () => {
      const rootRect = root.getBoundingClientRect();
      if (rootRect.width <= 0 || rootRect.height <= 0) return;

      const scale = Math.min(Math.max(window.devicePixelRatio || 1, 1), 2);
      drawGlassworkScene(sourceCanvas, rootRect.width, rootRect.height, scale, backdrop);
      if (!refract) {
        setGlassMode("off");
        return;
      }
      if (!material) {
        setGlassMode("css");
        return;
      }

      if (reduced) {
        renderer?.dispose();
        renderer = null;
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
      });
      setGlassMode(materialRect.width > 0 && materialRect.height > 0 && rendered ? "webgl" : "css");
    };

    const handleContextLost = (event: Event) => {
      event.preventDefault();
      renderer?.dispose();
      renderer = null;
      setGlassMode(refract ? "css" : "off");
    };
    const handleContextRestored = () => draw();
    const handlePreferenceChange = (event: MediaQueryListEvent) => {
      reduced = event.matches;
      setReducedTransparency(reduced);
      draw();
    };

    refractionCanvas.addEventListener("webglcontextlost", handleContextLost);
    refractionCanvas.addEventListener("webglcontextrestored", handleContextRestored);
    preference?.addEventListener("change", handlePreferenceChange);

    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(draw);
    resizeObserver?.observe(root);
    window.addEventListener("resize", draw);
    draw();

    return () => {
      active = false;
      resizeObserver?.disconnect();
      window.removeEventListener("resize", draw);
      preference?.removeEventListener("change", handlePreferenceChange);
      refractionCanvas.removeEventListener("webglcontextlost", handleContextLost);
      refractionCanvas.removeEventListener("webglcontextrestored", handleContextRestored);
      renderer?.dispose();
      renderer = null;
    };
  }, [backdrop, refract]);

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
          className="vf-hud-glasswork-material"
          data-glasswork-material=""
        />
      )}
    </div>
  );
}
