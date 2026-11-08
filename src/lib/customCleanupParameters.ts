export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = Record<string, JsonValue>;

const MAX_BYTES = 8 * 1024;
const MAX_DEPTH = 6;
const MAX_NODES = 256;
const MAX_FIELDS = 64;
const MAX_ARRAY_ITEMS = 32;
const MAX_STRING_BYTES = 2 * 1024;
const RESERVED = new Set([
  "model", "messages", "prompt", "input", "file", "audio", "stream", "stream_options",
  "headers", "header", "auth", "authorization", "api_key", "apikey", "key", "tools",
  "tool_choice", "functions", "function_call", "response_format", "modalities", "output_format",
  "role", "content", "n", "parallel_tool_calls",
]);

export type ParameterParseResult =
  | { ok: true; value: JsonObject }
  | { ok: false; error: string };

function normalizedKey(key: string): string {
  return key.toLowerCase().replace(/-/g, "_");
}

function validateTree(value: JsonValue, depth: number, state: { nodes: number }): string | null {
  state.nodes += 1;
  if (state.nodes > MAX_NODES || depth > MAX_DEPTH) return "参数嵌套过深或字段过多。";
  if (typeof value === "string") {
    return new TextEncoder().encode(value).length > MAX_STRING_BYTES ? "单个字符串不能超过 2 KiB。" : null;
  }
  if (Array.isArray(value)) {
    if (value.length > MAX_ARRAY_ITEMS) return "数组最多包含 32 项。";
    for (const item of value) {
      const error = validateTree(item, depth + 1, state);
      if (error) return error;
    }
    return null;
  }
  if (value && typeof value === "object") {
    const keys = Object.keys(value);
    if (keys.length > MAX_FIELDS) return "每个对象最多包含 64 个字段。";
    const normalized = new Set<string>();
    for (const key of keys) {
      const field = normalizedKey(key);
      if (!key || new TextEncoder().encode(key).length > 64 || key.trim() !== key || /[\u0000-\u001f]/.test(key)) {
        return "字段名必须简短且不包含控制字符。";
      }
      if (normalized.has(field)) return "字段名不能重复（包括大小写或连字符差异）。";
      normalized.add(field);
      if (/(__proto__|constructor|prototype|api_key|apikey|authorization|credential|password|secret)/.test(field)
        || field === "auth" || field.startsWith("auth_") || field === "bearer" || field === "token" || field.endsWith("_token")) {
        return "不能在参数中填写密钥、凭据或认证字段。";
      }
      const error = validateTree(value[key], depth + 1, state);
      if (error) return error;
    }
  }
  return null;
}

