import { describe, expect, it } from "vitest";
import { getEnglishTranslationKeys } from "./i18n";
import { deliveryReasonHudLabels, deliveryReasonLabels } from "./deliveryCopy";
import { cleanupCandidateLabels, cleanupGuardReasonLabels } from "./cleanupRejection";
import { providerLabelSources } from "./providers";

const productionSources = import.meta.glob("../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// Rust-emitted messages and bundled release notes reach the UI as runtime
// strings, so their table entries cannot be found as frontend literals.
const nativeSources = import.meta.glob(
  ["../../src-tauri/src/**/*.rs", "../../src-tauri/resources/release-notes/*.md"],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

const translationTablePaths = new Set(["./onboardingGuideTranslations.ts", "./dictionaryLearningAuditTranslations.ts"]);

function withoutEnglishTable(path: string, content: string): string {
  if (path !== "./i18n.tsx") return content;
  const start = content.indexOf("const englishTranslations: Record<string, string> = {");
  const end = content.indexOf("\n};\n", start);
  expect(start).toBeGreaterThanOrEqual(0);
  expect(end).toBeGreaterThan(start);
  return content.slice(0, start) + content.slice(end + 4);
}

function cookStringLiteral(raw: string, quote: string): string {
  if (quote === "\"") {
    try {
      return JSON.parse(`"${raw}"`) as string;
    } catch {
      return raw;
    }
  }
  return raw.replace(/\\(['`\\])/g, "$1").replace(/\\n/g, "\n");
}

function collectSourceLiterals(content: string, literals: Set<string>): void {
  for (const match of content.matchAll(/"((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'|`((?:[^`\\]|\\.)*)`/g)) {
    if (match[1] !== undefined) literals.add(cookStringLiteral(match[1], "\""));
    else if (match[2] !== undefined) literals.add(cookStringLiteral(match[2], "'"));
    else if (match[3].includes("${")) collectSourceLiterals(match[3], literals);
    else literals.add(cookStringLiteral(match[3], "`"));
  }
  for (const match of content.matchAll(/>([^<>{}]+)</g)) {
    const text = match[1].trim();
    if (text) literals.add(text);
  }
}

function extractLiteralTranslationKeys(content: string): string[] {
  const keys: string[] = [];
  const pattern = /\bt\(\s*"((?:\\.|[^"\\])*)"\s*\)/g;
  for (const match of content.matchAll(pattern)) {
    keys.push(match[1].replace(/\\"/g, "\""));
  }
  return keys;
}

describe("i18n english coverage", () => {
  it("includes English entries for every literal t() key in production source", () => {
    const englishKeys = getEnglishTranslationKeys();
    const missing: string[] = [];

    for (const [path, content] of Object.entries(productionSources)) {
      if (/\.test\.(ts|tsx)$/.test(path)) continue;
      for (const key of extractLiteralTranslationKeys(content)) {
        if (!englishKeys.has(key)) missing.push(`${key} (${path})`);
      }
    }

    expect(missing).toEqual([]);
  });

  it("keeps no English entry whose source string is absent from production code", () => {
    const literals = new Set<string>();
    for (const [path, content] of Object.entries(productionSources)) {
      if (/\.test\.(ts|tsx)$/.test(path) || path.startsWith("../test/") || translationTablePaths.has(path)) continue;
      collectSourceLiterals(withoutEnglishTable(path, content), literals);
    }
    const native = Object.values(nativeSources).join("\n");
    expect(native.length).toBeGreaterThan(0);

    const orphaned = [...getEnglishTranslationKeys()].filter((key) => !literals.has(key)
      && !native.includes(key)
      && !native.includes(JSON.stringify(key).slice(1, -1)));

    expect(orphaned).toEqual([]);
  });

  it("includes English entries for every exported delivery, cleanup, and provider label", () => {
    const englishKeys = getEnglishTranslationKeys();
    const labelMaps: ReadonlyArray<Readonly<Record<string, string>>> = [
      deliveryReasonLabels,
      deliveryReasonHudLabels,
      cleanupGuardReasonLabels,
      cleanupCandidateLabels,
    ];
    const missing: string[] = [];

    for (const labels of labelMaps) {
      for (const label of Object.values(labels)) {
        if (/[\u4e00-\u9fff]/.test(label) && !englishKeys.has(label)) missing.push(label);
      }
    }
    for (const label of providerLabelSources) {
      if (/[\u4e00-\u9fff]/.test(label) && !englishKeys.has(label)) missing.push(label);
    }

    expect(missing).toEqual([]);
  });
});
