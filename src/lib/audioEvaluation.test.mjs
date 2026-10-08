// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mkdtempSync, writeFileSync, rmSync, readFileSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { main, validateManifest } from "../../scripts/evaluate-audio.mjs";

let directory;
let manifest;
function writeManifest() {
  const path = join(directory, "manifest.json");
  writeFileSync(path, JSON.stringify(manifest));
  return path;
}

describe("audio evaluation entry point", () => {
  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), "voiceflow-audio-eval-test-"));
    manifest = { schema_version: 1, provenance: { kind: "synthetic", note: "Test-only PCM silence", reviewed: true }, cases: [{ id: "one", audio: "one.wav", reference: "Test", expected_final: "Test.", scenario: "test fixture", family: "general", focus_kind: "unknown", cleanup: "standard" }] };
    const wav = Buffer.alloc(44 + 3200);
    wav.write("RIFF", 0); wav.writeUInt32LE(wav.length - 8, 4); wav.write("WAVEfmt ", 8);
    wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
    wav.writeUInt32LE(16000, 24); wav.writeUInt32LE(32000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
    wav.write("data", 36); wav.writeUInt32LE(3200, 40);
    writeFileSync(join(directory, "one.wav"), wav);
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => { vi.restoreAllMocks(); rmSync(directory, { recursive: true, force: true }); });

  it("validates explicitly selected fixtures without needing credentials or contacting providers", () => {
    const path = writeManifest();
    expect(validateManifest(path).manifest.cases).toHaveLength(1);
    expect(main(["--manifest", path, "--validate"])).toBe(0);
    expect(process.stdout.write).toHaveBeenCalledWith(expect.stringContaining("No providers contacted"));
    manifest.cases[0].family = "prompt_or_code";
    manifest.cases[0].focus_kind = "coding_prompt";
    expect(validateManifest(writeManifest()).manifest.cases[0].focus_kind).toBe("coding_prompt");
  });

  it("requires provenance review and unique cases before a live run", () => {
    manifest.provenance.reviewed = false;
    expect(() => validateManifest(writeManifest())).toThrow(/provenance/);
    manifest.provenance.reviewed = true;
    manifest.cases.push({ ...manifest.cases[0] });
    expect(() => validateManifest(writeManifest())).toThrow(/unique/);
  });

  it("rejects path traversal and nonadjacent symlinks", () => {
    manifest.cases[0].audio = "../one.wav";
    expect(() => validateManifest(writeManifest())).toThrow(/adjacent/);
    manifest.cases[0].audio = "linked.wav";
    symlinkSync(resolve("package.json"), join(directory, "linked.wav"));
    expect(() => validateManifest(writeManifest())).toThrow(/outside the repository/);
  });

  it("rejects truncated or incompatible audio before resolving keys", () => {
    const path = writeManifest();
    const wav = readFileSync(join(directory, "one.wav"));
    wav.writeUInt32LE(48000, 24);
    writeFileSync(join(directory, "one.wav"), wav);
    expect(() => validateManifest(path)).toThrow(/16 kHz/);
    writeFileSync(join(directory, "one.wav"), wav.subarray(0, 48));
    expect(() => validateManifest(path)).toThrow();
  });

  it("requires explicit candidate selection for uploads and rejects conflicting actions", () => {
    const path = writeManifest();
    expect(() => main(["--manifest", path, "--run"])).toThrow(/candidate IDs/);
    expect(() => main(["--manifest", path, "--validate", "--run"])).toThrow(/Choose/);
    expect(() => main(["--manifest", path, "--unknown"])).toThrow(/Unknown/);
  });

  it("creates a non-uploadable template and preserves existing files", () => {
    const path = join(directory, "template.json");
    expect(main(["--init", path])).toBe(0);
    expect(JSON.parse(readFileSync(path, "utf8")).provenance.reviewed).toBe(false);
    expect(() => main(["--init", path])).toThrow();
    expect(() => main(["--init", resolve("template.json")])).toThrow(/outside/);
  });
});
