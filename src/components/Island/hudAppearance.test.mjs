// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/island.css", "utf8");
const livePaletteSelector = ':root[data-hud-appearance="liquid_glass"]';
const livePillSelector = ':root[data-hud-appearance="liquid_glass"] .voice-pill';

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
  it("keeps the light palette and shared high-opacity tint across native material handoff", () => {
    const palette = declarations(extractRule(css, livePaletteSelector));
    const pill = declarations(extractRule(css, livePillSelector));
    expect(palette["--voice-hud-text"]).toBe("#171717");
    expect(palette["--voice-glass-shell"]).toContain("#fafafa");
    expect(palette["--voice-glass-live-tint"]).toBe("rgba(255, 255, 255, 0.92)");
    expect(pill.background).toBe("var(--voice-glass-live-tint) !important");
    expect(pill["border-color"]).toBe("transparent !important");
    expect(pill["box-shadow"]).toBe("none !important");
    expect(pill.transition).toBe("none");
  });

  it("does not switch the live pill's tint when native material visibility changes", () => {
    expect(css).not.toContain(':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"] .voice-pill');
    expect(declarations(extractRule(css, livePillSelector)).background)
      .toBe("var(--voice-glass-live-tint) !important");
  });

  it("keeps the transparent rim and suppresses synthetic beams for the whole effective Glass lifetime", () => {
    const pill = declarations(extractRule(css, livePillSelector));
    expect(pill["border-color"]).toBe("transparent !important");
    expect(css).toContain(':root[data-hud-appearance="liquid_glass"] .voice-pill__beam-layer,');
    expect(css).not.toContain(':root[data-hud-appearance="liquid_glass"][data-hud-glass-visible="true"] .voice-pill__beam-layer');
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
    expect(darkPalette["--voice-hud-text"]).toBe("#f5f5f5");
    expect(darkPalette["--voice-hud-error"]).toBe("#f87171");
    expect(darkPalette["--voice-glass-live-tint"]).toBe("rgba(25, 25, 25, 0.92)");
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

  it("keeps the dark palette tied to the system color scheme", () => {
    const darkScheme = extractAtRule(css, "@media (prefers-color-scheme: dark)");
    const palette = declarations(extractRule(darkScheme, livePaletteSelector));
    expect(palette["--voice-hud-text"]).toBe("#f5f5f5");
    expect(palette["--voice-glass-live-tint"]).toBe("rgba(25, 25, 25, 0.92)");
    expect(palette["--voice-glass-shell"]).toContain("#252729");
    expect(palette["--voice-glass-veil"]).toBe("rgba(0, 0, 0, 0.6)");
  });

  it("preserves the POC and Settings preview glass treatments", () => {
    const previewRule = extractRule(css, ':root[data-native-glass-poc="true"] .voice-pill');
    expect(declarations(previewRule).background).toBe("var(--voice-glass-veil) !important");
    const darkScheme = extractAtRule(css, "@media (prefers-color-scheme: dark)");
    expect(declarations(extractRule(darkScheme, ':root[data-native-glass-poc="true"] .voice-pill'))["--voice-glass-veil"])
      .toBe("rgba(0, 0, 0, 0.6)");

    const settingsRule = declarations(extractRule(css, '.vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill'));
    expect(settingsRule.background).toBe("var(--voice-glass-veil) !important");
    const darkBackdropRule = declarations(extractRule(
      css,
      '.vf-hud-preview[data-backdrop="dark"] .vf-hud-preview-hud[data-appearance="liquid_glass"] .voice-pill',
    ));
    expect(darkBackdropRule["--voice-glass-veil"]).toBe("rgba(34, 36, 40, 0.28)");
  });
});
