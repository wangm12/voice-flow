const HEADER_VALUES = new Set(["word", "term", "phrase", "词", "词条", "术语"]);
export const MAX_DICTIONARY_FILE_BYTES = 1024 * 1024;

export type DictionaryImportResult = {
  words: string[];
  added: number;
  duplicates: number;
  invalid: number;
  limited: number;
};

function parseDelimited(text: string, delimiter: "," | "\t"): string[] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;

  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    const next = text[index + 1];
    if (character === '"') {
      if (quoted && next === '"') {
        cell += '"';
        index += 1;
      } else {
        quoted = !quoted;
      }
    } else if (character === delimiter && !quoted) {
      row.push(cell);
      cell = "";
    } else if ((character === "\n" || character === "\r") && !quoted) {
      if (character === "\r" && next === "\n") index += 1;
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
    } else {
      cell += character;
    }
  }

  if (cell.length > 0 || row.length > 0) {
    row.push(cell);
    rows.push(row);
  }

  return rows.map((values) => values[0] ?? "");
}

export function parseDictionaryText(text: string, fileName = ""): string[] {
  const normalized = text.replace(/^\uFEFF/, "");
  const extension = fileName.toLowerCase().split(".").pop();
  const values = extension === "txt"
    ? normalized.split(/\r?\n/)
    : parseDelimited(normalized, extension === "tsv" ? "\t" : ",");
  const words = values.map((value) => value.trim()).filter(Boolean);
  const first = words[0]?.toLocaleLowerCase();
  return first && HEADER_VALUES.has(first) ? words.slice(1) : words;
}

export function mergeDictionary(existing: string[], imported: string[], limit = 256): DictionaryImportResult {
  const words = [...existing];
  const seen = new Set(existing.map((word) => word.trim().toLocaleLowerCase()));
  let duplicates = 0;
  let invalid = 0;
  let limited = 0;

  for (const value of imported) {
    const word = value.trim();
    if (!word || word.length > 256) {
      invalid += 1;
      continue;
    }
    const normalized = word.toLocaleLowerCase();
    if (seen.has(normalized)) {
      duplicates += 1;
      continue;
    }
    if (words.length >= limit) {
      limited += 1;
      continue;
    }
    seen.add(normalized);
    words.push(word);
  }

  return { words, added: words.length - existing.length, duplicates, invalid, limited };
}
