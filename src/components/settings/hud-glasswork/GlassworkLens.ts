/*
 * @license MIT, Copyright (c) 2026 Kubai087
 * Adapted from Glasswork/lens.js at 4f0b6744b898b3a9b98fbd913ab24fdb83c53162.
 * Full MIT text and source pin: THIRD_PARTY_NOTICES.md (Glasswork).
 */

export type LiquidGlassOptions = {
  rim?: number;
  edge?: number;
  power?: number;
  dispersion?: number;
  radius?: number | null;
  corner?: number | null;
  enabled?: boolean;
  property?: string;
};

export type LiquidGlassHandle = {
  supported: boolean;
  bezel: number;
  update(next?: LiquidGlassOptions): void;
  refresh(): void;
  destroy(): void;
};

export const LENS_DEFAULTS = {
  rim: 40,
  edge: 1.1,
  power: 2.6,
  dispersion: 0.15,
  radius: null as number | null,
  corner: null as number | null,
  enabled: true,
  property: "--glass-lens",
};

export const ROUND_CORNER = 2;

const LENS_MIN_PX = 32;
const SETTLE_MS = 140;
const SVG_NS = "http://www.w3.org/2000/svg";
const TILE_CACHE_MAX = 48;

export function isLensSupported(): boolean {
  if (typeof CSS === "undefined" || !CSS.supports?.("backdrop-filter", "url(#a)")) return false;
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  return /Chrome\//.test(ua) && !/Firefox\//.test(ua);
}

export function cornerExponent(shape: string | null | undefined): number {
  const value = String(shape || "").trim();
  if (value === "squircle") return 4;
  const match = /^superellipse\(\s*([^)\s]+)\s*\)$/.exec(value);
  const k = match ? Number(match[1]) : Number.NaN;
  return Number.isFinite(k) && k >= 0 ? 2 ** k : ROUND_CORNER;
}

function rimStrength(depth: number, power: number): number {
  return depth >= 1 ? 0 : (1 - Math.max(0, depth)) ** power;
}

function contourRadius(depth: number, radius: number, bezel: number): number {
  if (radius >= bezel) return radius - depth * bezel;
  return (1 - depth) * (radius + (bezel - radius) * depth * (2 - depth));
}

function isWithinContour(
  x: number,
  y: number,
  depth: number,
  radius: number,
  bezel: number,
  corner: number,
): boolean {
  const inset = depth * bezel;
  if (x < inset || y < inset) return false;
  const curve = contourRadius(depth, radius, bezel);
  const center = inset + curve;
  if (x >= center || y >= center) return true;
  if (curve <= 0) return false;
  return ((center - x) / curve) ** corner + ((center - y) / curve) ** corner <= 1;
}

function bendNearCorner(
  x: number,
  y: number,
  radius: number,
  bezel: number,
  corner: number,
  power: number,
): [number, number] {
  if (isWithinContour(x, y, 1, radius, bezel, corner)) return [0, 0];
  let depth = 0;
  if (isWithinContour(x, y, 0, radius, bezel, corner)) {
    let low = 0;
    let high = 1;
    for (let step = 0; step < 14; step += 1) {
      const middle = (low + high) / 2;
      if (isWithinContour(x, y, middle, radius, bezel, corner)) low = middle;
      else high = middle;
    }
    depth = low;
  }
  const strength = rimStrength(depth, power);
  const curve = contourRadius(depth, radius, bezel);
  const center = depth * bezel + curve;
  if (x < center && y < center && curve > 0) {
    const nx = Math.max(0, (center - x) / curve) ** (corner - 1);
    const ny = Math.max(0, (center - y) / curve) ** (corner - 1);
    const length = Math.hypot(nx, ny) || 1;
    return [(nx / length) * strength, (ny / length) * strength];
  }
  return x < y ? [strength, 0] : [0, strength];
}

export function rimDisplacement(
  x: number,
  y: number,
  width: number,
  height: number,
  {
    bezel,
    radius,
    corner = ROUND_CORNER,
    power = LENS_DEFAULTS.power,
  }: { bezel: number; radius: number; corner?: number; power?: number },
): [number, number] {
  const rim = Math.min(bezel, height / 2, width / 2);
  const r = Math.max(0, Math.min(radius, width / 2, height / 2));
  const [inwardX, inwardY] = bendNearCorner(Math.min(x, width - x), Math.min(y, height - y), r, rim, corner, power);
  if (inwardX === 0 && inwardY === 0) return [0, 0];
  return [x < width / 2 ? inwardX : -inwardX, y < height / 2 ? inwardY : -inwardY];
}

function lensLayout(width: number, height: number, size: number): Array<[number, number, number, number]> {
  const across = Math.max(1, width - size * 2);
  const down = Math.max(1, height - size * 2);
  return [
    [0, 0, size, size],
    [width - size, 0, size, size],
    [0, height - size, size, size],
    [width - size, height - size, size, size],
    [size, 0, across, size],
    [size, height - size, across, size],
    [0, size, size, down],
    [width - size, size, size, down],
  ];
}

