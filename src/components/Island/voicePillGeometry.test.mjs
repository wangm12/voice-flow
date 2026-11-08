// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/island.css", "utf8");
const tokens = readFileSync("src/components/Island/voicePillTokens.ts", "utf8");

function declarations(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const blocks = [...css.matchAll(new RegExp(`${escaped}\\s*\\{([^}]+)\\}`, "g"))];
  const block = blocks.at(-1)?.[1];
  if (!block) throw new Error(`Missing CSS rule: ${selector}`);
  return Object.fromEntries(
    [...block.matchAll(/([\w-]+)\s*:\s*([^;]+);/g)].map((match) => [match[1], match[2].trim()]),
  );
}

function token(name) {
  const value = tokens.match(new RegExp(`export const ${name} = (\\d+);`))?.[1];
  if (!value) throw new Error(`Missing HUD geometry token: ${name}`);
  return Number(value);
}

describe("VoicePill native host geometry", () => {
  it("keeps the primary capsule at the same bottom-anchored offset when details open", () => {
    const stack = declarations(".voice-pill-stack");
    const expanded = declarations(".voice-pill-stack--with-details");
    const primary = token("voicePillHeight");
    const collapsedHost = token("voicePillWindowHeight");
    const expandedHost = token("voicePillRecoveryHeight");
    const paddingBottom = Number.parseFloat(stack["padding-bottom"]);
    const tray = Number.parseFloat(declarations(".voice-pill__secondary").height);
    const gap = Number.parseFloat(stack.gap);

    expect(stack["justify-content"]).toBe("flex-end");
    expect(expanded["flex-direction"]).toBe("column-reverse");
    expect(expanded["justify-content"]).toBe("flex-start");
    expect(expanded["padding-bottom"]).toBe(`${paddingBottom}px`);
    expect(expandedHost).toBe(primary + tray + gap + paddingBottom);

    const collapsedPrimaryTop = collapsedHost - paddingBottom - primary;
    const expandedPrimaryTop = expandedHost - paddingBottom - primary;
    const nativeOriginShift = collapsedHost - expandedHost;
    expect(collapsedPrimaryTop).toBe(expandedPrimaryTop + nativeOriginShift);
    expect(collapsedPrimaryTop).toBe(8);
  });
});
