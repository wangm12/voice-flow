import { describe, expect, it } from "vitest";
import { MAX_DICTIONARY_FILE_BYTES, mergeDictionary, parseDictionaryText } from "./dictionaryImport";

describe("dictionary import", () => {
  it("keeps the import size limit bounded", () => {
    expect(MAX_DICTIONARY_FILE_BYTES).toBe(1024 * 1024);
  });

  it("parses CSV headers, quoted commas, and only the first column", () => {
    expect(parseDictionaryText("term,notes\nVoiceFlow,product\n\"Cursor, IDE\",code", "terms.csv"))
      .toEqual(["VoiceFlow", "Cursor, IDE"]);
  });

  it("parses plain text and removes the UTF-8 BOM", () => {
    expect(parseDictionaryText("\uFEFFNotion\n\nOpenAI\n", "terms.txt")).toEqual(["Notion", "OpenAI"]);
  });

  it("deduplicates case-insensitively and enforces the dictionary limit", () => {
    const result = mergeDictionary(["VoiceFlow"], ["voiceflow", "Cursor", "", "x".repeat(257)], 2);
    expect(result.words).toEqual(["VoiceFlow", "Cursor"]);
    expect(result.duplicates).toBe(1);
    expect(result.invalid).toBe(2);
  });
});
