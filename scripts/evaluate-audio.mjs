import { readFileSync, writeFileSync, realpathSync, statSync } from "node:fs";
import { dirname, resolve, relative, isAbsolute, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const repository = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), ".."));
const families = new Set(["general", "email", "browser_search", "work_chat", "personal_chat", "document", "project_management", "calendar_task", "developer_collaboration", "prompt_or_code", "terminal", "form_filling", "notes_journaling", "social_media", "customer_support"]);
const focusKinds = new Set(["secure", "search", "code", "coding_prompt", "terminal", "email", "chat", "document", "form", "editable", "unknown"]);
const asrCandidates = {
  groq_whisper_large_v3_turbo: "GROQ",
  groq_whisper_large_v3: "GROQ",
  openai_gpt_transcribe: "OPENAI",
  qwen3_asr_flash_dashscope: "QWEN",
};
const cleanupCandidates = {
  groq_gpt_oss_120b: "GROQ",
  groq_gpt_oss_20b: "GROQ",
  openai_gpt_4o_mini: "OPENAI",
  qwen_plus_dashscope: "QWEN",
};

function assert(condition, message) { if (!condition) throw new Error(message); }
function boundedText(value, limit = 16_384) { return typeof value === "string" && value.trim() && Buffer.byteLength(value) <= limit; }
function externalExisting(path) {
  const canonical = realpathSync(resolve(path));
  const rel = relative(repository, canonical);
  assert(rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel), "Keep audio, manifests and reports outside the repository.");
  return canonical;
}
function knownFields(value, allowed) {
  assert(value && typeof value === "object" && !Array.isArray(value), "Expected a JSON object.");
  assert(Object.keys(value).every((key) => allowed.includes(key)), "Manifest contains unsupported fields.");
}

function validateWav(bytes) {
  assert(bytes.length > 44 && bytes.length <= 25 * 1024 * 1024, "WAV must be nonempty and at most 25 MiB.");
  assert(bytes.toString("ascii", 0, 4) === "RIFF" && bytes.toString("ascii", 8, 12) === "WAVE", "Expected a PCM WAV file.");
  let format = false;
  let dataBytes = 0;
  for (let offset = 12; offset + 8 <= bytes.length;) {
    const kind = bytes.toString("ascii", offset, offset + 4);
    const size = bytes.readUInt32LE(offset + 4);
    const start = offset + 8;
    assert(start + size <= bytes.length, "WAV contains a truncated chunk.");
    if (kind === "fmt ") {
      assert(size >= 16, "WAV format chunk is too short.");
      assert(bytes.readUInt16LE(start) === 1 && bytes.readUInt16LE(start + 2) === 1 && bytes.readUInt32LE(start + 4) === 16_000 && bytes.readUInt16LE(start + 14) === 16, "Use 16 kHz mono 16-bit PCM WAV, matching dictation audio.");
      format = true;
    }
    if (kind === "data") dataBytes += size;
    offset = start + size + (size % 2);
  }
  assert(format && dataBytes > 0 && dataBytes % 2 === 0 && dataBytes / 32_000 <= 600, "WAV must contain audio of at most 10 minutes.");
}

export function validateManifest(path) {
  const manifestPath = externalExisting(path);
  assert(statSync(manifestPath).size <= 1024 * 1024, "Manifest limit is 1 MiB.");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  knownFields(manifest, ["schema_version", "provenance", "cases"]);
  knownFields(manifest.provenance, ["kind", "note", "reviewed"]);
  assert(manifest.schema_version === 1 && ["human_recorded", "synthetic"].includes(manifest.provenance.kind) && manifest.provenance.reviewed === true && boundedText(manifest.provenance.note, 4096), "Record audio provenance, review the fixtures and set reviewed: true.");
  assert(Array.isArray(manifest.cases) && manifest.cases.length > 0 && manifest.cases.length <= 200, "Select 1–200 audio cases.");
  const ids = new Set();
  for (const entry of manifest.cases) {
    knownFields(entry, ["id", "audio", "reference", "expected_final", "scenario", "family", "focus_kind", "cleanup", "protected", "reference_variants"]);
    assert(boundedText(entry.id, 128) && !ids.has(entry.id), "Case IDs must be unique and nonempty.");
    ids.add(entry.id);
    assert(typeof entry.audio === "string" && /^[^/\\]+\.wav$/.test(entry.audio), "Audio must be an adjacent .wav filename.");
    assert([entry.reference, entry.expected_final, entry.scenario].every((value) => boundedText(value)), "Review the verbatim reference, expected final and scenario for each case.");
    assert(families.has(entry.family) && focusKinds.has(entry.focus_kind) && ["auto", "off", "light", "standard", "heavy"].includes(entry.cleanup), "Unsupported family, focus kind or cleanup intensity.");
    for (const [field, limit] of [["protected", 128], ["reference_variants", 16]]) {
      const values = entry[field] ?? [];
      assert(Array.isArray(values) && values.length <= limit && values.every((value) => boundedText(value)), `Invalid ${field} list.`);
    }
    const audio = externalExisting(resolve(dirname(manifestPath), entry.audio));
    assert(dirname(audio) === dirname(manifestPath), "Audio symlinks must remain adjacent to the manifest.");
    assert(statSync(audio).size <= 25 * 1024 * 1024, "WAV limit is 25 MiB.");
    validateWav(readFileSync(audio));
  }
  return { manifestPath, manifest };
}

