import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cleanupCandidateLabel, cleanupGuardReasonLabel, type CleanupRejectionStage } from "../lib/cleanupRejection";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass, ghostButtonClass, focusRingClass } from "../lib/theme";
import type { WritingMode } from "./ContextSettings";

type TrialResult = {
  text: string;
  status: "model" | "off" | "local_only" | "provider_fallback" | "guard_fallback";
  model_candidate?: string | null;
  provider_candidate?: string | null;
  cleanup_transformations?: string[];
  cleanup_rejection?: { candidate: string; reason: string; stage: CleanupRejectionStage } | null;
  provider_error?: {
    kind: "network" | "timeout" | "context_authorization_changed" | "unauthorized" | "rate_limited" | "server" | "guard" | "local_native" | "other";
    guard: string | null;
    native_failure?: NativeFailureFacts | null;
  } | null;
  final_guard_rejected?: boolean;
  elapsed_ms: number;
};
type TrialResponse = { saved: TrialResult | null; draft: TrialResult };

type NativeFailureStage = "endpoint_admission" | "model_admission" | "input_admission";
type NativeFailureFacts = {
  kind: "network" | "timeout" | "server" | "local_native";
  status_code: string | null;
  stage: NativeFailureStage | null;
  dispatched: boolean | null;
  input_utf8_bytes: number | null;
  max_input_bytes: number | null;
  certainty: "known" | "unknown" | "invalid";
};

