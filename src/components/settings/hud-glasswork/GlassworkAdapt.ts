/*
 * @license MIT, Copyright (c) 2026 Kubai087
 * Adapted from Glasswork/adapt.js at 4f0b6744b898b3a9b98fbd913ab24fdb83c53162.
 * Full MIT text and source pin: THIRD_PARTY_NOTICES.md (Glasswork).
 */

export type GlassInk = "light" | "dark";
export type RgbTriplet = [number, number, number];

export type WallpaperPatch = {
  luminance: number;
  texture: number;
  color: RgbTriplet;
};

export type WallpaperSampler = {
  capture(
    source: HTMLCanvasElement | HTMLImageElement | HTMLVideoElement,
    stageWidth: number,
    stageHeight: number,
  ): boolean;
  clear(): void;
  readonly ready: boolean;
  sample(box: { x: number; y: number; w: number; h: number }): WallpaperPatch | null;
};

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const smoothstep = (v: number) => {
  const a = clamp(v, 0, 1);
  return a * a * (3 - 2 * a);
};

export const luma = ([r, g, b]: RgbTriplet): number => (r * 0.299 + g * 0.587 + b * 0.114) / 255;
const chroma = ([r, g, b]: RgbTriplet): number => (Math.max(r, g, b) - Math.min(r, g, b)) / 255;

export function hexRgb(hex: string): RgbTriplet {
  const value = hex.replace("#", "");
  if (value.length === 3) {
    return [
      parseInt(value[0] + value[0], 16),
      parseInt(value[1] + value[1], 16),
      parseInt(value[2] + value[2], 16),
    ];
  }
  const normalized = value.padEnd(6, "0");
  return [
    parseInt(normalized.slice(0, 2), 16),
    parseInt(normalized.slice(2, 4), 16),
    parseInt(normalized.slice(4, 6), 16),
  ];
}

const SAMPLE_SCALE = 0.125;
const XS = [0.18, 0.34, 0.5, 0.66, 0.82];
const YS = [0.24, 0.5, 0.76];

export function sampler(): WallpaperSampler {
  let canvas: HTMLCanvasElement;
  let context: CanvasRenderingContext2D | null;
  const fresh = () => {
    canvas = document.createElement("canvas");
    context = canvas.getContext("2d", { willReadFrequently: true });
  };
  fresh();
  let pixels: Uint8ClampedArray | null = null;
  let width = 1;
  let height = 1;

  const at = (x: number, y: number): RgbTriplet => {
    const data = pixels!;
    const i = (
      clamp(Math.round(y * SAMPLE_SCALE), 0, height - 1) * width
      + clamp(Math.round(x * SAMPLE_SCALE), 0, width - 1)
    ) * 4;
    return [data[i], data[i + 1], data[i + 2]];
  };

  return {
    capture(source, stageWidth, stageHeight) {
      if (!context || stageWidth <= 0 || stageHeight <= 0) return false;
      const video = source as HTMLVideoElement;
      const img = source as HTMLImageElement;
      const sw = video.videoWidth || img.naturalWidth || source.width;
      const sh = video.videoHeight || img.naturalHeight || source.height;
      if (!sw || !sh || (video.readyState !== undefined && video.readyState < 2)) return false;
      width = Math.max(1, Math.round(stageWidth * SAMPLE_SCALE));
      height = Math.max(1, Math.round(stageHeight * SAMPLE_SCALE));
      if (canvas.width !== width) canvas.width = width;
      if (canvas.height !== height) canvas.height = height;
      const scale = Math.max(width / sw, height / sh);
      const dw = sw * scale;
      const dh = sh * scale;
      try {
        context.drawImage(source, (width - dw) / 2, (height - dh) / 2, dw, dh);
        pixels = context.getImageData(0, 0, width, height).data;
      } catch {
        pixels = null;
        fresh();
        return false;
      }
      return true;
    },
    clear() {
      pixels = null;
    },
    get ready() {
      return pixels !== null;
    },
    sample(box) {
      if (!pixels || box.w <= 0 || box.h <= 0) return null;
      const grid = YS.flatMap((fy) => XS.map((fx) => {
        const color = at(box.x + box.w * fx, box.y + box.h * fy);
        return { color, luminance: luma(color) };
      }));
      const values = [...grid].sort((a, b) => a.luminance - b.luminance);
      const texture = values[values.length - 1].luminance - values[0].luminance;
      const kept = values.slice(2, -2);
      const mean = (pick: (entry: { color: RgbTriplet; luminance: number }) => number) =>
        kept.reduce((sum, entry) => sum + pick(entry), 0) / kept.length;
      return {
        luminance: mean((entry) => entry.luminance),
        texture,
        color: [
          mean((entry) => entry.color[0]),
          mean((entry) => entry.color[1]),
          mean((entry) => entry.color[2]),
        ],
      };
    },
  };
}

export function pickInk(
  luminance: number,
  previous?: GlassInk,
  threshold = 0.5,
  band = 0.05,
): GlassInk {
  const shift = previous === "light" ? band : previous === "dark" ? -band : 0;
  return luminance < threshold + shift ? "light" : "dark";
}

export function adapt({ luminance, texture, color }: WallpaperPatch, ink: GlassInk) {
  const near = ink === "dark"
    ? smoothstep((0.8 - luminance) / 0.35)
    : smoothstep((luminance - 0.12) / 0.4);
  const c = chroma(color);
  return {
    density: clamp(near * 0.55 + texture * 0.7 + c * 0.3, 0, 1),
    saturate: clamp(1 - c * 0.75, 0.45, 1),
  };
}

export const TEXT_CONTRAST = 4.5;
const INK_RGB: Record<GlassInk, RgbTriplet> = {
  light: [245, 245, 247],
  dark: [29, 29, 31],
};
const MAX_FLOOR = 0.9;

function channel(value: number) {
  const c = value / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

const relativeLuminance = ([r, g, b]: RgbTriplet) =>
  0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);

export function contrast(a: RgbTriplet, b: RgbTriplet): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const gray = (level: number): RgbTriplet => {
  const v = Math.round(clamp(level, 0, 1) * 255);
  return [v, v, v];
};

export function tintFloor(
  luminance: number,
  texture: number,
  ink: GlassInk,
  tintLevel: number,
): number {
  const spread = clamp(Number(texture) || 0, 0, 1) / 2;
  const mean = clamp(Number(luminance) || 0, 0, 1);
  const backdrop = ink === "light" ? Math.min(1, mean + spread) : Math.max(0, mean - spread);
  const reads = (alpha: number) =>
    contrast(INK_RGB[ink], gray(backdrop + (tintLevel - backdrop) * alpha)) >= TEXT_CONTRAST;
  if (reads(0)) return 0;
  if (!reads(MAX_FLOOR)) return MAX_FLOOR;
  let low = 0;
  let high = MAX_FLOOR;
  for (let step = 0; step < 16; step += 1) {
    const middle = (low + high) / 2;
    if (reads(middle)) high = middle;
    else low = middle;
  }
  return high;
}
