import { writeFileSync, realpathSync } from "node:fs";
import { dirname, resolve, relative, isAbsolute, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const repository = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), ".."));
const flags = new Set(["--help", "--run", "--validate", "--json"]);
const values = new Set(["--trace-jsonl", "--trace-context", "--trace-report", "--filter-evidence", "--filter-report", "--admission-progress-socket", "--cleanup-candidate-manifest", "--cleanup-work-root", "--models-root", "--models-work-root", "--init", "--manifest", "--corpus-sidecar", "--experiment", "--non-speech", "--resume", "--checkpoint", "--asr", "--cleanup", "--out", "--gap-ms"]);
function assert(condition, message) { if (!condition) throw new Error(message); }
function externalDirectory(path) {
  const canonical = realpathSync(path);
  const rel = relative(repository, canonical);
  assert(rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel), "Keep audio, manifests and reports outside the repository.");
  return canonical;
}
// Rust owns schemas, paths, WAVs, digests and execution facts. This wrapper only
// parses CLI syntax and creates the intentionally unreviewed legacy template.
export function invokeEvaluator(args, { capture = false } = {}) {
  const binary = process.env.VOICEFLOW_EVAL_BIN;
  const command = binary || "cargo";
  const parameters = binary ? args : ["run", "--quiet", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--features", "evaluation", "--bin", "voiceflow-eval", "--", ...args];
  const result = spawnSync(command, parameters, { cwd: repository, shell: false, stdio: capture ? "pipe" : "inherit", encoding: "utf8", maxBuffer: 2 * 1024 * 1024 });
  if (result.error) throw result.error;
  return result;
}
export function validateManifest(path, invoke = invokeEvaluator) {
  const result = invoke(["--manifest", resolve(path), "--validate", "--json"], { capture: true });
  if (result.status !== 0) {
    let message = result.stderr?.trim() || "Evaluation input rejected.";
    try { message = JSON.parse(message.split("\n").at(-1)).safe_detail; } catch { /* compiler/launcher diagnostics retain their own text */ }
    throw new Error(message);
  }
  return { manifestPath: realpathSync(path), manifest: JSON.parse(result.stdout).manifest };
}
export function main(args, invoke = invokeEvaluator) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    assert(flags.has(arg) || values.has(arg), `Unknown argument: ${arg}`);
    assert(!Object.hasOwn(options, arg), `Repeated argument: ${arg}`);
    if (flags.has(arg)) options[arg] = true;
    else { assert(args[index + 1] && !args[index + 1].startsWith("--"), `Missing value for ${arg}`); options[arg] = args[++index]; }
  }
  assert(!(options["--run"] && options["--validate"]), "Choose --validate or --run.");
  const traceFlags = ["--trace-jsonl", "--trace-context", "--trace-report"];
  if (traceFlags.some((flag) => Object.hasOwn(options, flag))) {
    assert(options["--trace-jsonl"] && options["--trace-report"] && Object.keys(options).every((flag) => traceFlags.includes(flag)), "Offline trace mode requires --trace-jsonl and --trace-report and cannot mix with other evaluator options.");
  }
  const filterFlags = ["--filter-evidence", "--filter-report"];
  if (filterFlags.some((flag) => Object.hasOwn(options, flag))) {
    assert(filterFlags.every((flag) => Object.hasOwn(options, flag)) && Object.keys(options).every((flag) => filterFlags.includes(flag)), "Offline filter mode requires --filter-evidence and --filter-report together and cannot mix with other evaluator options.");
  }
  assert(!options["--admission-progress-socket"] || (options["--validate"] && !options["--run"]), "--admission-progress-socket requires explicit --validate and cannot run providers");
  assert(!(options["--experiment"] && ["--asr", "--cleanup", "--gap-ms"].some((flag) => Object.hasOwn(options, flag))), "--experiment cannot mix with legacy --asr/--cleanup/--gap-ms controls.");
  if (options["--init"]) {
    assert(args.length === 2, "Use --init on its own.");
    const path = resolve(options["--init"]);
    externalDirectory(dirname(path));
    writeFileSync(path, JSON.stringify({ schema_version: 1, provenance: { kind: "human_recorded", note: "Describe the source, consent/rights and de-identification. This is a template, not benchmark evidence.", reviewed: false }, cases: [{ id: "mixed-work-chat-01", audio: "mixed-work-chat-01.wav", reference: "填写人工核对的逐字稿", expected_final: "填写可接受的最终文字", scenario: "中英混合工作聊天", family: "work_chat", focus_kind: "chat", cleanup: "standard", protected: [], reference_variants: [] }] }, null, 2) + "\n", { flag: "wx", mode: 0o600 });
    process.stdout.write(`Template created: ${path}\n`);
    return 0;
  }
  const result = invoke(args.length ? args : ["--help"]);
  return result.status ?? 1;
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { process.exitCode = main(process.argv.slice(2)); }
  catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