type DecodedTiles = {
  size: number;
  urls: string[];
};

function tilePixels(radius: number, bezel: number, corner: number, power: number) {
  const size = Math.max(1, Math.ceil(Math.max(radius, bezel)));
  const probe = size * 2 + 1;
  const geometry = { bezel, radius, corner, power };
  return {
    size,
    tiles: lensLayout(probe, probe, size).map(([left, top, w, h]) => {
      const data = new Uint8ClampedArray(w * h * 4);
      for (let y = 0; y < h; y += 1) {
        for (let x = 0; x < w; x += 1) {
          const [dx, dy] = rimDisplacement(left + x + 0.5, top + y + 0.5, probe, probe, geometry);
          const i = (y * w + x) * 4;
          data[i] = Math.round(128 + dx * 127);
          data[i + 1] = Math.round(128 + dy * 127);
          data[i + 2] = 128;
          data[i + 3] = 255;
        }
      }
      return { width: w, height: h, data };
    }),
  };
}

const tileCache = new Map<string, Promise<DecodedTiles | null>>();

function lensTiles(radius: number, bezel: number, corner: number, power: number): Promise<DecodedTiles | null> {
  const key = `${radius}:${bezel}:${corner.toFixed(3)}:${power.toFixed(3)}`;
  let entry = tileCache.get(key);
  if (entry) {
    tileCache.delete(key);
    tileCache.set(key, entry);
    return entry;
  }
  const { size, tiles } = tilePixels(radius, bezel, corner, power);
  // Tauri CSP permits `data:` URLs for images and forbids `blob:` URLs, so encode via PNG data URLs.
  entry = Promise.all(tiles.map(async (tile) => {
    const canvas = document.createElement("canvas");
    canvas.width = tile.width;
    canvas.height = tile.height;
    const context = canvas.getContext("2d");
    if (!context) return null;
    const image = context.createImageData(tile.width, tile.height);
    image.data.set(tile.data);
    context.putImageData(image, 0, 0);
    const url = canvas.toDataURL("image/png");
    if (typeof Image !== "undefined") {
      const decoded = new Image();
      decoded.src = url;
      if (typeof decoded.decode === "function") {
        await decoded.decode().catch(() => {});
      }
    }
    return url;
  })).then((urls) => {
    if (urls.some((url) => url === null)) return null;
    return { size, urls: urls as string[] };
  });
  tileCache.set(key, entry);
  while (tileCache.size > TILE_CACHE_MAX) {
    const oldest = tileCache.keys().next().value;
    if (oldest !== undefined) tileCache.delete(oldest);
  }
  return entry;
}

function createSvgNode<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number>,
  parent?: Element | null,
): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) {
    el.setAttribute(name, String(value));
  }
  parent?.append(el);
  return el;
}

type BuiltFilter = {
  id: string;
  host: SVGSVGElement;
  filter: SVGFilterElement;
  pieces: SVGFEImageElement[];
  passes: Array<{ el: SVGFEDisplacementMapElement }>;
  tiles: DecodedTiles;
};

let filterCount = 0;

function buildFilter(width: number, height: number, tiles: DecodedTiles, split: boolean): BuiltFilter {
  const id = `liquid-glass-lens-${++filterCount}`;
  const host = createSvgNode("svg", {
    "aria-hidden": "true",
    width: 0,
    height: 0,
    style: "position:absolute;width:0;height:0;overflow:hidden;pointer-events:none",
  });
  const filter = createSvgNode("filter", {
    id,
    filterUnits: "userSpaceOnUse",
    "color-interpolation-filters": "sRGB",
    x: 0,
    y: 0,
    width,
    height,
  }, host);
  createSvgNode("feFlood", { "flood-color": "rgb(128,128,128)", result: "flat" }, filter);
  const rects = lensLayout(width, height, tiles.size);
  const pieces = rects.map(([x, y, w, h], index) => createSvgNode("feImage", {
    href: tiles.urls[index],
    preserveAspectRatio: "none",
    x,
    y,
    width: w,
    height: h,
    result: `t${index}`,
  }, filter));
  const merge = createSvgNode("feMerge", { result: "map" }, filter);
  createSvgNode("feMergeNode", { in: "flat" }, merge);
  rects.forEach((_, index) => createSvgNode("feMergeNode", { in: `t${index}` }, merge));
  const passes: Array<{ el: SVGFEDisplacementMapElement }> = [];
  const addPass = () => {
    passes.push({
      el: createSvgNode("feDisplacementMap", {
        in: "SourceGraphic",
        in2: "map",
        xChannelSelector: "R",
        yChannelSelector: "G",
        scale: 0,
      }, filter),
    });
  };
  if (split) {
    (["r", "g", "b"] as const).forEach((name, channel) => {
      addPass();
      const keep = Array.from({ length: 20 }, (_, cell) => Number(cell === channel * 6 || cell === 18)).join(" ");
      createSvgNode("feColorMatrix", { type: "matrix", values: keep, result: name }, filter);
    });
    createSvgNode("feBlend", { in: "r", in2: "g", mode: "screen" }, filter);
    createSvgNode("feBlend", { in2: "b", mode: "screen" }, filter);
  } else {
    addPass();
  }
  document.body.append(host);
  return { id, host, filter, pieces, passes, tiles };
}