const resultStatuses = new Set<TrialResult["status"]>([
  "model", "off", "local_only", "provider_fallback", "guard_fallback",
]);
const providerErrorKinds = new Set<NonNullable<TrialResult["provider_error"]>["kind"]>([
  "network", "timeout", "context_authorization_changed", "unauthorized", "rate_limited",
  "server", "guard", "local_native", "other",
]);
const nativeFailureStatuses = new Set([
  "unreachable", "timeout", "invalid_endpoint", "model_missing", "model_mismatch",
  "instance_not_ready", "reasoning_off_unavailable", "remote_instance",
  "instance_source_unknown", "cli_unavailable", "cli_timeout", "cli_failed",
  "invalid_response", "http_error", "body_too_large", "invalid_input", "input_too_large",
]);
const nativeFailureStages = new Set<NativeFailureStage>([
  "endpoint_admission", "model_admission", "input_admission",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nullableString(value: unknown): string | null | undefined {
  if (value === null) return null;
  return typeof value === "string" ? value : undefined;
}

function nullableBoolean(value: unknown): boolean | null | undefined {
  if (value === null) return null;
  return typeof value === "boolean" ? value : undefined;
}

function nullableSafeInteger(value: unknown): number | null | undefined {
  if (value === null) return null;
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function unknownNativeFailure(kind: NativeFailureFacts["kind"], certainty: "unknown" | "invalid" = "unknown"): NativeFailureFacts {
  return { kind, status_code: null, stage: null, dispatched: null, input_utf8_bytes: null, max_input_bytes: null, certainty };
}

function parseNativeFailure(value: unknown, expectedKind: NativeFailureFacts["kind"]): NativeFailureFacts {
  if (!isRecord(value) || value.kind !== expectedKind) return unknownNativeFailure(expectedKind, "invalid");
  const status = nullableString(value.status_code);
  const rawStage = nullableString(value.stage);
  const dispatched = nullableBoolean(value.dispatched);
  const inputBytes = nullableSafeInteger(value.input_utf8_bytes);
  const maxBytes = nullableSafeInteger(value.max_input_bytes);
  if (status === undefined || rawStage === undefined || dispatched === undefined || inputBytes === undefined || maxBytes === undefined) {
    return unknownNativeFailure(expectedKind, "invalid");
  }
  if (status === null || !nativeFailureStatuses.has(status)) return unknownNativeFailure(expectedKind);
  const stage = rawStage !== null && nativeFailureStages.has(rawStage as NativeFailureStage)
    ? rawStage as NativeFailureStage : null;
  if (rawStage !== null && stage === null) return { ...unknownNativeFailure(expectedKind, "invalid"), status_code: status };

  let coherent = false;
  switch (status) {
    case "invalid_endpoint":
      coherent = stage === "endpoint_admission" && dispatched === false && inputBytes === null && maxBytes === null;
      break;
    case "model_mismatch":
      coherent = stage === "model_admission" && dispatched === false && inputBytes === null && maxBytes === null;
      break;
    case "invalid_input":
      coherent = stage === "input_admission" && dispatched === false && inputBytes === null && maxBytes === null;
      break;
    case "input_too_large":
      coherent = stage === "input_admission" && dispatched === false
        && inputBytes !== null && maxBytes !== null && maxBytes > 0 && inputBytes > maxBytes;
      break;
    default:
      coherent = stage === null && dispatched === null && inputBytes === null && maxBytes === null;
  }
  return {
    kind: expectedKind,
    status_code: status,
    stage: coherent ? stage : null,
    dispatched: coherent ? dispatched : null,
    input_utf8_bytes: coherent ? inputBytes : null,
    max_input_bytes: coherent ? maxBytes : null,
    certainty: coherent ? "known" : "invalid",
  };
}

function parseProviderError(value: unknown): TrialResult["provider_error"] | undefined {
  if (value === null) return null;
  if (!isRecord(value)) return undefined;
  const rawKind = value.kind;
  const kind = typeof rawKind === "string" && providerErrorKinds.has(rawKind as NonNullable<TrialResult["provider_error"]>["kind"])
    ? rawKind as NonNullable<TrialResult["provider_error"]>["kind"] : "other";
  const guard = nullableString(value.guard);
  if (guard === undefined) return undefined;
  const nativeKinds = new Set(["network", "timeout", "server", "local_native"]);
  if (value.native_failure === null || value.native_failure === undefined) {
    return kind === "local_native"
      ? { kind, guard, native_failure: unknownNativeFailure("local_native", "invalid") }
      : { kind, guard, native_failure: null };
  }
  if (!nativeKinds.has(kind)) return { kind: "other", guard: null, native_failure: null };
  return { kind, guard, native_failure: parseNativeFailure(value.native_failure, kind as NativeFailureFacts["kind"]) };
}

function parseTrialResult(value: unknown): TrialResult | null {
  if (!isRecord(value) || typeof value.text !== "string" || typeof value.elapsed_ms !== "number"
    || !Number.isSafeInteger(value.elapsed_ms) || value.elapsed_ms < 0
    || typeof value.status !== "string" || !resultStatuses.has(value.status as TrialResult["status"])) return null;
  const providerError = Object.prototype.hasOwnProperty.call(value, "provider_error")
    ? parseProviderError(value.provider_error) : null;
  if (providerError === undefined) return null;
  const cleanupRejection = isRecord(value.cleanup_rejection)
    && typeof value.cleanup_rejection.candidate === "string"
    && typeof value.cleanup_rejection.reason === "string"
    && typeof value.cleanup_rejection.stage === "string"
    ? { candidate: value.cleanup_rejection.candidate, reason: value.cleanup_rejection.reason, stage: value.cleanup_rejection.stage as CleanupRejectionStage }
    : null;
  const transformations = Array.isArray(value.cleanup_transformations)
    && value.cleanup_transformations.every((item) => typeof item === "string")
    ? value.cleanup_transformations as string[] : [];
  return {
    text: value.text,
    status: value.status as TrialResult["status"],
    elapsed_ms: value.elapsed_ms,
    model_candidate: typeof value.model_candidate === "string" ? value.model_candidate : null,
    provider_candidate: typeof value.provider_candidate === "string" ? value.provider_candidate : null,
    cleanup_transformations: transformations,
    cleanup_rejection: cleanupRejection,
    provider_error: providerError,
    final_guard_rejected: value.final_guard_rejected === true,
  };
}

function parseTrialResponse(value: unknown): TrialResponse | null {
  if (!isRecord(value) || !Object.prototype.hasOwnProperty.call(value, "draft")) return null;
  const draft = parseTrialResult(value.draft);
  const saved = value.saved === null ? null : parseTrialResult(value.saved);
  return draft && (value.saved === null || saved) ? { saved, draft } : null;
}

const statusLabels: Record<TrialResult["status"], string> = {
  model: "模型结果",
  off: "整理已关闭，未调用模型",
  local_only: "本地处理，未调用模型",
  provider_fallback: "服务未返回结果，已回退本地处理",
  guard_fallback: "事实保护规则触发，已回退",
};

function outcomeLabel(result: TrialResult): string {
  if (result.final_guard_rejected
    && result.model_candidate == null
    && result.cleanup_rejection?.stage === "provider_output") {
    return "服务商候选未采纳，本地回退结果也未通过最终保真检查";
  }
  if (result.final_guard_rejected) return "最终事实保护未接受候选，已回退";
  const error = result.provider_error;
  if (error?.kind === "guard") {
    return cleanupGuardReasonLabel(error.guard) === "具体原因未确认"
      ? "模型回复无法按支持的格式确认，已回退"
      : statusLabels.guard_fallback;
  }
  if (error?.native_failure?.status_code === "input_too_large" && error.native_failure.certainty === "known") {
    return "本机整理输入超过允许容量，已在发送前拒绝";
  }
  switch (error?.kind) {
    case "timeout": return "整理请求超时，已回退本地处理";
    case "network": return "整理连接失败，已回退本地处理";
    case "unauthorized": return "整理服务拒绝凭据，已回退本地处理";
    case "rate_limited": return "整理服务限流，已回退本地处理";
    case "server": return "整理服务出错，已回退本地处理";
    case "context_authorization_changed": return "整理授权已变化，结果未作为模型成功";
    case "local_native": return "本机整理服务未完成请求，已回退本地处理";
    case "other": return "整理未完成，具体原因未确认，已回退";
  }
  return statusLabels[result.status] ?? "预览结果状态无法确认";
}

function cleanupTransformationLabel(transformation: string, t: (value: string) => string): string {
  switch (transformation) {
    case "restore_spoken_layout": return t("恢复口述结构");
    default: return t("其他共享整理转换");
  }
}

function nativeFailureStatusLabel(failure: NativeFailureFacts): string {
  if (failure.certainty !== "known" || failure.status_code === null) return "本机整理服务状态未确认";
  switch (failure.status_code) {
    case "input_too_large": return "本机整理输入超过允许容量";
    case "invalid_input": return "本机整理输入未通过格式检查";
    case "unreachable": return "本机整理服务无法连接";
    case "timeout": return "本机整理服务请求超时";
    case "invalid_endpoint": return "本机整理服务地址无效";
    case "model_missing": return "本机整理模型未找到";
    case "model_mismatch": return "本机整理模型与所选实例不匹配";
    case "instance_not_ready": return "本机整理实例尚未就绪";
    case "reasoning_off_unavailable": return "本机整理实例不支持关闭推理";
    case "remote_instance": return "本机整理拒绝远程实例";
    case "instance_source_unknown": return "本机整理实例来源无法确认";
    case "invalid_response": return "本机整理回复格式无效";
    case "http_error": return "本机整理服务返回错误";
    case "body_too_large": return "本机整理回复超过允许容量";
    default: return "本机整理服务状态未确认";
  }
}

export function WritingPreview({ mode, compareSaved, isDraft = compareSaved }: { mode: WritingMode; compareSaved: boolean; isDraft?: boolean }) {
  const { t } = useI18n();
  const [text, setText] = useState(() => t("嗯，帮我跟同事说一下，周五下午三点我们一起看 API 方案，先确认接口，再安排测试。"));
  const [result, setResult] = useState<TrialResponse | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestRef = useRef<string | null>(null);
  const runButtonRef = useRef<HTMLButtonElement>(null);
  const restoreRunFocusRef = useRef(false);
  const reasonId = useId();
  const unavailableReason = !mode.label.trim() ? t("请先填写语气名称。")
    : !mode.prompt.trim() ? t("请先填写 Prompt。")
      : !text.trim() ? t("请先填写试跑文本。")
        : new TextEncoder().encode(text).length > 16_384 ? t("试跑文本过长，请缩短后再试。") : null;

  useEffect(() => {
    if (!pending && restoreRunFocusRef.current) {
      restoreRunFocusRef.current = false;
      runButtonRef.current?.focus({ preventScroll: true });
    }
  }, [pending]);

  function cancel() {
    const requestId = requestRef.current;
    requestRef.current = null;
    if (requestId) void invoke("cancel_writing_preview", { requestId }).catch(() => {});
  }

  useEffect(() => {
    cancel();
    setPending(false);
    setResult(null);
    setError(null);
    return cancel;
  }, [mode.id, mode.label, mode.prompt, mode.family, compareSaved]);

  async function run() {
    cancel();
    const requestId = crypto.randomUUID();
    requestRef.current = requestId;
    setPending(true);
    setResult(null);
    setError(null);
    try {
      const wireResponse: unknown = await invoke("preview_writing_mode", {
        request: { request_id: requestId, text, mode, compare_saved: compareSaved },
      });
      const response = parseTrialResponse(wireResponse);
      if (!response) throw new Error("preview_invalid_response");
      if (requestRef.current === requestId) setResult(response);
    } catch (reason) {
      if (requestRef.current !== requestId) return;
      setError(reason === "preview_timeout" ? "试跑超时，请稍后重试。" : reason === "preview_cancelled"
        ? "试跑已取消或配置变化，请重新试跑。" : "试跑未完成，请检查语气和服务配置后重试。");
    } finally {
      if (requestRef.current === requestId) {
        requestRef.current = null;
        setPending(false);
      }
    }
  }

  return (
    <div className="mt-6 border-t border-border pt-4">
      <label className="block text-xs font-medium text-secondary">
        {t("语气试跑")}
        <textarea
          aria-label={t("试跑文本")}
          value={text}
          rows={3}
          maxLength={16_384}
          onChange={(event) => { cancel(); setPending(false); setResult(null); setError(null); setText(event.target.value); }}
          className={`mt-2 w-full resize-y rounded-lg border border-border bg-elevated px-3 py-2.5 text-sm font-normal leading-6 text-primary outline-none focus:border-accent ${focusRingClass}`}
        />
      </label>
      <p className="mt-2 text-xs leading-5 text-tertiary">{t("点击试跑后使用样例文字和 Prompt，可能调用当前整理服务；不会读取历史、保存结果或粘贴到其他 App。")}</p>
      {compareSaved && <p className="mt-1 text-xs text-secondary">{t("将使用同一段文字分别试跑已保存语气和当前草稿。")}</p>}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <button ref={runButtonRef} type="button" aria-busy={pending} aria-describedby={unavailableReason ? reasonId : undefined} onClick={() => void run()} disabled={pending || Boolean(unavailableReason)} className={`${secondaryButtonClass} min-w-36`}>
          {pending ? t("正在试跑…") : compareSaved ? t("试跑并对比") : t("试跑当前语气")}
        </button>
        {pending && <button type="button" className={ghostButtonClass} onClick={() => { restoreRunFocusRef.current = true; cancel(); setPending(false); setError("试跑已取消，迟到结果不会作为成功。"); }}>{t("取消")}</button>}
        <span className="text-xs text-secondary">{t(isDraft ? "使用当前草稿" : "使用已保存配置")}</span>
      </div>
      {unavailableReason && <p id={reasonId} className="mt-2 text-xs leading-5 text-secondary">{unavailableReason}</p>}
      <div aria-live="polite" aria-busy={pending}>
        {error && <p role="alert" className="mt-3 text-xs text-error-ink">{t(error)}</p>}
        {result && <div className={`mt-4 grid gap-3 ${result.saved ? "sm:grid-cols-2" : ""}`}>
          {result.saved && <Result result={result.saved} label={t("已保存语气")} />}
          <Result result={result.draft} label={t(isDraft ? "当前草稿" : "已保存语气")} />
        </div>}
      </div>
    </div>
  );
}

function Result({ result, label }: { result: TrialResult; label: string }) {
  const { t } = useI18n();
  const hasTransformations = result.cleanup_transformations?.length;
  const nativeFailure = result.provider_error?.native_failure;
  return <div className="min-w-0 rounded-lg border border-border bg-elevated/60 p-3">
    <p className="text-xs font-medium text-primary">{label}</p>
    <p className={`mt-1 text-xs ${result.status === "model" ? "text-tertiary" : "text-warning-ink"}`}>{t(outcomeLabel(result))} · {(result.elapsed_ms / 1000).toFixed(1)}s</p>
    {result.provider_error?.kind === "guard" && <p className="mt-1 text-xs text-warning-ink">{t("保真检查原因：")} {t(cleanupGuardReasonLabel(result.provider_error.guard))}</p>}
    {nativeFailure && <p className="mt-1 text-xs text-warning-ink">
      {t(nativeFailureStatusLabel(nativeFailure))}
      {typeof nativeFailure.input_utf8_bytes === "number" && typeof nativeFailure.max_input_bytes === "number" && <span> · {nativeFailure.input_utf8_bytes} / {nativeFailure.max_input_bytes} {t("UTF-8 字节")}</span>}
      {nativeFailure.stage && <span> · {t(nativeFailure.stage === "input_admission" ? "输入准入" : nativeFailure.stage === "model_admission" ? "模型准入" : "地址准入")}</span>}
      {nativeFailure.dispatched === false && <span> · {t("已确认请求未发送")}</span>}
      {nativeFailure.dispatched === true && <span> · {t("已确认请求已发送")}</span>}
      {nativeFailure.dispatched === null && <span> · {t("请求发送状态未确认")}</span>}
    </p>}
    <p className="mt-3 whitespace-pre-wrap break-words text-sm leading-6 text-primary">{result.text}</p>
    {result.cleanup_rejection && <details className="mt-2 text-xs text-secondary"><summary className="cursor-pointer">{t(cleanupCandidateLabel(result.cleanup_rejection.stage))}</summary><p className="mt-2 text-warning-ink">{t("保真检查原因：")}{t(cleanupGuardReasonLabel(result.cleanup_rejection.reason))}</p><p className="mt-2 whitespace-pre-wrap break-words">{result.cleanup_rejection.candidate}</p></details>}
    {typeof result.provider_candidate === "string" && <details className="mt-2 text-xs text-secondary"><summary className="cursor-pointer">{t("解析后的服务商候选（共享整理前）")}</summary><p className="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap break-words">{result.provider_candidate}</p></details>}
    {typeof result.model_candidate === "string" && <details className="mt-2 text-xs text-secondary"><summary className="cursor-pointer">{t("共享整理器接受稿（最终保真检查前）")}</summary><p className="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap break-words">{result.model_candidate}</p></details>}
    {hasTransformations ? <details className="mt-2 text-xs text-secondary"><summary className="cursor-pointer">{t("共享整理转换")}</summary><ul className="mt-2 list-disc space-y-1 pl-5">{result.cleanup_transformations?.map((transformation, index) => <li key={`${transformation}-${index}`}>{cleanupTransformationLabel(transformation, t)}</li>)}</ul></details> : null}
  </div>;
}
