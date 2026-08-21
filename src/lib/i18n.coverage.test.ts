import { describe, expect, it } from "vitest";
import { getEnglishTranslationKeys } from "./i18n";

const productionSources = import.meta.glob("../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

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
});
