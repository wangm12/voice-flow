// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { main, validateManifest } from "../../scripts/evaluate-audio.mjs";

let directory;
describe("audio evaluation wrapper syntax and dispatch", () => {
  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), "voiceflow-audio-eval-test-"));
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => { vi.restoreAllMocks(); rmSync(directory, { recursive: true, force: true }); });
  it("dispatches validation and real execution to the same Rust owner", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    const args = ["--manifest", "/external/manifest.json", "--validate"];
    expect(main(args, invoke)).toBe(0);
    expect(invoke).toHaveBeenCalledWith(args);
    const run = ["--manifest", "/external/manifest.json", "--corpus-sidecar", "/external/corpus.json", "--experiment", "/external/recipe.json", "--run", "--out", "/external/report.json"];
    expect(main(run, invoke)).toBe(0);
    expect(invoke).toHaveBeenLastCalledWith(run);
    const replay = [...run, "--resume", "/external/prior-checkpoint.json", "--checkpoint", "/external/new-checkpoint.json"];
    expect(main(replay, invoke)).toBe(0);
    expect(invoke).toHaveBeenLastCalledWith(replay);
    const local = [...run, "--models-root", "/external/models-original", "--models-work-root", "/external/new-workspace"];
    expect(main(local, invoke)).toBe(0);
    expect(invoke).toHaveBeenLastCalledWith(local);
  });
  it("forwards an explicit standalone V2 negative experiment without inventing speech paths", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    const args = ["--experiment", "/external/negative-v2.json", "--non-speech", "/external/negative-index.json", "--validate", "--json"];
    expect(main(args, invoke)).toBe(0);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith(args);
    expect(args).not.toContain("--manifest");
    expect(args).not.toContain("--corpus-sidecar");
  });
  it("forwards either explicit V3 ASR-only purpose through the shared experiment owner", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    for (const purpose of ["functional_unscored", "publisher_reference_asr"]) {
      const args = ["--experiment", `/external/${purpose}-v3.json`, "--validate", "--json"];
      expect(main(args, invoke)).toBe(0);
      expect(invoke).toHaveBeenLastCalledWith(args);
      expect(args).not.toContain("--manifest");
      expect(args).not.toContain("--corpus-sidecar");
    }
  });
  it("forwards an audited60 recipe without guessing source/profile metadata", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    const args = ["--experiment", "/external/audited60/recipe.json", "--validate", "--json"];
    expect(main(args, invoke)).toBe(0);
    expect(invoke).toHaveBeenCalledWith(args);
    expect(invoke).toHaveBeenCalledTimes(1);
  });
  it("forwards only an explicit validate-only admission socket choice", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    const args = ["--manifest", "/external/speech.json", "--validate", "--json", "--admission-progress-socket", "/private/tmp/owned/p.sock"];
    expect(main(args, invoke)).toBe(0);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith(args);
    invoke.mockClear();
    for (const bad of [
      ["--admission-progress-socket"],
      ["--admission-progress-socket", "/external/p.sock"],
      ["--run", "--admission-progress-socket", "/external/p.sock"],
      ["--validate", "--admission-progress-socket", "/external/a.sock", "--admission-progress-socket", "/external/b.sock"],
      ["--validate", "--admission-progress"],
    ]) expect(() => main(bad, invoke)).toThrow();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("forwards the offline trace report entry point and rejects incomplete or mixed modes", () => {
    const invoke = vi.fn().mockReturnValue({ status: 0 });
    const args = ["--trace-jsonl", "/external/traces.jsonl", "--trace-report", "/external/new-report.json", "--trace-context", "/external/branches.json"];
    expect(main(args, invoke)).toBe(0);
    expect(invoke).toHaveBeenCalledWith(args);
    invoke.mockClear();
    for (const bad of [
      ["--trace-jsonl", "/external/traces.jsonl"],
      ["--trace-report", "/external/report.json"],
      [...args, "--manifest", "/external/audio.json"],
      ["--trace-jsonl", "/external/a.jsonl", "--trace-report", "/external/a.json", "--trace-jsonl", "/external/b.jsonl"],
    ]) expect(() => main(bad, invoke)).toThrow();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("forwards the paired filter evidence mode and preserves its UNKNOWN exit code", () => {
    const invoke = vi.fn().mockReturnValue({ status: 3 });
    const args = ["--filter-evidence", "/external/evidence.json", "--filter-report", "/external/report.json"];
    expect(main(args, invoke)).toBe(3);
    expect(invoke).toHaveBeenCalledWith(args);
    invoke.mockClear();
    for (const bad of [
      ["--filter-evidence", "/external/evidence.json"],
      ["--filter-report", "/external/report.json"],
      [...args, "--validate"],
    ]) expect(() => main(bad, invoke)).toThrow();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("preserves executable error and cancellation exit codes", () => {
    expect(main(["--help"], () => ({ status: 2 }))).toBe(2);
    expect(main(["--help"], () => ({ status: 130 }))).toBe(130);
  });
  it("rejects syntax conflicts before dispatch", () => {
    const invoke = vi.fn();
    expect(() => main(["--validate", "--run"], invoke)).toThrow(/Choose/);
    expect(() => main(["--unknown"], invoke)).toThrow(/Unknown/);
    expect(() => main(["--manifest"], invoke)).toThrow(/Missing/);
    expect(() => main(["--run", "--run"], invoke)).toThrow(/Repeated/);
    expect(() => main(["--experiment", "recipe", "--asr", "candidate"], invoke)).toThrow(/cannot mix/);
    expect(invoke).not.toHaveBeenCalled();
  });
  it("consumes the Rust manifest alongside versioned artifact facts without reinterpreting them", () => {
    const path = join(directory, "manifest.json");writeFileSync(path, "{}");
    const invoke = vi.fn().mockReturnValue({ status: 0, stdout: JSON.stringify({ manifest: { cases: [{ id: "one" }] }, baseline: { schema_version: 2, source_commit: null, dirty: null, source_sha256: null, source_provenance: { kind: "unknown", reason: "compiled_source_metadata_unavailable" }, executable_sha256: "a".repeat(64) } }) });
    expect(validateManifest(path, invoke).manifest.cases[0].id).toBe("one");
    expect(invoke).toHaveBeenCalledWith(["--manifest", path, "--validate", "--json"], { capture: true });
    expect(() => validateManifest(path, () => ({status:2,stderr:JSON.stringify({safe_detail:"provenance not reviewed"})}))).toThrow(/provenance/);
  });
  it("creates a non-uploadable template and preserves existing files", () => {
    const path = join(directory, "template.json");
    expect(main(["--init", path])).toBe(0);
    expect(JSON.parse(readFileSync(path, "utf8")).provenance.reviewed).toBe(false);
    expect(() => main(["--init", path])).toThrow();
    expect(() => main(["--init", resolve("template.json")])).toThrow(/outside/);
  });
});
