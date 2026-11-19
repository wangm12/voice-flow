// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/island.css", "utf8");
const settingsCss = readFileSync("src/components/settings/HudAppearanceSettings.css", "utf8");
const glassworkCss = readFileSync("src/components/settings/hud-glasswork/glasswork.css", "utf8");
const livePaletteSelector = ':root[data-hud-appearance="liquid_glass"]';
const visiblePaletteSelector = ':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"]';
const livePillSelector = ':root[data-hud-appearance="liquid_glass"] .voice-pill';
const visibleLivePillSelector = ':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"] .voice-pill';

function extractRule(source, selector) {
  let start = source.indexOf(selector);
  while (start >= 0 && !/[\s,{]/.test(source[start + selector.length] ?? "")) {
    start = source.indexOf(selector, start + selector.length);
  }
  if (start < 0) throw new Error(`Missing CSS rule: ${selector}`);
  const opening = source.indexOf("{", start);
  if (opening < 0) throw new Error(`Missing CSS rule body: ${selector}`);
  const bodyStart = opening + 1;
  const end = source.indexOf("}", bodyStart);
  if (end < 0) throw new Error(`Unclosed CSS rule: ${selector}`);
  return source.slice(bodyStart, end);
}

function extractAtRule(source, header) {
  const start = source.indexOf(`${header} {`);
  if (start < 0) throw new Error(`Missing CSS at-rule: ${header}`);
  const bodyStart = start + header.length + 2;
  let depth = 1;
  for (let index = bodyStart; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    else if (source[index] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(bodyStart, index);
    }
  }
  throw new Error(`Unclosed CSS at-rule: ${header}`);
}

function declarations(block) {
  return Object.fromEntries(
    [...block.matchAll(/([\w-]+)\s*:\s*([^;]+);/g)].map((match) => [match[1], match[2].trim()]),
  );
}

describe("native Liquid Glass HUD appearance", () => {
  it("keeps the light palette and uses the 96% solid surface tint when native glass is hidden", () => {
    const palette = declarations(extractRule(css, livePaletteSelector));
    const pill = declarations(extractRule(css, livePillSelector));
    expect(palette["--voice-hud-text"]).toBe("#171717");
    expect(palette["--voice-glass-shell"]).toContain("#fafafa");
    expect(palette["--voice-glass-live-tint"]).toBe("rgba(255, 255, 255, 0.96)");
    expect(palette["--voice-glass-control"]).toBe("rgba(0, 0, 0, 0.15)");
    expect(pill["corner-shape"]).toBe("round");
    expect(pill.background).toBe("var(--voice-glass-live-tint) !important");
    expect(pill["border-color"]).toBe("transparent !important");
    expect(pill["box-shadow"]).toBe("none !important");
    expect(pill.transition).toBe("none");
  });

  it("uses Glasswork density tints and directional rim catchlights while native glass is visible", () => {
    const visiblePalette = declarations(extractRule(css, visiblePaletteSelector));
    const visiblePill = declarations(extractRule(css, visibleLivePillSelector));
    expect(visiblePalette["--voice-glass-live-tint"]).toBe("rgba(255, 255, 255, 0.12)");
    expect(visiblePill["box-shadow"]).toContain("inset 1.5px 1.5px 1px -1px rgb(255 255 255 / 1)");
    expect(visiblePill["box-shadow"]).toContain("inset 0 6px 8px -6px rgb(255 255 255 / 0.5)");
    expect(declarations(extractRule(css, livePillSelector)).background)
      .toBe("var(--voice-glass-live-tint) !important");
  });

  it("keeps the transparent rim and suppresses synthetic beams for the whole effective Glass lifetime", () => {
    const pill = declarations(extractRule(css, livePillSelector));
    expect(pill["border-color"]).toBe("transparent !important");
    expect(css).toContain(':root[data-hud-appearance="liquid_glass"] .voice-pill__beam-layer,');
    expect(css).not.toContain(':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"] .voice-pill__beam-layer');
    expect(css).not.toContain(".voice-pill::after");
  });

  it("keeps details, recovery, and learning surfaces opaque and legible in both schemes", () => {
    const tray = declarations(extractRule(css, ':root[data-hud-appearance="liquid_glass"] .voice-pill__secondary'));
    const toast = declarations(extractRule(css, ':root[data-hud-appearance="liquid_glass"] .island-learn-toast'));
    expect(tray.background).toBe("var(--voice-glass-shell)");
    expect(tray["border-color"]).toBe("transparent");
    expect(toast.background).toBe("var(--voice-glass-shell)");
    expect(toast["border-color"]).toBe("transparent");
    expect(declarations(extractRule(css, "\n.voice-pill__secondary {")).border).toBe("1px solid var(--voice-hud-border)");

    const secondaryFocus = declarations(extractRule(
      css,
      ".voice-pill__action:focus-visible,\n.voice-pill__secondary button:focus-visible",
    ));
    expect(secondaryFocus.outline).toBe("2px solid var(--voice-hud-focus)");
    expect(secondaryFocus["outline-offset"]).toBe("-2px");

    const darkScheme = extractAtRule(css, "@media (prefers-color-scheme: dark)");
    const darkPalette = declarations(extractRule(darkScheme, livePaletteSelector));
    const darkVisiblePill = declarations(extractRule(darkScheme, visibleLivePillSelector));
    expect(darkPalette["--voice-hud-text"]).toBe("#f5f5f5");
    expect(darkPalette["--voice-hud-error"]).toBe("#f87171");
    expect(darkPalette["--voice-glass-live-tint"]).toBe("rgba(30, 30, 34, 0.96)");
    expect(darkPalette["--voice-glass-control"]).toBe("rgba(255, 255, 255, 0.22)");
    expect(declarations(extractRule(darkScheme, visiblePaletteSelector))["--voice-glass-live-tint"])
      .toBe("rgba(30, 30, 34, 0.24)");
    expect(darkVisiblePill["box-shadow"]).toContain("inset 1px 1px 0.5px -0.5px rgb(255 255 255 / 0.46)");
    expect(darkPalette["--voice-glass-shell"]).toContain("#252729");
    expect(css).not.toContain(':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"] .island-learn-toast');

    const learningFocus = declarations(extractRule(css, ".island-learn-toast__action:focus-visible"));
    expect(learningFocus.outline).toBe("2px solid var(--voice-hud-focus)");
  });

  it("mirrors the monochrome orb only on light Glass surfaces", () => {
    const liveOrb = ':root[data-hud-appearance="liquid_glass"] .voice-pill__orb canvas';
    const pocOrb = ':root[data-native-glass-poc="true"] .voice-pill__orb canvas';
    const previewOrb = '.vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill__orb canvas';
    const darkPreviewOrb = '.vf-hud-preview[data-backdrop="dark"] .vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill__orb canvas';
    expect(declarations(extractRule(css, liveOrb)).filter).toBe("invert(1)");
    expect(declarations(extractRule(css, pocOrb)).filter).toBe("invert(1)");
    expect(declarations(extractRule(css, previewOrb)).filter).toBe("invert(1)");
    expect(declarations(extractRule(css, darkPreviewOrb)).filter).toBe("none");

    const darkScheme = extractAtRule(css, "@media (prefers-color-scheme: dark)");
    expect(declarations(extractRule(darkScheme, liveOrb)).filter).toBe("none");
    expect(declarations(extractRule(darkScheme, pocOrb)).filter).toBe("none");
    const classicOrb = declarations(extractRule(css, "\n.voice-pill__orb canvas {"));
    expect(classicOrb.filter).toBeUndefined();
    expect(classicOrb.transition).toBe("none !important");
  });

  it("falls back to a 96% solid surface without shadow when prefers-reduced-transparency is reduced", () => {
    const reduced = extractAtRule(css, "@media (prefers-reduced-transparency: reduce)");
    expect(declarations(extractRule(reduced, visiblePaletteSelector))["--voice-glass-live-tint"])
      .toBe("rgba(255, 255, 255, 0.96)");
    expect(declarations(extractRule(reduced, visibleLivePillSelector))["box-shadow"])
      .toBe("none !important");

    const reducedDark = extractAtRule(css, "@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark)");
    expect(declarations(extractRule(reducedDark, visiblePaletteSelector))["--voice-glass-live-tint"])
      .toBe("rgba(30, 30, 34, 0.96)");
  });

  it("keeps the isolated POC veil while leaving Settings preview ink layer transparent for the single Glasswork surface", () => {
    const pocRule = declarations(extractRule(css, ':root[data-native-glass-poc="true"] .voice-pill'));
    expect(pocRule.background).toBe("var(--voice-glass-veil) !important");
    expect(pocRule["--voice-glass-veil"]).toBe("rgba(255, 255, 255, 0.6)");
    const darkScheme = extractAtRule(css, "@media (prefers-color-scheme: dark)");
    expect(declarations(extractRule(darkScheme, ':root[data-native-glass-poc="true"] .voice-pill'))["--voice-glass-veil"])
      .toBe("rgba(0, 0, 0, 0.6)");

    const settingsRule = declarations(extractRule(css, '.vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill'));
    expect(settingsRule["corner-shape"]).toBe("round");
    expect(settingsRule["--voice-glass-veil"]).toBe("transparent");
    expect(settingsRule.background).toBe("var(--voice-glass-veil) !important");
    const darkBackdropRule = declarations(extractRule(
      css,
      '.vf-hud-preview[data-backdrop="dark"] .vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill',
    ));
    expect(darkBackdropRule["--voice-glass-veil"]).toBe("transparent");
  });

  it("gives Classic a smooth matte obsidian finish with a subtle inner rim bezel", () => {
    const rootVars = declarations(extractRule(css, ":root {"));
    const classicPill = declarations(extractRule(css, "\n.voice-pill {"));
    expect(rootVars["--voice-hud-top"]).toBe("#141415");
    expect(rootVars["--voice-hud-bottom"]).toBe("#101011");
    expect(rootVars["--voice-hud-border"]).toBe("#2c2c2e");
    expect(classicPill.background).toBe("linear-gradient(180deg, var(--voice-hud-top) 0%, var(--voice-hud-bottom) 100%)");
    expect(classicPill["box-shadow"]).toContain("inset 0 0 0 1px rgba(255, 255, 255, 0.085)");
    expect(css).not.toContain(".voice-pill::before");
  });

  it("wires Glasswork's four declarations, corner-shape: round, and 96% reduced-transparency fallback in glasswork.css", () => {
    const light = declarations(extractRule(glassworkCss, ".vf-glasswork-surface"));
    const dark = declarations(extractRule(glassworkCss, '.vf-glasswork-surface[data-kind="dark"]'));
    const webgl = declarations(extractRule(glassworkCss, '.vf-glasswork-surface[data-refract="webgl"]'));
    const reducedMedia = extractAtRule(glassworkCss, "@media (prefers-reduced-transparency: reduce)");
    const reducedSurface = declarations(extractRule(reducedMedia, ".vf-glasswork-surface"));

    expect(light["corner-shape"]).toBe("round");
    expect(light.color).toBe("var(--ink)");
    expect(light.background).toBe("color-mix(in srgb, var(--m-tint) calc(var(--tint-a) * 100%), transparent)");
    expect(light["backdrop-filter"]).toBe("var(--filter) var(--glass-lens,)");
    expect(light["box-shadow"]).toContain("inset calc(1.5 * var(--u))");
    expect(light["--m-tint"]).toBe("#ffffff");
    expect(light["--m-thin"]).toBe("0.06");
    expect(light["--m-dense"]).toBe("0.22");
    expect(light["--m-sat"]).toBe("1.8");
    expect(light["--m-bri"]).toBe("1.06");

    expect(dark["--m-tint"]).toBe("#1e1e22");
    expect(dark["--m-thin"]).toBe("0.16");
    expect(dark["--m-dense"]).toBe("0.38");
    expect(dark["--m-sat"]).toBe("1.6");
    expect(dark["--m-bri"]).toBe("0.8");
    expect(dark["--well"]).toBe("rgba(255, 255, 255, 0.22)");

    expect(webgl["backdrop-filter"]).toBe("none");
    expect(glassworkCss).toBeTruthy();
    expect(reducedSurface.background).toBe("color-mix(in srgb, var(--m-tint) 96%, transparent)");
    expect(reducedSurface["backdrop-filter"]).toBe("none");

    const subtitle = declarations(extractRule(css, "\n.voice-pill__subtitle {"));
    const stage = declarations(extractRule(settingsCss, ".vf-hud-pg-stage.vf-hud-preview"));
    expect(subtitle["font-size"]).toBe("11px");
    expect(subtitle["font-weight"]).toBe("500");
    expect(stage.transition).toBe("background-color 620ms ease, box-shadow 620ms ease");
  });
});
