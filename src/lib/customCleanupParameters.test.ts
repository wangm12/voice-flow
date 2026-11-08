import { describe, expect, it } from "vitest";
import { parseCustomCleanupParameters } from "./customCleanupParameters";

describe("custom cleanup request parameters", () => {
  it("accepts empty input and preserves bounded nested extension values", () => {
    expect(parseCustomCleanupParameters(" ")).toEqual({ ok: true, value: {} });
    const parsed = parseCustomCleanupParameters('{"temperature":0,"logprobs":false,"vendor":{"enabled":false,"bias":0}}');
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.value.temperature).toBe(0);
      expect(parsed.value.logprobs).toBe(false);
      expect(parsed.value.vendor).toEqual({ enabled: false, bias: 0 });
    }
  });

  it("rejects duplicate decoded keys at every object level", () => {
    expect(parseCustomCleanupParameters('{"temperature":0.1,"temperature":0.2}').ok).toBe(false);
    expect(parseCustomCleanupParameters('{"vendor":{"a":1,"\\u0061":2}}').ok).toBe(false);
  });

  it("rejects VoiceFlow-owned protocol and output fields and credential fields", () => {
    for (const source of [
      '{"model":"other"}',
      '{"messages":[]}',
      '{"stream":false}',
      '{"response_format":{"type":"json_object"}}',
      '{"vendor":{"authorization":"secret"}}',
      '{"vendor":{"apiKey":"secret"}}',
    ]) expect(parseCustomCleanupParameters(source).ok).toBe(false);
  });

  it("checks known control types and bounds, including token field exclusivity", () => {
    expect(parseCustomCleanupParameters('{"temperature":0.3,"max_completion_tokens":2048,"include_reasoning":false}').ok).toBe(true);
    expect(parseCustomCleanupParameters('{"reasoning_effort":"default"}').ok).toBe(true);
    for (const source of [
      '{"temperature":2.1}',
      '{"top_p":-0.1}',
      '{"max_tokens":100,"max_completion_tokens":100}',
      '{"max_tokens":1.5}',
      '{"include_reasoning":"yes"}',
      '{"Temperature":0.2}',
      '{"max-tokens":100}',
    ]) expect(parseCustomCleanupParameters(source).ok).toBe(false);
  });

  it("counts known string limits by Unicode scalar values like Rust", () => {
    expect(parseCustomCleanupParameters(JSON.stringify({ stop: "停".repeat(100) })).ok).toBe(true);
    expect(parseCustomCleanupParameters(JSON.stringify({ stop: "停".repeat(256) })).ok).toBe(true);
    expect(parseCustomCleanupParameters(JSON.stringify({ stop: "停".repeat(257) })).ok).toBe(false);
    expect(parseCustomCleanupParameters(JSON.stringify({ stop: "😀".repeat(256) })).ok).toBe(true);
    expect(parseCustomCleanupParameters(JSON.stringify({ repetition_penalty: 0 })).ok).toBe(true);
    expect(parseCustomCleanupParameters(JSON.stringify({ repetition_penalty: -0.01 })).ok).toBe(false);
  });

  it("bounds payload size, nesting, and array length", () => {
    expect(parseCustomCleanupParameters(JSON.stringify({ vendor: "x".repeat(8 * 1024) })).ok).toBe(false);
    expect(parseCustomCleanupParameters('{"vendor":{"a":{"b":{"c":{"d":{"e":{"f":1}}}}}}}').ok).toBe(false);
    expect(parseCustomCleanupParameters(JSON.stringify({ vendor: Array(33).fill(0) })).ok).toBe(false);
  });
});