function placeFilter(lens: BuiltFilter, width: number, height: number) {
  lens.filter.setAttribute("width", String(width));
  lens.filter.setAttribute("height", String(height));
  lensLayout(width, height, lens.tiles.size).forEach(([x, y, w, h], index) => {
    const piece = lens.pieces[index];
    piece.setAttribute("x", String(x));
    piece.setAttribute("y", String(y));
    piece.setAttribute("width", String(w));
    piece.setAttribute("height", String(h));
  });
}

export function liquidGlass(el: HTMLElement, options: LiquidGlassOptions = {}): LiquidGlassHandle {
  let opts = { ...LENS_DEFAULTS, ...options };
  const supported = isLensSupported();
  let current: { key: string; lens: BuiltFilter; width: number; height: number } | null = null;
  let token = 0;
  let frame = 0;
  let lastChange = 0;
  let settle = 0;
  let destroyed = false;

  const clear = () => {
    token += 1;
    el.style.removeProperty(opts.property);
    current?.lens.host.remove();
    current = null;
  };

  const setScale = (lens: BuiltFilter, bezel: number) => {
    const scale = 2 * opts.edge * bezel;
    const split = lens.passes.length > 1;
    lens.passes.forEach((p, channel) => {
      const share = split ? 1 + (1 - channel) * opts.dispersion : 1;
      p.el.setAttribute("scale", (scale * share).toFixed(1));
    });
  };

  const fit = async () => {
    frame = 0;
    if (destroyed) return;
    const width = Math.round(el.offsetWidth);
    const height = Math.round(el.offsetHeight);
    if (!opts.enabled || width < LENS_MIN_PX || height < LENS_MIN_PX) {
      clear();
      return;
    }
    const style = getComputedStyle(el);
    const cssRadius = Number.parseFloat(style.borderTopLeftRadius) || 0;
    const radius = Math.round(Math.min(opts.radius ?? cssRadius, width / 2, height / 2));
    const corner = opts.corner ?? cornerExponent(style.getPropertyValue("corner-top-left-shape"));
    const bezel = Math.round(Math.max(4, Math.min(opts.rim, Math.min(width, height) / 3.5)));
    const split = opts.dispersion > 0;
    const key = `${radius}:${bezel}:${corner.toFixed(3)}:${opts.power.toFixed(3)}:${split}`;
    api.bezel = bezel;

    if (current?.key === key) {
      if (current.width !== width || current.height !== height) {
        placeFilter(current.lens, width, height);
        current.width = width;
        current.height = height;
      }
      setScale(current.lens, bezel);
      return;
    }

    const now = performance.now();
    const settling = Boolean(current && now - lastChange < SETTLE_MS);
    lastChange = now;
    if (settling && current) {
      if (current.width !== width || current.height !== height) {
        placeFilter(current.lens, width, height);
        current.width = width;
        current.height = height;
      }
      setScale(current.lens, bezel);
      window.clearTimeout(settle);
      settle = window.setTimeout(schedule, SETTLE_MS);
      return;
    }

    if (current && (current.width !== width || current.height !== height)) {
      placeFilter(current.lens, width, height);
      current.width = width;
      current.height = height;
      setScale(current.lens, bezel);
    }

    const mine = ++token;
    const tiles = await lensTiles(radius, bezel, corner, opts.power);
    if (destroyed || mine !== token || !tiles) return;
    const lens = buildFilter(width, height, tiles, split);
    setScale(lens, bezel);
    const previous = current;
    current = { key, lens, width, height };
    el.style.setProperty(opts.property, `url(#${lens.id})`);
    if (previous) {
      requestAnimationFrame(() => requestAnimationFrame(() => previous.lens.host.remove()));
    }
  };

  const schedule = () => {
    if (!destroyed && !frame) frame = requestAnimationFrame(() => { void fit(); });
  };

  const api: LiquidGlassHandle = {
    supported,
    bezel: 0,
    update(next = {}) {
      opts = { ...opts, ...next };
      schedule();
    },
    refresh: () => schedule(),
    destroy() {
      destroyed = true;
      observer?.disconnect();
      cancelAnimationFrame(frame);
      window.clearTimeout(settle);
      clear();
    },
  };

  if (!supported) {
    api.update = () => {};
    api.refresh = () => {};
    api.destroy = () => {};
    return api;
  }

  const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
  observer?.observe(el);
  schedule();
  return api;
}