function validateKnown(key: string, value: JsonValue): string | null {
  const number = typeof value === "number" && Number.isFinite(value) ? value : null;
  const integer = number !== null && Number.isInteger(number) ? number : null;
  const ranges: Record<string, [number, number, boolean]> = {
    temperature: [0, 2, false], top_p: [0, 1, false], min_p: [0, 1, false],
    presence_penalty: [-2, 2, false], frequency_penalty: [-2, 2, false],
    repetition_penalty: [0, 2, false], max_tokens: [1, 131072, true],
    max_completion_tokens: [1, 131072, true], seed: [0, 4294967295, true],
    top_k: [0, 1000, true], top_logprobs: [0, 20, true],
  };
  const range = ranges[key];
  if (range) {
    const candidate = range[2] ? integer : number;
    return candidate !== null && candidate >= range[0] && candidate <= range[1]
      ? null : "已知数值参数超出支持范围。";
  }
  if (key === "reasoning_effort" && !(typeof value === "string" && Array.from(value).length <= 16 && ["default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].includes(value))) {
    return "reasoning_effort 必须是受支持的简短等级名称。";
  }
  if (key === "verbosity" && !(typeof value === "string" && ["low", "medium", "high"].includes(value))) {
    return "verbosity 只能是 low、medium 或 high。";
  }
  if (key === "logprobs" && typeof value !== "boolean") return "logprobs 必须是 true 或 false。";
  if (key === "include_reasoning" && typeof value !== "boolean") return "include_reasoning 必须是 true 或 false。";
  if (key === "stop") {
    const valid = (item: JsonValue) => typeof item === "string" && item.length > 0 && Array.from(item).length <= 256;
    if (valid(value) || Array.isArray(value) && value.length >= 1 && value.length <= 4 && value.every(valid)) return null;
    return "stop 必须是短字符串，或最多包含四个短字符串的数组。";
  }
  return null;
}

class JsonReader {
  private index = 0;
  private nodes = 0;

  constructor(private readonly source: string) {}

  parse(): JsonValue {
    this.skipWhitespace();
    const value = this.readValue(0);
    this.skipWhitespace();
    if (this.index !== this.source.length) throw new Error("JSON 末尾有多余内容。");
    return value;
  }

  private readValue(depth: number): JsonValue {
    this.nodes += 1;
    if (this.nodes > MAX_NODES || depth > MAX_DEPTH) throw new Error("参数嵌套过深或字段过多。");
    this.skipWhitespace();
    const char = this.source[this.index];
    if (char === "{") return this.readObject(depth);
    if (char === "[") return this.readArray(depth);
    if (char === '"') return this.readString();
    if (char === "t" && this.source.slice(this.index, this.index + 4) === "true") { this.index += 4; return true; }
    if (char === "f" && this.source.slice(this.index, this.index + 5) === "false") { this.index += 5; return false; }
    if (char === "n" && this.source.slice(this.index, this.index + 4) === "null") { this.index += 4; return null; }
    const match = this.source.slice(this.index).match(/^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/);
    if (match) {
      this.index += match[0].length;
      const number = Number(match[0]);
      if (!Number.isFinite(number)) throw new Error("数字必须是有限值。");
      return number;
    }
    throw new Error("请输入有效的 JSON 值。");
  }

  private readObject(depth: number): JsonObject {
    this.index += 1;
    this.skipWhitespace();
    const value: JsonObject = Object.create(null) as JsonObject;
    const keys = new Set<string>();
    if (this.source[this.index] === "}") { this.index += 1; return value; }
    while (true) {
      this.skipWhitespace();
      if (this.source[this.index] !== '"') throw new Error("对象字段名必须使用双引号。");
      const key = this.readString();
      if (keys.has(key)) throw new Error("JSON 对象中有重复字段名。");
      keys.add(key);
      this.skipWhitespace();
      if (this.source[this.index] !== ":") throw new Error("对象字段名后需要冒号。");
      this.index += 1;
      value[key] = this.readValue(depth + 1);
      this.skipWhitespace();
      if (this.source[this.index] === "}") { this.index += 1; return value; }
      if (this.source[this.index] !== ",") throw new Error("对象字段之间需要逗号。");
      this.index += 1;
    }
  }

  private readArray(depth: number): JsonValue[] {
    this.index += 1;
    this.skipWhitespace();
    const values: JsonValue[] = [];
    if (this.source[this.index] === "]") { this.index += 1; return values; }
    while (true) {
      if (values.length >= MAX_ARRAY_ITEMS) throw new Error("数组最多包含 32 项。");
      values.push(this.readValue(depth + 1));
      this.skipWhitespace();
      if (this.source[this.index] === "]") { this.index += 1; return values; }
      if (this.source[this.index] !== ",") throw new Error("数组字段之间需要逗号。");
      this.index += 1;
    }
  }

  private readString(): string {
    const start = this.index;
    this.index += 1;
    while (this.index < this.source.length) {
      const char = this.source[this.index];
      if (char === "\\") { this.index += 2; continue; }
      if (char === '"') {
        this.index += 1;
        const parsed: unknown = JSON.parse(this.source.slice(start, this.index));
        if (typeof parsed !== "string") throw new Error("字符串无效。");
        if (new TextEncoder().encode(parsed).length > MAX_STRING_BYTES) throw new Error("单个字符串不能超过 2 KiB。");
        return parsed;
      }
      this.index += 1;
    }
    throw new Error("字符串没有结束。");
  }

  private skipWhitespace() {
    while (/\s/.test(this.source[this.index] ?? "")) this.index += 1;
  }
}

export function parseCustomCleanupParameters(source: string): ParameterParseResult {
  if (new TextEncoder().encode(source).length > MAX_BYTES) return { ok: false, error: "参数 JSON 不能超过 8 KiB。" };
  if (!source.trim()) return { ok: true, value: {} };
  try {
    const parsed = new JsonReader(source).parse();
    if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") return { ok: false, error: "最外层必须是 JSON 对象。" };
    const keys = Object.keys(parsed);
    const seen = new Set<string>();
    for (const key of keys) {
      const field = normalizedKey(key);
      if (seen.has(field)) return { ok: false, error: "字段名不能重复（包括大小写或连字符差异）。" };
      seen.add(field);
      if (isKnownParameter(field) && field !== key) return { ok: false, error: "已知参数请使用标准字段拼写。" };
      if (RESERVED.has(field)) return { ok: false, error: "不能覆盖由 VoiceFlow 管理的请求协议或输出字段。" };
      const error = validateKnown(field, parsed[key]);
      if (error) return { ok: false, error };
    }
    if (seen.has("max_tokens") && seen.has("max_completion_tokens")) return { ok: false, error: "max_tokens 和 max_completion_tokens 只能填写一个。" };
    const treeError = validateTree(parsed, 0, { nodes: 0 });
    return treeError ? { ok: false, error: treeError } : { ok: true, value: parsed };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : "JSON 格式无效。" };
  }
}

function isKnownParameter(key: string): boolean {
  return ["temperature", "top_p", "min_p", "presence_penalty", "frequency_penalty", "repetition_penalty",
    "max_tokens", "max_completion_tokens", "seed", "top_k", "top_logprobs", "reasoning_effort",
    "verbosity", "logprobs", "include_reasoning", "stop"].includes(key);
}