function selection(value, choices) {
  const ids = value?.split(",").map((id) => id.trim()) ?? [];
  assert(ids.length && ids.every((id) => Object.hasOwn(choices, id)) && new Set(ids).size === ids.length, `Select comma-separated candidate IDs: ${Object.keys(choices).join(", ")}`);
  return ids;
}

export function main(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    assert(["--help", "--init", "--manifest", "--asr", "--cleanup", "--out", "--gap-ms", "--run", "--validate"].includes(arg), `Unknown argument: ${arg}`);
    assert(!Object.hasOwn(options, arg), `Repeated argument: ${arg}`);
    if (["--help", "--run", "--validate"].includes(arg)) options[arg] = true;
    else { assert(args[index + 1] && !args[index + 1].startsWith("--"), `Missing value for ${arg}`); options[arg] = args[++index]; }
  }
  if (options["--help"] || args.length === 0) {
    process.stdout.write("Audio evaluation (no uploads unless --run):\n  npm run eval:audio -- --init /external/fixtures/manifest.json\n  npm run eval:audio -- --manifest /external/fixtures/manifest.json --validate\n  npm run eval:audio -- --manifest /external/fixtures/manifest.json --asr groq_whisper_large_v3_turbo --cleanup groq_gpt_oss_20b --out /external/results/audio.json --run\nExplicit VOICEFLOW_EVAL_<PROVIDER>_API_KEY or _KEY_FILE is required for live runs.\n");
    return 0;
  }
  assert(!(options["--run"] && options["--validate"]), "Choose --validate or --run.");
  if (options["--init"]) {
    assert(args.length === 2, "Use --init on its own.");
    const path = resolve(options["--init"]);
    externalExisting(dirname(path));
    writeFileSync(path, JSON.stringify({ schema_version: 1, provenance: { kind: "human_recorded", note: "Describe the source, consent/rights and de-identification. This is a template, not benchmark evidence.", reviewed: false }, cases: [{ id: "mixed-work-chat-01", audio: "mixed-work-chat-01.wav", reference: "填写人工核对的逐字稿", expected_final: "填写可接受的最终文字", scenario: "中英混合工作聊天", family: "work_chat", focus_kind: "chat", cleanup: "standard", protected: [], reference_variants: [] }] }, null, 2) + "\n", { flag: "wx", mode: 0o600 });
    process.stdout.write(`Template created: ${path}\n`);
    return 0;
  }
  assert(options["--manifest"], "Select --manifest. Use --help for examples.");
  const { manifestPath, manifest } = validateManifest(options["--manifest"]);
  if (!options["--run"]) {
    process.stdout.write(`Validated ${manifest.cases.length} reviewed ${manifest.provenance.kind} audio cases. No providers contacted.\n`);
    return 0;
  }
  const asr = selection(options["--asr"], asrCandidates);
  const cleanup = selection(options["--cleanup"], cleanupCandidates);
  assert(options["--out"], "Select an external --out report path.");
  const report = resolve(options["--out"]);
  externalExisting(dirname(report));
  const gap = Number(options["--gap-ms"] ?? 250);
  assert(Number.isInteger(gap) && gap >= 0 && gap <= 5000, "Cleanup request gap must be 0–5000 ms.");
  const providers = new Set([...asr.map((id) => asrCandidates[id]), ...cleanup.map((id) => cleanupCandidates[id])]);
  for (const provider of providers) {
    const prefix = `VOICEFLOW_EVAL_${provider}`;
    const key = process.env[`${prefix}_API_KEY`]?.trim();
    const file = process.env[`${prefix}_KEY_FILE`];
    assert(key || (file && readFileSync(file, "utf8").trim()), `Supply ${prefix}_API_KEY or ${prefix}_KEY_FILE explicitly.`);
  }
  process.stdout.write(`Running ${manifest.cases.length} selected audio cases with ASR ${asr.join(", ")} and cleanup ${cleanup.join(", ")}. Reports contain transcripts.\n`);
  const result = spawnSync("cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "live_reviewed_audio_pipeline_uses_production_adapters", "--", "--ignored", "--nocapture", "--test-threads=1"], {
    cwd: repository, stdio: "inherit", shell: false,
    env: { ...process.env, VOICEFLOW_AUDIO_EVAL_OPT_IN: "1", VOICEFLOW_AUDIO_EVAL_MANIFEST: manifestPath, VOICEFLOW_AUDIO_EVAL_ASR_IDS: asr.join(","), VOICEFLOW_LIVE_CLEANUP_CANDIDATE_IDS: cleanup.join(","), VOICEFLOW_AUDIO_EVAL_REPORT: report, VOICEFLOW_LIVE_MIN_REQUEST_GAP_MS: String(gap) },
  });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { process.exitCode = main(process.argv.slice(2)); }
  catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
