import { useEffect, useRef } from "react";
import "./ParticleText.css";

type Particle = {
  alpha: number;
  depth: number;
  delay: number;
  opacity: number;
  seed: number;
  size: number;
  startX: number;
  startY: number;
  targetX: number;
  targetY: number;
  x: number;
  y: number;
};

type ParticleTextProps = {
  text: string;
  className?: string;
  delay?: number;
  density?: number;
  gatherDuration?: number;
  particleScale?: number;
  scatter?: number;
  stagger?: number;
};

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);
const easeOutCubic = (value: number) => 1 - Math.pow(1 - value, 3);

/** A small, reduced-motion-aware canvas text reveal inspired by React Bits' Particle Text. */
export function ParticleText({
  text,
  className = "",
  delay = 0,
  density = 1,
  gatherDuration = 440,
  particleScale = 1,
  scatter = 96,
  stagger = 60,
}: ParticleTextProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return undefined;

    const context = canvas.getContext("2d");
    if (!context) return undefined;

    let width = 0;
    let height = 0;
    let particles: Particle[] = [];
    let animationFrame: number | null = null;
    let resizeFrame: number | null = null;
    let gatherTimer: number | null = null;
    let gatherStart = 0;
    let building = 0;
    let gathering = false;
    let reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

    const stopAnimation = () => {
      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = null;
      }
    };

    const drawParticle = (particle: Particle) => {
      context.beginPath();
      context.arc(particle.x, particle.y, particle.size / 2, 0, Math.PI * 2);
      context.fill();
    };

    const render = (now: number) => {
      context.clearRect(0, 0, width, height);
      let complete = true;

      particles.forEach((particle) => {
        let progress = 1;
        let x = particle.targetX;
        let y = particle.targetY;

        if (gathering) {
          progress = clamp((now - gatherStart - particle.delay) / gatherDuration, 0, 1);
          const eased = easeOutCubic(progress);
          x = particle.startX + (particle.targetX - particle.startX) * eased;
          y = particle.startY + (particle.targetY - particle.startY) * eased;
          if (progress < 1) complete = false;
        }

        particle.x = x;
        particle.y = y;
        context.globalAlpha = particle.alpha * particle.opacity * (0.34 + progress * 0.66);
        drawParticle(particle);
      });

      context.globalAlpha = 1;
      if (gathering && !complete) {
        animationFrame = window.requestAnimationFrame(render);
        return;
      }

      gathering = false;
      animationFrame = null;
    };

    const scheduleGather = () => {
      if (!particles.length) return;

      stopAnimation();
      if (gatherTimer !== null) window.clearTimeout(gatherTimer);

      if (reducedMotion) {
        particles.forEach((particle) => {
          particle.x = particle.targetX;
          particle.y = particle.targetY;
          particle.startX = particle.targetX;
          particle.startY = particle.targetY;
          particle.delay = 0;
        });
        gathering = false;
        render(performance.now());
        return;
      }

      particles.forEach((particle) => {
        const angle = particle.seed * Math.PI * 2;
        const distance = scatter * (0.45 + particle.depth * 0.7);
        particle.startX = particle.targetX + Math.cos(angle) * distance;
        particle.startY = particle.targetY + Math.sin(angle) * distance;
        particle.x = particle.startX;
        particle.y = particle.startY;
        particle.delay = particle.seed * stagger;
      });

      gatherTimer = window.setTimeout(() => {
        gatherStart = performance.now();
        gathering = true;
        animationFrame = window.requestAnimationFrame(render);
      }, Math.max(0, delay));
    };

    const sampleText = async () => {
      const currentBuild = ++building;
      const rect = container.getBoundingClientRect();
      width = Math.floor(rect.width);
      height = Math.floor(rect.height);
      if (width <= 0 || height <= 0) return;

      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.max(1, Math.floor(width * dpr));
      canvas.height = Math.max(1, Math.floor(height * dpr));
      canvas.style.width = "100%";
      canvas.style.height = "100%";
      context.setTransform(dpr, 0, 0, dpr, 0, 0);

      if ("fonts" in document) await document.fonts.ready;
      if (currentBuild !== building) return;

      const computed = window.getComputedStyle(container);
      const fontSize = parseFloat(computed.fontSize) || 16;
      const fontWeight = computed.fontWeight || "400";
      const fontFamily = computed.fontFamily || "sans-serif";
      const font = `${fontWeight} ${fontSize}px ${fontFamily}`;
      const offscreen = document.createElement("canvas");
      const offscreenContext = offscreen.getContext("2d", { willReadFrequently: true });
      if (!offscreenContext) return;

      offscreenContext.font = font;
      const content = text || " ";
      let metrics = offscreenContext.measureText(content);
      let resolvedFontSize = fontSize;
      const maxTextWidth = width * 0.96;
      if (metrics.width > maxTextWidth) {
        resolvedFontSize = Math.max(12, fontSize * (maxTextWidth / metrics.width));
        offscreenContext.font = `${fontWeight} ${resolvedFontSize}px ${fontFamily}`;
        metrics = offscreenContext.measureText(content);
      }

      const left = Math.ceil(metrics.actualBoundingBoxLeft || 0);
      const right = Math.ceil(metrics.actualBoundingBoxRight || metrics.width);
      const ascent = Math.ceil(metrics.actualBoundingBoxAscent || resolvedFontSize * 0.78);
      const descent = Math.ceil(metrics.actualBoundingBoxDescent || resolvedFontSize * 0.22);
      const padding = Math.max(4, Math.ceil(resolvedFontSize * 0.08));
      offscreen.width = Math.max(1, left + right + padding * 2);
      offscreen.height = Math.max(1, ascent + descent + padding * 2);
      offscreenContext.clearRect(0, 0, offscreen.width, offscreen.height);
      offscreenContext.font = `${fontWeight} ${resolvedFontSize}px ${fontFamily}`;
      offscreenContext.textBaseline = "alphabetic";
      offscreenContext.fillStyle = "#ffffff";
      offscreenContext.fillText(content, padding - left, padding + ascent);

      const image = offscreenContext.getImageData(0, 0, offscreen.width, offscreen.height);
      const step = Math.max(1, Math.floor(density));
      const sampled: Array<{ x: number; y: number; alpha: number }> = [];
      for (let y = 0; y < offscreen.height; y += step) {
        for (let x = 0; x < offscreen.width; x += step) {
          const alpha = image.data[(y * offscreen.width + x) * 4 + 3];
          if (alpha > 20) {
            sampled.push({
              x: width / 2 - offscreen.width / 2 + x + 0.5,
              y: height / 2 - offscreen.height / 2 + y + 0.5,
              alpha: alpha / 255,
            });
          }
        }
      }

      const maxParticles = Math.min(14000, sampled.length);
      const stride = Math.max(1, Math.ceil(sampled.length / Math.max(1, maxParticles)));
      const color = computed.color || "currentColor";
      context.fillStyle = color;
      particles = sampled.filter((_, index) => index % stride === 0).map((target, index) => {
        const seed = ((index * 9301 + 49297) % 233280) / 233280;
        return {
          alpha: target.alpha,
          depth: 0.55 + (((index * 233 + 97) % 1000) / 1000) * 0.7,
          delay: 0,
          opacity: 0.76 + (((index * 479 + 31) % 1000) / 1000) * 0.24,
          seed,
          size: Math.max(0.8, (1.35 + target.alpha * 0.55) * particleScale),
          startX: target.x,
          startY: target.y,
          targetX: target.x,
          targetY: target.y,
          x: target.x,
          y: target.y,
        };
      });

      scheduleGather();
    };

    const queueSample = () => {
      if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
      resizeFrame = window.requestAnimationFrame(() => void sampleText());
    };

    const reduceMotionQuery = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    const onReducedMotionChange = (event: MediaQueryListEvent) => {
      reducedMotion = event.matches;
      queueSample();
    };
    reduceMotionQuery?.addEventListener("change", onReducedMotionChange);

    const themeObserver = typeof MutationObserver === "undefined" ? null : new MutationObserver(queueSample);
    themeObserver?.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    const colorSchemeQuery = window.matchMedia?.("(prefers-color-scheme: dark)");
    const onColorSchemeChange = () => queueSample();
    colorSchemeQuery?.addEventListener("change", onColorSchemeChange);

    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(queueSample);
    resizeObserver?.observe(container);
    void sampleText();

    return () => {
      building += 1;
      resizeObserver?.disconnect();
      themeObserver?.disconnect();
      reduceMotionQuery?.removeEventListener("change", onReducedMotionChange);
      colorSchemeQuery?.removeEventListener("change", onColorSchemeChange);
      stopAnimation();
      if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
      if (gatherTimer !== null) window.clearTimeout(gatherTimer);
    };
  }, [delay, density, gatherDuration, particleScale, scatter, stagger, text]);

  return (
    <div ref={containerRef} className={`vf-particle-text ${className}`.trim()} role="img" aria-label={text}>
      <canvas ref={canvasRef} className="vf-particle-text__canvas" aria-hidden="true" />
      <span className="vf-particle-text__sr">{text}</span>
    </div>
  );
}
