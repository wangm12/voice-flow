// @vitest-environment node
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const stylesheet = readFileSync("src/App.css", "utf8");
const websiteStylesheet = readFileSync("website/src/styles.css", "utf8");
const hudStylesheet = readFileSync("src/island.css", "utf8");

function declarations(block, prefix = "color-") {
  return Object.fromEntries([...block.matchAll(new RegExp(`--${prefix}([\\w-]+):\\s*([^;]+);`, "g"))].map((match) => [match[1], match[2].trim()]));
}

const linearize = (value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
const encode = (value) => value <= 0.0031308 ? 12.92 * value : 1.055 * value ** (1 / 2.4) - 0.055;

function rgba(color) {
  if (/^#[\da-f]{6}$/i.test(color)) return [...[1, 3, 5].map((offset) => parseInt(color.slice(offset, offset + 2), 16) / 255), 1];
  const rgb = color.match(/^rgba?\(([^)]+)\)$/);
  if (rgb) {
    const values = rgb[1].split(/[\s,/]+/).filter(Boolean);
    return [...values.slice(0, 3).map((value) => value.endsWith("%") ? parseFloat(value) / 100 : Number(value) / 255), values[3] ? (values[3].endsWith("%") ? parseFloat(values[3]) / 100 : Number(values[3])) : 1];
  }
  const match = color.match(/^oklch\(([\d.]+%?)\s+([\d.]+)\s+([\d.-]+)(?:deg)?(?:\s*\/\s*([\d.]+%?))?\)$/);
  if (!match) throw new Error(`Unsupported contrast color: ${color}`);
  const number = (value) => value.endsWith("%") ? parseFloat(value) / 100 : Number(value);
  const lightness = number(match[1]);
  const chroma = number(match[2]);
  const hue = number(match[3]) * Math.PI / 180;
  const a = chroma * Math.cos(hue);
  const b = chroma * Math.sin(hue);
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.2914855480 * b) ** 3;
  const linear = [4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s, -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s, -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s];
  return [...linear.map((value) => encode(Math.min(1, Math.max(0, value)))), match[4] ? number(match[4]) : 1];
}

function composite(foreground, background, opacity = foreground[3]) {
  return [...foreground.slice(0, 3).map((value, index) => value * opacity + background[index] * (1 - opacity)), 1];
}

function luminance(color) {
  const rgb = typeof color === "string" ? rgba(color) : color;
  const [r, g, b] = rgb.map(linearize);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(foreground, background) {
  const first = luminance(foreground);
  const second = luminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

function expectContrast(foreground, background, minimum, label) {
  expect(contrast(foreground, background), label).toBeGreaterThanOrEqual(minimum);
}

describe("color conversion", () => {
  it("converts chromatic OKLCH and composites alpha before measuring", () => {
    expect(contrast("oklch(0.521966 0.177090 255.830)", "#FDFEFF")).toBeCloseTo(contrast("#0066CC", "#FDFEFF"), 3);
    const translucent = rgba("oklch(0.521966 0.177090 255.830 / 40%)");
    expect(translucent[3]).toBe(0.4);
    expect(luminance(composite(translucent, rgba("#FDFEFF")))).toBeGreaterThan(luminance(translucent));
  });
});

describe("shared HUD contrast", () => {
  const colors = declarations(hudStylesheet.match(/:root\s*\{([^}]+)\}/)[1], "voice-hud-");
  it("keeps labels, auxiliary feedback, and keyboard focus legible", () => {
    for (const surface of ["top", "bottom"]) {
      expectContrast(colors.text, colors[surface], 4.5, `HUD status on ${surface}`);
      expectContrast(colors.muted, colors[surface], 4.5, `HUD auxiliary text on ${surface}`);
      expectContrast(colors.focus, colors[surface], 3, `HUD action focus on ${surface}`);
    }
  });
  it("keeps status text readable over the brightest progress fill", () => {
    const fill = hudStylesheet.match(/\.voice-pill__progress\s*\{[^}]+background:\s*linear-gradient\(90deg,\s*(rgba\([^)]+\))/)[1];
    const opacity = Number(hudStylesheet.match(/\.voice-pill__progress--visible\s*\{[^}]+opacity:\s*([\d.]+)/)[1]);
    const progress = rgba(fill);
    for (const surface of ["top", "bottom"]) expectContrast(colors.text, composite(progress, rgba(colors[surface]), progress[3] * opacity), 4.5, "HUD label over progress");
  });
});

describe("settings contrast", () => {
  const base = declarations(stylesheet.match(/@theme\s*\{([^}]+)\}/)[1]);
  const light = declarations(stylesheet.match(/:root\[data-theme="light"\]\s*\{([^}]+)\}/)[1]);
  const systemLight = declarations(stylesheet.match(/:root:not\(\[data-theme\]\)\s*\{([^}]+)\}/)[1]);

  for (const [theme, overrides] of [["dark", {}], ["light", light], ["system light", systemLight]]) {
    const colors = { ...base, ...overrides };
    it(`${theme} keeps normal text and links readable on settings surfaces`, () => {
      for (const foreground of ["primary", "secondary", "tertiary", "accent", "success-ink", "warning-ink", "error-ink"]) {
        for (const surface of ["base", "card", "elevated", "accent-soft"]) {
          expectContrast(colors[foreground], colors[surface], 4.5, `${foreground} on ${surface}`);
        }
      }
    });
    it(`${theme} keeps action labels readable in every enabled state`, () => {
      for (const background of ["action", "action-hover", "action-pressed"]) expectContrast(colors["action-foreground"], colors[background], 4.5, background);
      for (const background of ["danger-action", "danger-action-hover", "danger-action-pressed"]) expectContrast(colors["danger-action-foreground"], colors[background], 4.5, background);
      for (const background of ["toggle-checked", "toggle-checked-hover", "toggle-checked-pressed"]) expectContrast(colors["toggle-thumb"], colors[background], 3, `switch thumb on ${background}`);
      expectContrast(colors["toggle-idle-thumb"], colors.elevated, 3, "unchecked switch thumb");
      expectContrast(colors.primary, colors.border, 4.5, "pressed secondary, ghost, and icon labels");
      for (const [ink, fill] of [["error-ink", "error"], ["warning-ink", "warning"], ["success-ink", "success"]]) {
        for (const opacity of [0.05, 0.1, 0.15]) {
          expectContrast(colors[ink], composite(rgba(colors[fill]), rgba(colors.card), opacity), 4.5, `${ink} at ${opacity}`);
        }
      }
    });
    it(`${theme} keeps disabled labels and menu focus readable`, () => {
      expectContrast(colors["disabled-foreground"], colors["disabled-background"], 4.5, "disabled control label");
      expectContrast(colors.primary, colors.elevated, 4.5, "focused menu action");
      expectContrast(colors.focus, colors.elevated, 3, "focused menu outline");
    });
    it(`${theme} keeps focus and primary actions distinct`, () => {
      for (const role of ["focus", "action-border"]) {
        for (const surface of ["base", "card", "elevated", "accent-soft"]) expectContrast(colors[role], colors[surface], 3, `${role} on ${surface}`);
      }
      const hover = composite(rgba(colors.elevated), rgba(colors.card), 0.4);
      expectContrast(colors.secondary, hover, 4.5, "disclosure hover text");
      expectContrast(colors.focus, hover, 3, "disclosure hover focus");
    });
  }
});

describe("website contrast", () => {
  const colors = declarations(websiteStylesheet.match(/:root\s*\{([^}]+)\}/)[1], "");
  it("uses readable text, links, and selected controls on actual surfaces", () => {
    for (const ink of ["ink", "muted", "soft-ink", "link"]) {
      for (const surface of ["paper", "surface", "elevated", "demo-surface"]) expectContrast(colors[ink], colors[surface], 4.5, `${ink} on ${surface}`);
    }
    for (const role of ["control-border", "focus"]) {
      for (const surface of ["paper", "surface", "elevated", "demo-surface"]) expectContrast(colors[role], colors[surface], 3, `${role} on ${surface}`);
    }
  });
  it("keeps demo text readable over composited atmosphere and illustration surfaces", () => {
    const block = (selector) => websiteStylesheet.slice(websiteStylesheet.indexOf(`${selector} {`)).match(/^[^{]+\{([^}]+)\}/)[1];
    const color = (selector) => block(selector).match(/(?:^|;)\s*color:\s*([^;]+);/)[1].trim();
    const resolve = (value) => value.startsWith("var(") ? colors[value.slice(6, -1)] : value;
    const firstOklch = (selector) => block(selector).match(/oklch\([^)]+\)/)[0];
    let demoSurface = rgba(colors["demo-surface"]);
    for (const selector of [".demo-stage", ".demo-atmosphere > span"]) demoSurface = composite(rgba(firstOklch(selector)), demoSurface);
    const grain = rgba(firstOklch(".demo-grain"));
    demoSurface = composite(grain, demoSurface, grain[3] * Number(block(".demo-grain").match(/opacity:\s*([\d.]+)/)[1]));
    for (const selector of [".mini-label", ".spoken-example > p", ".spoken-tokens > span", ".spoken-tokens > .is-heard", ".phase-label"]) {
      expectContrast(resolve(color(selector)), demoSurface, 4.5, `${selector} over demo atmosphere`);
    }
    expectContrast(color(".recipient-avatar"), firstOklch(".recipient-avatar"), 4.5, "avatar letter");
    expectContrast(color(".snippet-expansion > p"), colors.surface, 4.5, "expanded snippet");
    expectContrast(colors.ink, colors.elevated, 4.5, "pressed scene label");
  });
  it("keeps every CTA state readable and privacy copy readable on its dark surface", () => {
    for (const background of ["action", "action-hover", "action-pressed"]) expectContrast(colors["action-foreground"], colors[background], 4.5, background);
    for (const ink of ["privacy-ink", "privacy-muted", "privacy-link"]) expectContrast(colors[ink], colors["privacy-surface"], 4.5, ink);
    expectContrast(colors["privacy-link"], colors["privacy-surface"], 3, "privacy focus");
  });
});
