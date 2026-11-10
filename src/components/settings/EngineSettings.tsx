import { useSessionDraftState } from "../../lib/sessionDraft";
import { Autocomplete } from "../Autocomplete";
import { Select } from "../Select";
import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, Copy, KeyRound, RefreshCw } from "lucide-react";
import { ConfirmDialog } from "../ConfirmDialog";
import { PasswordInput } from "../PasswordInput";
import { SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { useI18n } from "../../lib/i18n";
import {
  asrLanguageDescription,
  draftFromSettings,
  customOriginChanged,
  customHostChanged,
  hasSupportedAsrLanguage,
  hasProviderSecret,
  persistPatch,
  probePayload,
  providerConfigured,
  providerHint,
  providerOf,
  step2Ready,
  switchProvider,
  type EngineDraft,
} from "../../lib/engineWizard";
import { parseCustomCleanupParameters } from "../../lib/customCleanupParameters";
import {
  PROVIDERS,
  asrModelProfile,
  capabilityLabel,
  defaultModel,
  isLoopbackUrl,
  isProviderId,
  isRetiredOnDeviceModel,
  onDeviceMemoryCompatibilityCopy,
  providerById,
  providersFor,
  type ModelOption,
  type ProviderId,
} from "../../lib/providers";
import { friendlySettingsError } from "../../lib/settingsError";
import { buttonClass, ghostButtonClass, compactButtonClass, compactDangerButtonClass, colors, focusRingClass, radius } from "../../lib/theme";
import type { CleanupModelStatus, LocalCleanupModelStatus, LocalCleanupStatus, MlxCleanupModelStatus, OnDeviceModelStatus, ProviderCredentialPurpose, ProviderCredentialReadResult, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;
const LEGACY_MLX_CLEANUP_MODEL_ID = "qwen3_8_27b_mlx_6bit";

type ProbeStage = {
  ok: boolean;
  skipped: boolean;
  error_kind?: string | null;
  message?: string;
};

type ProbeResult = {
  asr: ProbeStage;
  cleanup: ProbeStage;
};

export type EnginePage = "service" | "api_key" | "local_models" | "advanced";

const ENGINE_PAGES: ReadonlyArray<{ id: EnginePage; label: string; description: string }> = [
  { id: "service", label: "服务配置", description: "配置转写和整理路线；更改在保存并使用后生效。" },
  { id: "api_key", label: "API Key", description: "按服务商管理凭据；密钥保存在这台 Mac 的钥匙串中。" },
  { id: "local_models", label: "本地模型", description: "下载后，在「服务配置」中选择模型并保存。下载需要联网，模型在这台 Mac 上运行。" },
  { id: "advanced", label: "高级设置", description: "管理严格离线策略、连接地址和高级请求参数。" },
];

type LatencySummary = {
  sample_count: number;
  p50_ms: number | null;
  p95_ms: number | null;
};

type DiagnosticNamedCount = {
  name: string;
  count: number;
};

type DiagnosticDelivery = {
  paste_sent: number;
  readback_confirmed: number;
  paste_unconfirmed: number;
  paste_failed: number;
  paste_cancelled: number;
  copied: number;
  history_only: number;
  preview_only: number;
  failed: number;
  cancelled: number;
};

type DiagnosticReturnedUsage = {
  unit: string;
  amount: number;
};

type DiagnosticAsrUsage = {
  request_count: number;
  failed_request_count: number;
  submitted_audio_seconds: number;
  audio_duration_request_count: number;
  returned_usage: DiagnosticReturnedUsage[];
};

type DiagnosticGroup = {
  provider: string;
  model: string;
  path: string;
  prefetch_asr: LatencySummary;
  final_asr: LatencySummary;
  cleanup: LatencySummary;
  validation: LatencySummary;
  paste_submission: LatencySummary;
  readback_confirmation: LatencySummary;
  history_journal: LatencySummary;
  capture_spool_flush: LatencySummary;
  retained_history: LatencySummary;
  stop_to_insert: LatencySummary;
  delivery: DiagnosticDelivery;
  asr_usage: DiagnosticAsrUsage;
  fallback_reasons: DiagnosticNamedCount[];
  error_reasons: DiagnosticNamedCount[];
};

type LatencyMetrics = {
  prefetch_asr: LatencySummary;
  final_asr: LatencySummary;
  asr: LatencySummary;
  cleanup: LatencySummary;
  paste: LatencySummary;
  history_journal: LatencySummary;
  capture_spool_flush: LatencySummary;
  retained_history: LatencySummary;
  stop_to_insert: LatencySummary;
  cleanup_guard_fallbacks: number;
  groups: DiagnosticGroup[];
};

const deliveryFields = [
  ["paste_sent", "已发送粘贴"],
  ["readback_confirmed", "读回确认"],
  ["paste_unconfirmed", "粘贴未确认"],
  ["paste_failed", "粘贴失败"],
  ["paste_cancelled", "粘贴已取消"],
  ["copied", "已复制"],
  ["history_only", "仅保存历史"],
  ["preview_only", "仅预览"],
  ["failed", "失败"],
  ["cancelled", "已取消"],
] as const satisfies ReadonlyArray<readonly [keyof DiagnosticDelivery, string]>;

const groupLatencyFields = [
  ["prefetch_asr", "预识别"],
  ["final_asr", "最终转写"],
  ["cleanup", "文字整理"],
  ["validation", "验证"],
  ["paste_submission", "粘贴提交"],
  ["readback_confirmation", "读回确认"],
  ["history_journal", "History 预交付日志"],
  ["capture_spool_flush", "录音恢复尾部确认"],
  ["retained_history", "交付后保留写入"],
  ["stop_to_insert", "停止到插入"],
] as const satisfies ReadonlyArray<readonly [keyof Pick<DiagnosticGroup, "prefetch_asr" | "final_asr" | "cleanup" | "validation" | "paste_submission" | "readback_confirmation" | "history_journal" | "capture_spool_flush" | "retained_history" | "stop_to_insert">, string]>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function diagnosticCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? Math.floor(value) : 0;
}

function diagnosticLabel(value: unknown): string {
  if (typeof value !== "string") return "unknown";
  const label = value.replace(/[^A-Za-z0-9._:/@-]/g, "").slice(0, 64);
  return label || "unknown";
}

function formatModelBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(exponent === 0 ? 0 : 1)} ${units[exponent]}`;
}

function onDeviceStateCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("模型文件已下载并校验");
    case "downloading": return t("模型文件正在下载");
    case "corrupt": return t("模型文件校验失败，可重新下载");
    case "missing": return t("模型文件未下载");
    default: return `${t("模型文件状态")}：${state}`;
  }
}

function onDeviceFileSummaryCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("已下载");
    case "downloading": return t("下载中");
    case "corrupt": return t("文件校验失败");
    case "missing": return t("未下载");
    default: return `${t("文件状态")}: ${state}`;
  }
}

function onDeviceRuntimeCopy(status: string, t: (key: string) => string): string {
  switch (status) {
    case "not_checked": return t("本机运行时尚未检查");
    case "unsupported_platform": return t("需要 Apple Silicon 与 macOS 14 或更新版本");
    case "sidecar_missing": return t("本机推理组件未安装");
    case "runtime_ready": return t("本机运行时握手通过，模型尚未加载");
    case "loading": return t("本机模型正在加载");
    case "loaded": return t("模型已加载");
    case "runtime_failed": return t("本机运行时启动失败");
    case "legacy_no_runtime": return t("旧版模型文件没有 MLX 推理支持");
    default: return `${t("本机运行时状态")}：${status}`;
  }
}

function cleanupModelStoreStateCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("模型文件已校验");
    case "installed": return t("模型已安装；首次加载会校验全部文件");
    case "missing": return t("尚未安装模型文件");
    case "partial": return t("下载尚未完成；可以继续下载或删除部分文件。");
    case "downloading": return t("正在下载本机模型");
    case "importing": return t("正在导入并校验本机模型");
    case "deleting": return t("正在删除本机模型");
    case "cancelled": return t("已取消");
    case "invalid": return t("模型清单或文件校验失败");
    case "error": return t("本机模型操作失败");
    default: return t("正在读取模型状态");
  }
}

function cleanupModelStoreSummaryCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("已安装");
    case "installed": return t("已安装");
    case "missing": return t("未下载");
    case "partial": return t("下载未完成");
    case "downloading": return t("下载中");
    case "importing": return t("导入中");
    case "deleting": return t("删除中");
    case "cancelled": return t("已取消");
    case "invalid": return t("文件校验失败");
    case "error": return t("操作失败");
    default: return t("状态未知");
  }
}

function localCleanupModelStateCopy(state: string, verified: boolean, t: (key: string) => string, partialManaged = false): string {
  switch (state) {
    case "missing": return partialManaged ? t("下载未完成") : t("尚未下载");
    case "downloading": return t("正在下载");
    case "installed": return verified ? t("已下载并校验") : t("文件尚未校验");
    case "corrupt": return t("文件校验失败");
    case "deleting": return t("正在删除");
    default: return `${t("文件状态")}: ${state}`;
  }
}

function mlxCleanupModelStateCopy(model: MlxCleanupModelStatus, t: (key: string) => string): string {
  if (model.operation === "verify") return t("正在校验模型文件");
  switch (model.state) {
    case "missing": return model.partial_managed ? t("下载未完成") : t("尚未下载");
    case "downloading": return t("正在下载");
    case "canceling": return t("正在取消下载");
    case "installed": return model.verified ? t("已下载并校验") : t("文件尚未校验");
    case "ready": return model.verified ? t("已下载并校验") : t("已安装；首次使用时会重新校验");
    case "corrupt": return t("文件校验失败");
    case "deleting": return t("正在删除");
    case "error": return t("操作失败");
    default: return `${t("文件状态")}: ${model.state}`;
  }
}

function mlxCleanupCompatibilityCopy(code: string | null | undefined, t: (key: string) => string): string {
  switch (code) {
    case "runtime_missing": return t("本机推理组件未安装");
    case "unsupported_platform": return t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。");
    case "model_not_verified": return t("模型文件尚未通过校验。");
    case "uncalibrated_memory": return t("此模型的最低内存档尚未完成验证。");
    case "insufficient_memory": return t("当前可用内存不足以运行此模型。");
    default: return code ? `${t("兼容性检查未通过")} (${code})` : t("当前设备不满足此模型的兼容条件。");
  }
}

function mlxCleanupLanguageCopy(language: string): string | null {
  return language === "Multilingual; product language coverage pending verification" ? null : language;
}

function isMlxCleanupModelOperationActive(model: MlxCleanupModelStatus): boolean {
  return Boolean(model.operation) || ["downloading", "canceling", "deleting"].includes(model.state);
}

function isCancellationError(message: string | null | undefined): boolean {
  return /cancel/i.test(message ?? "");
}

function localCleanupCompatibilityCopy(code: string | null | undefined, t: (key: string) => string): string {
  switch (code) {
    case "runtime_missing": return t("本机 llama.cpp 运行时不可用；请重新安装 VoiceFlow。");
    case "unsupported_platform": return t("当前设备或 macOS 版本不支持此模型。");
    case "model_not_verified": return t("模型文件尚未通过校验。");
    case "uncalibrated_memory": return t("此模型的最低内存档尚未完成验证。");
    case "insufficient_memory": return t("当前可用内存不足以运行此模型。");
    default: return code ? `${t("兼容性检查未通过")} (${code})` : t("当前设备不满足此模型的兼容条件。");
  }
}

function isReadyLocalCleanupModel(model: LocalCleanupModelStatus | undefined): boolean {
  return Boolean(model
    && model.state === "installed"
    && model.managed
    && model.verified
    && model.compatible
    && !model.operation);
}

function isSelectableLocalCleanupModel(model: LocalCleanupModelStatus | undefined): boolean {
  return model?.selectable === true && isReadyLocalCleanupModel(model);
}

function normalizeLatencySummary(value: unknown): LatencySummary {
  const summary = isRecord(value) ? value : {};
  const optionalMs = (candidate: unknown) => (
    typeof candidate === "number" && Number.isFinite(candidate) && candidate >= 0
      ? Math.floor(candidate)
      : null
  );
  return {
    sample_count: diagnosticCount(summary.sample_count),
    p50_ms: optionalMs(summary.p50_ms),
    p95_ms: optionalMs(summary.p95_ms),
  };
}

function normalizeNamedCounts(value: unknown): DiagnosticNamedCount[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 32).map((entry) => {
    const item = isRecord(entry) ? entry : {};
    return { name: diagnosticLabel(item.name), count: diagnosticCount(item.count) };
  });
}

function normalizeAsrUsage(value: unknown): DiagnosticAsrUsage {
  const usage = isRecord(value) ? value : {};
  const amount = (candidate: unknown) => (
    typeof candidate === "number" && Number.isFinite(candidate) && candidate >= 0
      ? Math.min(candidate, 1_000_000_000_000)
      : 0
  );
  const returnedUsage = Array.isArray(usage.returned_usage)
    ? usage.returned_usage.slice(0, 8).flatMap((entry) => {
        if (!isRecord(entry) || typeof entry.unit !== "string") return [];
        const unit = diagnosticLabel(entry.unit);
        const value = amount(entry.amount);
        return unit === "unknown" ? [] : [{ unit, amount: value }];
      })
    : [];
  return {
    request_count: diagnosticCount(usage.request_count),
    failed_request_count: diagnosticCount(usage.failed_request_count),
    submitted_audio_seconds: amount(usage.submitted_audio_seconds),
    audio_duration_request_count: diagnosticCount(usage.audio_duration_request_count),
    returned_usage: returnedUsage,
  };
}

function normalizeLatencyMetrics(value: unknown): LatencyMetrics {
  const source = isRecord(value) ? value : {};
  const normalizeDelivery = (value: unknown): DiagnosticDelivery => {
    const delivery = isRecord(value) ? value : {};
    return {
      paste_sent: diagnosticCount(delivery.paste_sent),
      readback_confirmed: diagnosticCount(delivery.readback_confirmed),
      paste_unconfirmed: diagnosticCount(delivery.paste_unconfirmed),
      paste_failed: diagnosticCount(delivery.paste_failed),
      paste_cancelled: diagnosticCount(delivery.paste_cancelled),
      copied: diagnosticCount(delivery.copied),
      history_only: diagnosticCount(delivery.history_only),
      preview_only: diagnosticCount(delivery.preview_only),
      failed: diagnosticCount(delivery.failed),
      cancelled: diagnosticCount(delivery.cancelled),
    };
  };
  const normalizeGroup = (value: unknown): DiagnosticGroup => {
    const group = isRecord(value) ? value : {};
    return {
      provider: diagnosticLabel(group.provider),
      model: diagnosticLabel(group.model),
      path: diagnosticLabel(group.path),
      prefetch_asr: normalizeLatencySummary(group.prefetch_asr),
      final_asr: normalizeLatencySummary(group.final_asr),
      cleanup: normalizeLatencySummary(group.cleanup),
      validation: normalizeLatencySummary(group.validation),
      paste_submission: normalizeLatencySummary(group.paste_submission),
      readback_confirmation: normalizeLatencySummary(group.readback_confirmation),
      history_journal: normalizeLatencySummary(group.history_journal),
      capture_spool_flush: normalizeLatencySummary(group.capture_spool_flush),
      retained_history: normalizeLatencySummary(group.retained_history),
      stop_to_insert: normalizeLatencySummary(group.stop_to_insert),
      delivery: normalizeDelivery(group.delivery),
      asr_usage: normalizeAsrUsage(group.asr_usage),
      fallback_reasons: normalizeNamedCounts(group.fallback_reasons),
      error_reasons: normalizeNamedCounts(group.error_reasons),
    };
  };
  return {
    prefetch_asr: normalizeLatencySummary(source.prefetch_asr),
    final_asr: normalizeLatencySummary(source.final_asr),
    asr: normalizeLatencySummary(source.asr),
    cleanup: normalizeLatencySummary(source.cleanup),
    paste: normalizeLatencySummary(source.paste),
    history_journal: normalizeLatencySummary(source.history_journal),
    capture_spool_flush: normalizeLatencySummary(source.capture_spool_flush),
    retained_history: normalizeLatencySummary(source.retained_history),
    stop_to_insert: normalizeLatencySummary(source.stop_to_insert),
    cleanup_guard_fallbacks: diagnosticCount(source.cleanup_guard_fallbacks),
    groups: Array.isArray(source.groups) ? source.groups.slice(0, 32).map(normalizeGroup) : [],
  };
}

function formatLatency(value: number | null, t: (key: string) => string): string {
  return value === null ? t("无") : `${value} ${t("毫秒")}`;
}

function LatencyMetricsViewer({ onErrorChange }: { onErrorChange: (error: string | null) => void }) {
  const { t } = useI18n();
  const [metrics, setMetrics] = useState<LatencyMetrics | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (copyTimer.current) clearTimeout(copyTimer.current); }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(false);
    setCopied(false);
    try {
      const result = await invoke<unknown>("get_latency_metrics");
      setMetrics(normalizeLatencyMetrics(result));
    } catch {
      setError(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    onErrorChange(error ? t("诊断数据未能读取或复制，请重试。") : null);
  }, [error, onErrorChange, t]);

  const copyJson = async () => {
    if (!metrics) return;
    setCopied(false);
    try {
      await navigator.clipboard.writeText(JSON.stringify(metrics, null, 2));
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 2000);
    } catch {
      setError(true);
    }
  };

  const summaryFields = metrics ? [
    ["prefetch_asr", "预识别", metrics.prefetch_asr],
    ["final_asr", "最终转写", metrics.final_asr],
    ["asr", "转写（兼容）", metrics.asr],
    ["cleanup", "文字整理", metrics.cleanup],
    ["paste", "粘贴", metrics.paste],
    ["history_journal", "History 预交付日志", metrics.history_journal],
    ["capture_spool_flush", "录音恢复尾部确认", metrics.capture_spool_flush],
    ["retained_history", "交付后保留写入", metrics.retained_history],
    ["stop_to_insert", "停止到插入", metrics.stop_to_insert],
  ] as const : [];

  return (
    <div className="vf-diagnostics pt-4">
      <p className="text-xs leading-5 text-secondary">{t("查看本机听写阶段延迟和交付计数。仅包含受限的服务商、模型与路径标签，不包含文字、音频或上下文。")}</p>
      <div className="flex flex-wrap items-center justify-between gap-3 py-4">
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" onClick={() => void refresh()} disabled={loading} className={compactButtonClass}>
            <RefreshCw size={14} className={loading ? "animate-spin motion-reduce:animate-none" : ""} aria-hidden="true" />
            {loading ? t("正在刷新…") : t("刷新")}
          </button>
          <button type="button" onClick={() => void copyJson()} disabled={!metrics || loading} className={compactButtonClass}>
            <Copy size={14} aria-hidden="true" />
            {copied ? t("已复制") : t("复制 JSON")}
          </button>
        </div>
        {error && <p role="alert" className="text-xs text-error-ink">{t("本机诊断操作失败，请重试。")}</p>}
      </div>
      {loading && !metrics && <p role="status" className="pb-4 text-xs text-tertiary">{t("正在读取本机诊断…")}</p>}
      {metrics && (
        <>
          <div className="grid gap-2 pb-4 sm:grid-cols-2 lg:grid-cols-3">
            {summaryFields.map(([key, label, summary]) => (
              <div key={key} className="rounded-lg bg-elevated px-3 py-2.5">
                <p className="text-xs font-medium text-secondary">{t(label)}</p>
                <p className="mt-1 text-xs text-tertiary">
                  {t("样本")} {summary.sample_count} · P50 {formatLatency(summary.p50_ms, t)} · P95 {formatLatency(summary.p95_ms, t)}
                </p>
              </div>
            ))}
            <div className="rounded-lg bg-elevated px-3 py-2.5">
              <p className="text-xs font-medium text-secondary">{t("整理保护回退")}</p>
              <p className="mt-1 text-xs text-tertiary">{metrics.cleanup_guard_fallbacks}</p>
            </div>
          </div>
          <div className="border-t border-border pt-4">
            <p className="text-xs font-medium text-secondary">{t("分组摘要")}</p>
            {metrics.groups.length === 0 ? (
              <p className="mt-2 text-xs text-tertiary">{t("尚无分组记录")}</p>
            ) : (
              <div className="mt-2 space-y-3">
                {metrics.groups.map((group, index) => (
                  <section key={`${group.provider}-${group.model}-${group.path}-${index}`} className="rounded-xl border border-border bg-card/35 p-3">
                    <p className="break-all text-xs font-medium text-primary">{group.provider} · {group.model}</p>
                    <p className="mt-0.5 break-all text-xs text-tertiary">{t("路径")}: {group.path}</p>
                    <div className="mt-3 space-y-1.5">
                      {groupLatencyFields.map(([field, label]) => {
                        const summary = group[field];
                        return (
                          <div key={field} className="grid grid-cols-[minmax(5rem,0.8fr)_minmax(0,2fr)] gap-2 text-xs sm:grid-cols-[minmax(7rem,1fr)_minmax(0,3fr)]">
                            <span className="text-tertiary">{t(label)}</span>
                            <span className="text-secondary">{t("样本")} {summary.sample_count} · P50 {formatLatency(summary.p50_ms, t)} · P95 {formatLatency(summary.p95_ms, t)}</span>
                          </div>
                        );
                      })}
                    </div>
                    <div className="mt-3 border-t border-border pt-2">
                      <p className="text-xs font-medium text-tertiary">{t("ASR 用量")}</p>
                      <div className="mt-1 flex flex-wrap gap-1.5">
                        <span className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">{t("请求")}: {group.asr_usage.request_count}</span>
                        <span className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">{t("失败请求")}: {group.asr_usage.failed_request_count}</span>
                        <span className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">
                          {t("提交音频")}: {group.asr_usage.submitted_audio_seconds.toLocaleString(undefined, { maximumFractionDigits: 1 })} {t("秒")}
                        </span>
                        <span className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">{t("有时长的请求")}: {group.asr_usage.audio_duration_request_count}</span>
                      </div>
                      <p className="mt-2 text-xs font-medium text-tertiary">{t("服务商返回用量")}</p>
                      <div className="mt-1 flex flex-wrap gap-1.5">
                        {group.asr_usage.returned_usage.length > 0
                          ? group.asr_usage.returned_usage.map((item, usageIndex) => (
                              <span key={`${item.unit}-${usageIndex}`} className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">
                                {item.unit}: {item.amount.toLocaleString(undefined, { maximumFractionDigits: 4 })}
                              </span>
                            ))
                          : <span className="text-xs text-tertiary">{t("无")}</span>}
                      </div>
                    </div>
                    <div className="mt-3 border-t border-border pt-2">
                      <p className="text-xs font-medium text-tertiary">{t("交付结果")}</p>
                      <div className="mt-1 flex flex-wrap gap-1.5">
                        {deliveryFields.filter(([field]) => group.delivery[field] > 0).map(([field, label]) => (
                          <span key={field} className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">{t(label)}: {group.delivery[field]}</span>
                        ))}
                        {deliveryFields.every(([field]) => group.delivery[field] === 0) && <span className="text-xs text-tertiary">{t("无")}</span>}
                      </div>
                    </div>
                    <DiagnosticReasonCounts title={t("回退原因")} counts={group.fallback_reasons} />
                    <DiagnosticReasonCounts title={t("错误原因")} counts={group.error_reasons} />
                  </section>
                ))}
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function DiagnosticReasonCounts({ title, counts }: { title: string; counts: DiagnosticNamedCount[] }) {
  if (counts.length === 0) return null;
  return (
    <div className="mt-2">
      <p className="text-xs font-medium text-tertiary">{title}</p>
      <div className="mt-1 flex flex-wrap gap-1.5">
        {counts.map((item, index) => (
          <span key={`${item.name}-${index}`} className="rounded-md bg-elevated px-2 py-1 text-xs text-secondary">{item.name}: {item.count}</span>
        ))}
      </div>
    </div>
  );
}

function probeCopy(kind: string | null | undefined, message: string | undefined, t: (key: string) => string): string {
  if (kind === "address") return t("地址连不上");
  if (kind === "key") return t("密钥无效");
  if (kind === "model") return t("模型名不被这个接口接受");
  if (kind === "path") return t("请检查接口地址和模型 ID");
  if (kind === "missing_key") return t("缺少密钥");
  if (kind === "provider") return t("服务返回错误");
  if (kind === "on_device_model_missing") return t("本地模型未就绪");
  if (kind === "on_device_inference_unavailable") return t("本机推理不可用；请检查运行时或模型状态后重试");
  return message?.trim() || t("服务返回错误");
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${stableJson(object[key])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}

type ProviderKeyRevisions = Partial<Record<ProviderId, number>>;

function asrDraftIdentity(draft: EngineDraft, keyRevisions: ProviderKeyRevisions): string {
  return JSON.stringify([
    draft.asrProvider,
    draft.asrModel.trim(),
    draft.asrProvider === "custom" ? draft.customBaseUrl.trim() : null,
    draft.asrProvider === "local_whisper" ? draft.localWhisperBaseUrl.trim() : null,
    draft.asrProvider === "dashscope" ? draft.dashscopeRegion : null,
    keyRevisions[draft.asrProvider] ?? 0,
  ]);
}

function cleanupDraftIdentity(
  draft: EngineDraft,
  enabled: boolean,
  keyRevisions: ProviderKeyRevisions,
): string {
  return JSON.stringify([
    enabled,
    draft.cleanupProvider,
    draft.cleanupModel.trim(),
    draft.cleanupProvider === "custom" ? draft.customBaseUrl.trim() : null,
    draft.cleanupProvider === "custom" ? draft.customCleanupParametersText : null,
    draft.cleanupProvider === "ollama" ? draft.ollamaBaseUrl.trim() : null,
    keyRevisions[draft.cleanupProvider] ?? 0,
  ]);
}

function capabilityText(id: ProviderId, t: (key: string) => string): string {
  const definition = providerById(id);
  const capabilities = id === "custom"
    ? ["asr" as const, "llm" as const]
    : definition?.capabilities ?? [];
  const kind = capabilityLabel(capabilities);
  if (kind === "both") return `${t("转写")} · ${t("润色")}`;
  if (kind === "asr") return t("转写");
  return t("润色");
}

function providerHasCredentialInput(id: ProviderId): boolean {
  return id !== "on_device" && id !== "on_device_mlx" && id !== "on_device_llama_cpp";
}

function keepEngineControlInsideRails(target: HTMLElement, alignFirst = false) {
  if (!target.closest(".vf-engine-settings") || target.closest(".vf-engine-footer")
    || target.closest("[data-radix-popper-content-wrapper]")) return;
  if (alignFirst) target.scrollIntoView?.({ block: "nearest" });
  const scrollPane = target.closest<HTMLElement>(".vf-settings-scroll-pane");
  if (!scrollPane) return;
  const paneBounds = scrollPane.getBoundingClientRect();
  const footerTop = document.querySelector<HTMLElement>(".vf-engine-footer")?.getBoundingClientRect().top;
  const bounds = target.getBoundingClientRect();
  const visibleTop = paneBounds.top + 8;
  const visibleBottom = (footerTop ?? paneBounds.bottom) - 8;
  if (bounds.bottom > visibleBottom) scrollPane.scrollTop += bounds.bottom - visibleBottom;
  else if (bounds.top < visibleTop) scrollPane.scrollTop -= visibleTop - bounds.top;
}

export function EngineSettings({
  settings,
  page,
  onPageChange,
  removeProviderKey,
  commitEngine,
  readProviderCredential,
  cancelProviderCredentialRead,
}: {
  settings: Settings;
  page: EnginePage;
  onPageChange: (page: EnginePage) => void;
  removeProviderKey: (provider: string) => Promise<void>;
  commitEngine: (patch: Record<string, unknown>) => Promise<void>;
  readProviderCredential?: (provider: string, requestId: string, purpose?: ProviderCredentialPurpose) => Promise<ProviderCredentialReadResult>;
  cancelProviderCredentialRead?: (requestId: string) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraftState] = useSessionDraftState<EngineDraft>("engine:draft", () => draftFromSettings(settings));
  const [draftCleanupEnabled, setDraftCleanupEnabledState] = useSessionDraftState("engine:draftCleanupEnabled", settings.cleanup_enabled);
  const [draftRevision, setDraftRevision] = useState(0);
  const [asrDraftRevision, setAsrDraftRevision] = useState(0);
  const [cleanupDraftRevision, setCleanupDraftRevision] = useState(0);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const cleanupEnabledRef = useRef(draftCleanupEnabled);
  cleanupEnabledRef.current = draftCleanupEnabled;
  const providerKeyRevisions = useRef<ProviderKeyRevisions>({});
  const setDraftCleanupEnabled = useCallback<Dispatch<SetStateAction<boolean>>>((action) => {
    const previous = cleanupEnabledRef.current;
    const next = typeof action === "function" ? action(previous) : action;
    if (previous !== next) setCleanupDraftRevision((revision) => revision + 1);
    cleanupEnabledRef.current = next;
    setDraftRevision((revision) => revision + 1);
    setDraftCleanupEnabledState(next);
  }, [setDraftCleanupEnabledState]);
  const setDraft = useCallback<Dispatch<SetStateAction<EngineDraft>>>((action) => {
    const previous = draftRef.current;
    const next = typeof action === "function" ? (action as (current: EngineDraft) => EngineDraft)(previous) : action;
    const previousKeyRevisions = providerKeyRevisions.current;
    const nextKeyRevisions: ProviderKeyRevisions = { ...previousKeyRevisions };
    for (const provider of PROVIDERS) {
      if ((previous.providerKeys[provider.id] ?? "") === (next.providerKeys[provider.id] ?? "")) continue;
      nextKeyRevisions[provider.id] = (nextKeyRevisions[provider.id] ?? 0) + 1;
    }
    if (asrDraftIdentity(previous, previousKeyRevisions) !== asrDraftIdentity(next, nextKeyRevisions)) {
      setAsrDraftRevision((revision) => revision + 1);
    }
    if (cleanupDraftIdentity(previous, cleanupEnabledRef.current, previousKeyRevisions)
      !== cleanupDraftIdentity(next, cleanupEnabledRef.current, nextKeyRevisions)) {
      setCleanupDraftRevision((revision) => revision + 1);
    }
    draftRef.current = next;
    providerKeyRevisions.current = nextKeyRevisions;
    setDraftRevision((revision) => revision + 1);
    setDraftState(next);
  }, [setDraftState]);
  const [pendingPageFocus, setPendingPageFocus] = useState<string | null>(null);
  const [advancedConnectionsOpen, setAdvancedConnectionsOpen] = useState(false);
  const [metricsError, setMetricsError] = useState<string | null>(null);
  const draftSettings = { ...settings, cleanup_enabled: draftCleanupEnabled };
  const settingsObjectRef = useRef(settings);
  const settingsSnapshotRevisionRef = useRef(0);
  if (settingsObjectRef.current !== settings) {
    settingsObjectRef.current = settings;
    settingsSnapshotRevisionRef.current += 1;
  }
  const savedCredentialRevision = (provider: ProviderId) => JSON.stringify([
    settings.provider_keys?.[provider]?.configured,
    settings.provider_keys?.[provider]?.hint,
    settings.provider_keys?.[provider]?.keychain_state,
    provider === "groq" ? settings.api_key_configured : undefined,
    provider === "custom" ? settings.asr_api_key_configured : undefined,
    provider === "custom" ? settings.cleanup_api_key_configured : undefined,
  ]);
  const asrRoute = JSON.stringify([
    asrDraftIdentity(draft, providerKeyRevisions.current),
    asrDraftRevision,
    savedCredentialRevision(draft.asrProvider),
  ]);
  const cleanupRoute = JSON.stringify([
    cleanupDraftIdentity(draft, draftCleanupEnabled, providerKeyRevisions.current),
    cleanupDraftRevision,
    savedCredentialRevision(draft.cleanupProvider),
  ]);
  const probeConfigurationRevision = JSON.stringify([
    asrRoute,
    cleanupRoute,
    draftCleanupEnabled,
    draftRevision,
    settingsSnapshotRevisionRef.current,
    settings.asr_provider,
    settings.asr_model,
    settings.asr_base_url,
    settings.cleanup_provider,
    settings.cleanup_model,
    settings.cleanup_enabled,
    settings.custom_base_url,
    settings.cleanup_base_url,
    settings.strict_offline_enabled,
    ...PROVIDERS.flatMap((provider) => [
      provider.id,
      settings.provider_keys?.[provider.id]?.configured,
      settings.provider_keys?.[provider.id]?.keychain_state,
    ]),
  ]);
  const probeConfigurationRevisionRef = useRef(probeConfigurationRevision);
  probeConfigurationRevisionRef.current = probeConfigurationRevision;
  const customParametersParse = parseCustomCleanupParameters(draft.customCleanupParametersText);
  const [probing, setProbing] = useState(false);
  const [rowProbing, setRowProbing] = useState<ProviderId | null>(null);
  const [rowProbingStage, setRowProbingStage] = useState<"asr" | "cleanup" | null>(null);
  const [rowProbeFeedback, setRowProbeFeedback] = useState<{
    provider: ProviderId;
    stage?: "asr" | "cleanup";
    tone: "success" | "error";
    message: string;
  } | null>(null);
  const [probeSuccess, setProbeSuccess] = useState<"tested" | "saved" | "">("");
  const [testedCleanupRoute, setTestedCleanupRoute] = useState<string | null>(null);
  const [testedAsrRoute, setTestedAsrRoute] = useState<string | null>(null);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [stageFail, setStageFail] = useState<{ asr?: string | null; cleanup?: string | null }>({});
  const [credentialReadPending, setCredentialReadPending] = useState(false);
  const [credentialReadStatus, setCredentialReadStatus] = useState<ProviderCredentialReadResult["status"] | null>(null);
  const [credentialReadTarget, setCredentialReadTarget] = useState<"groq" | "custom" | null>(null);
  const [credentialReadMessage, setCredentialReadMessage] = useState<{ tone: "success" | "status" | "error"; text: string } | null>(null);
  const credentialReadRequest = useRef<string | null>(null);
  const cancelCredentialReadRef = useRef(cancelProviderCredentialRead);
  useEffect(() => {
    cancelCredentialReadRef.current = cancelProviderCredentialRead;
  }, [cancelProviderCredentialRead]);
  useEffect(() => () => {
    const requestId = credentialReadRequest.current;
    credentialReadRequest.current = null;
    const cancel = cancelCredentialReadRef.current;
    if (requestId && cancel) void cancel(requestId).catch(() => undefined);
  }, []);
  const credentialReadConfiguration = JSON.stringify([
    draft.asrProvider,
    draft.asrModel,
    draft.cleanupProvider,
    draft.cleanupModel,
    draft.customAsr,
    draft.customLlm,
    draft.dashscopeRegion,
    draftCleanupEnabled,
    draft.customBaseUrl,
    draft.customCleanupParametersText,
    draft.providerKeys.custom,
    draft.ollamaBaseUrl,
    draft.localWhisperBaseUrl,
    draft.providerKeys.groq,
    settings.asr_provider,
    settings.cleanup_provider,
    settings.cleanup_enabled,
    settings.custom_base_url,
    settings.asr_base_url,
    settings.cleanup_base_url,
  ]);
  const credentialReadConfigurationRef = useRef(credentialReadConfiguration);
  useEffect(() => {
    if (credentialReadConfigurationRef.current === credentialReadConfiguration) return;
    credentialReadConfigurationRef.current = credentialReadConfiguration;
    const requestId = credentialReadRequest.current;
    if (!requestId) return;
    credentialReadRequest.current = null;
    setCredentialReadPending(false);
    setCredentialReadStatus("stale");
    setCredentialReadMessage({ tone: "status", text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") });
    const cancel = cancelCredentialReadRef.current;
    if (cancel) void cancel(requestId).catch(() => undefined);
  }, [credentialReadConfiguration, t]);
  const [expanded, setExpanded] = useState<Set<ProviderId>>(() => new Set());
  const [confirmRemove, setConfirmRemove] = useState<ProviderId | null>(null);
  const [removing, setRemoving] = useState(false);
  const [onDeviceModels, setOnDeviceModels] = useState<OnDeviceModelStatus[]>([]);
  const [onDeviceModelsLoading, setOnDeviceModelsLoading] = useState(true);
  const [onDeviceModelReadError, setOnDeviceModelReadError] = useState<string | null>(null);
  const [onDeviceActionId, setOnDeviceActionId] = useState<string | null>(null);
  const [onDeviceActionError, setOnDeviceActionError] = useState<{ id: string; message: string } | null>(null);
  const [legacyModelsOpen, setLegacyModelsOpen] = useState(false);
  const [cleanupModelStoreStatus, setCleanupModelStoreStatus] = useState<CleanupModelStatus | null>(null);
  const [cleanupModelStoreBusy, setCleanupModelStoreBusy] = useState(false);
  const [cleanupModelStoreActionError, setCleanupModelStoreActionError] = useState<string | null>(null);
  const [cleanupModelStoreReadError, setCleanupModelStoreReadError] = useState<string | null>(null);
  const [confirmDeleteCleanupModel, setConfirmDeleteCleanupModel] = useState(false);
  const [localCleanupModels, setLocalCleanupModels] = useState<LocalCleanupModelStatus[]>([]);
  const [localCleanupModelsLoading, setLocalCleanupModelsLoading] = useState(true);
  const [localCleanupModelsReadError, setLocalCleanupModelsReadError] = useState<string | null>(null);
  const [localCleanupModelBusy, setLocalCleanupModelBusy] = useState<Record<string, string>>({});
  const [localCleanupModelActionErrors, setLocalCleanupModelActionErrors] = useState<Record<string, string>>({});
  const [confirmDeleteLocalCleanupModel, setConfirmDeleteLocalCleanupModel] = useState<string | null>(null);
  const [mlxCleanupModels, setMlxCleanupModels] = useState<MlxCleanupModelStatus[]>([]);
  const [mlxCleanupModelsLoading, setMlxCleanupModelsLoading] = useState(true);
  const [mlxCleanupModelsReadError, setMlxCleanupModelsReadError] = useState<string | null>(null);
  const [mlxCleanupModelBusy, setMlxCleanupModelBusy] = useState<Record<string, string>>({});
  const [mlxCleanupModelActionErrors, setMlxCleanupModelActionErrors] = useState<Record<string, string>>({});
  const [confirmDeleteMlxCleanupModel, setConfirmDeleteMlxCleanupModel] = useState<string | null>(null);
  const [confirmDeleteModel, setConfirmDeleteModel] = useState<string | null>(null);
  const [localCleanupStatus, setLocalCleanupStatus] = useState<LocalCleanupStatus | null>(null);
  const [localCleanupChecking, setLocalCleanupChecking] = useState(false);
  const [localCleanupError, setLocalCleanupError] = useState<string | null>(null);
  const [strictOfflineSaving, setStrictOfflineSaving] = useState(false);
  const [strictOfflineSaveError, setStrictOfflineSaveError] = useState<string | null>(null);
  const [savedDraftSnapshot, setSavedDraftSnapshot] = useSessionDraftState("engine:saved", () => ({ draft: draftFromSettings(settings), cleanup: settings.cleanup_enabled }));
  const previousSettings = useRef(savedDraftSnapshot);
  useEffect(() => {
    const previous = previousSettings.current;
    const next = { draft: draftFromSettings(settings), cleanup: settings.cleanup_enabled };
    const savedCustomOriginChanged = customOriginChanged(previous.draft.customBaseUrl, next.draft.customBaseUrl);
    previousSettings.current = next;
    setSavedDraftSnapshot(next);
    setDraft((current) => {
      if (JSON.stringify({ ...current, providerKeys: {} }) !== JSON.stringify(previous.draft)) return current;
      return {
        ...next.draft,
        providerKeys: {
          ...current.providerKeys,
          ...(savedCustomOriginChanged ? { custom: "" } : {}),
        },
      };
    });
    setDraftCleanupEnabled((current) => current === previous.cleanup ? settings.cleanup_enabled : current);
  }, [settings, setDraft, setDraftCleanupEnabled, setSavedDraftSnapshot]);

  const navigateToPage = useCallback((nextPage: EnginePage, targetId?: string) => {
    if (nextPage === "api_key") {
      const provider = targetId?.startsWith("provider-key-") ? targetId.slice("provider-key-".length) : null;
      if (provider && isProviderId(provider) && providerHasCredentialInput(provider)) {
        setExpanded((current) => new Set([...current, provider]));
      }
    }
    if (nextPage === "advanced" && targetId && targetId !== "strict-offline-toggle") setAdvancedConnectionsOpen(true);
    if (nextPage === "local_models" && targetId === "local-asr-model-sensevoice-small") setLegacyModelsOpen(true);
    setPendingPageFocus(targetId ?? null);
    onPageChange(nextPage);
  }, [onPageChange]);
  useEffect(() => {
    if (!pendingPageFocus) return;
    let frame = 0;
    frame = window.requestAnimationFrame(() => {
      const target = document.getElementById(pendingPageFocus);
      if (!target) {
        setPendingPageFocus(null);
        return;
      }
      keepEngineControlInsideRails(target, true);
      target.focus({ preventScroll: true });
      setPendingPageFocus(null);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [page, pendingPageFocus, advancedConnectionsOpen, expanded, legacyModelsOpen]);

  useEffect(() => {
    if (page === "api_key") return;
    const requestId = credentialReadRequest.current;
    if (!requestId) return;
    credentialReadRequest.current = null;
    setCredentialReadPending(false);
    setCredentialReadStatus("stale");
    setCredentialReadMessage({ tone: "status", text: t("离开 API Key 页面后，读取请求已取消；请重新发起读取。") });
    const cancel = cancelCredentialReadRef.current;
    if (cancel) void cancel(requestId).catch(() => undefined);
  }, [page, t]);

  useEffect(() => {
    let cancelled = false;
    const apply = (models: OnDeviceModelStatus[]) => {
      if (cancelled) return;
      setOnDeviceModels(Array.isArray(models) ? models : []);
      setOnDeviceModelReadError(null);
      setOnDeviceModelsLoading(false);
    };
    void Promise.resolve(invoke<OnDeviceModelStatus[]>("list_on_device_models"))
      .then(apply)
      .catch(() => {
        if (!cancelled) {
          setOnDeviceModels([]);
          setOnDeviceModelReadError(t("无法读取本机模型状态，请重试。"));
          setOnDeviceModelsLoading(false);
        }
      });
    const subscription = listen<OnDeviceModelStatus>("ondevice://download", (event) => {
      if (cancelled) return;
      setOnDeviceModels((current) => {
        const next = current.filter((model) => model.id !== event.payload.id);
        return [...next, event.payload];
      });
    });
    return () => {
      cancelled = true;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [t]);

  const onDeviceLoadInProgress = onDeviceModels.some((model) => model.runtime_status === "loading");

  useEffect(() => {
    if (!onDeviceLoadInProgress) return;
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const models = await invoke<OnDeviceModelStatus[]>("list_on_device_models");
        if (!cancelled) setOnDeviceModels(Array.isArray(models) ? models : []);
      } catch {
        // Keep the last known loading state and try again while it remains active.
      } finally {
        if (!cancelled) timer = window.setTimeout(() => void poll(), 1000);
      }
    };
    timer = window.setTimeout(() => void poll(), 1000);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [onDeviceLoadInProgress]);

  const selectedOnDeviceModel = onDeviceModels.find((model) => model.id === draft.asrModel);
  const onDeviceReady = selectedOnDeviceModel?.inference_ready === true;

  const refreshOnDeviceModels = async () => {
    setOnDeviceModelsLoading(true);
    setOnDeviceModelReadError(null);
    try {
      const models = await invoke<OnDeviceModelStatus[]>("list_on_device_models");
      setOnDeviceModels(Array.isArray(models) ? models : []);
      setOnDeviceModelReadError(null);
    } catch {
      setOnDeviceModelReadError(t("无法读取本机模型状态，请重试。"));
    } finally {
      setOnDeviceModelsLoading(false);
    }
  };

  const refreshCleanupModelStore = async () => {
    try {
      const status = await invoke<CleanupModelStatus>("get_cleanup_model_store_status");
      setCleanupModelStoreStatus(status);
      setCleanupModelStoreReadError(null);
    } catch {
      setCleanupModelStoreReadError(t("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。"));
    }
  };

  const refreshLocalCleanupModels = async (showLoading = false) => {
    if (showLoading) setLocalCleanupModelsLoading(true);
    try {
      const models = await invoke<LocalCleanupModelStatus[]>("list_local_cleanup_models");
      setLocalCleanupModels(Array.isArray(models) ? models : []);
      setLocalCleanupModelsReadError(null);
    } catch (reason) {
      setLocalCleanupModelsReadError(`${t("本机整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
    } finally {
      if (showLoading) setLocalCleanupModelsLoading(false);
    }
  };

  const runLocalCleanupModelAction = async (id: string, action: "download" | "cancel" | "delete") => {
    setLocalCleanupModelBusy((current) => ({ ...current, [id]: action }));
    setLocalCleanupModelActionErrors((current) => ({ ...current, [id]: "" }));
    try {
      if (action === "download") await invoke("download_local_cleanup_model", { id });
      else if (action === "cancel") {
        const cancelled = await invoke<boolean>("cancel_local_cleanup_model_download", { id });
        if (!cancelled) setLocalCleanupModelActionErrors((current) => ({
          ...current,
          [id]: t("下载已经结束或当前无法取消；模型状态已刷新。"),
        }));
      } else await invoke("delete_local_cleanup_model", { id });
    } catch (reason) {
      const rawMessage = typeof reason === "string"
        ? reason
        : reason instanceof Error
          ? reason.message
          : reason && typeof reason === "object" && "message" in reason && typeof (reason as { message: unknown }).message === "string"
            ? (reason as { message: string }).message
            : String(reason);
      const message = rawMessage === "local_cleanup_model_is_active"
        ? t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")
        : `${action === "download" ? t("模型下载失败") : action === "cancel" ? t("取消下载失败") : t("删除模型失败")}: ${rawMessage}`;
      setLocalCleanupModelActionErrors((current) => ({ ...current, [id]: message }));
    } finally {
      await refreshLocalCleanupModels();
      setLocalCleanupModelBusy((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
  };

  const refreshMlxCleanupModels = async (showLoading = false) => {
    if (showLoading) setMlxCleanupModelsLoading(true);
    try {
      const models = await invoke<MlxCleanupModelStatus[]>("list_mlx_cleanup_models");
      setMlxCleanupModels(Array.isArray(models) ? models : []);
      setMlxCleanupModelsReadError(null);
    } catch (reason) {
      setMlxCleanupModelsReadError(`${t("本机 MLX 整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
    } finally {
      if (showLoading) setMlxCleanupModelsLoading(false);
    }
  };

  const runMlxCleanupModelAction = async (id: string, action: "download" | "verify" | "cancel" | "delete") => {
    setMlxCleanupModelBusy((current) => ({ ...current, [id]: action }));
    setMlxCleanupModelActionErrors((current) => ({ ...current, [id]: "" }));
    try {
      if (action === "download") await invoke("download_mlx_cleanup_model", { id });
      else if (action === "verify") await invoke("verify_mlx_cleanup_model", { id });
      else if (action === "cancel") await invoke("cancel_mlx_cleanup_model_download", { id });
      else await invoke("delete_mlx_cleanup_model", { id });
    } catch (reason) {
      const rawMessage = typeof reason === "string"
        ? reason
        : reason instanceof Error
          ? reason.message
          : reason && typeof reason === "object" && "message" in reason && typeof (reason as { message: unknown }).message === "string"
            ? (reason as { message: string }).message
            : String(reason);
      const message = rawMessage === "local_cleanup_model_is_active"
        ? t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")
        : `${action === "download" ? t("MLX 整理模型下载失败") : action === "verify" ? t("验证 MLX 整理模型失败") : action === "cancel" ? t("取消 MLX 整理模型下载失败") : t("删除 MLX 整理模型失败")}: ${rawMessage}`;
      setMlxCleanupModelActionErrors((current) => ({ ...current, [id]: message }));
    } finally {
      await refreshMlxCleanupModels();
      setMlxCleanupModelBusy((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
  };

  const runCleanupModelStoreAction = async (action: "import" | "download" | "cancel" | "delete") => {
    setCleanupModelStoreBusy(true);
    setCleanupModelStoreActionError(null);
    try {
      if (action === "import") await invoke("import_cleanup_model");
      else if (action === "download") await invoke("download_cleanup_model");
      else if (action === "cancel") await invoke("cancel_cleanup_model_store_operation");
      else await invoke("delete_cleanup_model_store");
    } catch {
      setCleanupModelStoreActionError(t("本机 MLX 模型操作失败，请重试。"));
    } finally {
      await refreshCleanupModelStore();
      setCleanupModelStoreBusy(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const status = await invoke<CleanupModelStatus>("get_cleanup_model_store_status");
        if (cancelled) return;
        setCleanupModelStoreStatus(status);
        setCleanupModelStoreReadError(null);
      } catch {
        if (!cancelled) setCleanupModelStoreReadError(t("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。"));
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    const applyModels = (models: MlxCleanupModelStatus[]) => {
      if (cancelled) return;
      setMlxCleanupModels(Array.isArray(models) ? models : []);
      setMlxCleanupModelsReadError(null);
      setMlxCleanupModelsLoading(false);
    };
    void Promise.resolve(invoke<MlxCleanupModelStatus[]>("list_mlx_cleanup_models"))
      .then(applyModels)
      .catch((reason) => {
        if (!cancelled) {
          setMlxCleanupModelsReadError(`${t("本机 MLX 整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
          setMlxCleanupModelsLoading(false);
        }
      });
    const subscription = listen<MlxCleanupModelStatus>("mlx-cleanup-model://status", (event) => {
      if (cancelled || !event.payload?.id) return;
      setMlxCleanupModels((current) => {
        const next = current.filter((model) => model.id !== event.payload.id);
        return [...next, event.payload];
      });
      setMlxCleanupModelsReadError(null);
      setMlxCleanupModelsLoading(false);
    });
    return () => {
      cancelled = true;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const models = await invoke<LocalCleanupModelStatus[]>("list_local_cleanup_models");
        if (cancelled) return;
        const nextModels = Array.isArray(models) ? models : [];
        setLocalCleanupModels(nextModels);
        setLocalCleanupModelsReadError(null);
      } catch (reason) {
        if (cancelled) return;
        setLocalCleanupModelsReadError(`${t("本机整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
      } finally {
        if (!cancelled) setLocalCleanupModelsLoading(false);
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [t]);

  useEffect(() => {
    if (!cleanupModelStoreStatus?.operation) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const status = await invoke<CleanupModelStatus>("get_cleanup_model_store_status");
        if (cancelled) return;
        setCleanupModelStoreStatus(status);
        setCleanupModelStoreReadError(null);
        if (status.operation) timer = window.setTimeout(() => void poll(), 900);
      } catch {
        if (!cancelled) {
          setCleanupModelStoreReadError(t("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。"));
          timer = window.setTimeout(() => void poll(), 900);
        }
      }
    };
    timer = window.setTimeout(() => void poll(), 900);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [cleanupModelStoreStatus?.operation, t]);

  const localCleanupOperationsKey = localCleanupModels
    .filter((model) => Boolean(model.operation) || model.state === "downloading")
    .map((model) => `${model.id}:${model.operation ?? model.state}`)
    .sort()
    .join("|");

  useEffect(() => {
    if (!localCleanupOperationsKey) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const models = await invoke<LocalCleanupModelStatus[]>("list_local_cleanup_models");
        if (cancelled) return;
        const nextModels = Array.isArray(models) ? models : [];
        setLocalCleanupModels(nextModels);
        setLocalCleanupModelsReadError(null);
        if (nextModels.some((model) => Boolean(model.operation) || model.state === "downloading")) {
          timer = window.setTimeout(() => void poll(), 900);
        }
      } catch (reason) {
        if (!cancelled) {
          setLocalCleanupModelsReadError(`${t("本机整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
          timer = window.setTimeout(() => void poll(), 900);
        }
      }
    };
    timer = window.setTimeout(() => void poll(), 900);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [localCleanupOperationsKey, t]);

  const mlxCleanupOperationsKey = mlxCleanupModels
    .filter(isMlxCleanupModelOperationActive)
    .map((model) => `${model.id}:${model.operation ?? model.state}`)
    .sort()
    .join("|");

  useEffect(() => {
    if (!mlxCleanupOperationsKey) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const models = await invoke<MlxCleanupModelStatus[]>("list_mlx_cleanup_models");
        if (cancelled) return;
        const nextModels = Array.isArray(models) ? models : [];
        setMlxCleanupModels(nextModels);
        setMlxCleanupModelsReadError(null);
        if (nextModels.some(isMlxCleanupModelOperationActive)) {
          timer = window.setTimeout(() => void poll(), 900);
        }
      } catch (reason) {
        if (!cancelled) {
          setMlxCleanupModelsReadError(`${t("本机 MLX 整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
          timer = window.setTimeout(() => void poll(), 900);
        }
      }
    };
    timer = window.setTimeout(() => void poll(), 900);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [mlxCleanupOperationsKey, t]);

  const runOnDeviceAction = async (id: string, action: "download" | "cancel" | "cancel_load" | "delete") => {
    setOnDeviceActionId(id);
    setOnDeviceActionError(null);
    try {
      if (action === "download") await invoke("download_on_device_model", { id });
      else if (action === "cancel") await invoke("cancel_on_device_download", { id });
      else if (action === "cancel_load") await invoke<boolean>("cancel_on_device_model_load", { id });
      else await invoke("delete_on_device_model", { id });
      await refreshOnDeviceModels();
    } catch {
      setOnDeviceActionError({ id, message: t("本机模型操作失败，请重试。") });
      await refreshOnDeviceModels();
    } finally {
      setOnDeviceActionId(null);
    }
  };

  const checkLocalCleanupStatus = async () => {
    setLocalCleanupChecking(true);
    setLocalCleanupError(null);
    try {
      setLocalCleanupStatus(await invoke<LocalCleanupStatus>("get_local_cleanup_status"));
    } catch {
      setLocalCleanupStatus(null);
      setLocalCleanupError(t("无法读取本机 Ollama 状态，请重试。"));
    } finally {
      setLocalCleanupChecking(false);
    }
  };

  const chooseLocalCleanup = () => {
    const ollamaBaseUrl = providerById("ollama")?.defaultBaseUrl ?? "http://127.0.0.1:11434/v1";
    setDraft((current) => ({
      ...current,
      cleanupProvider: "ollama",
      cleanupModel: "qwen3.5:4b",
      ollamaBaseUrl,
    }));
    setDraftCleanupEnabled(true);
    setProbeSuccess("");
    setLocalCleanupStatus(null);
  };

  const currentCleanupProvider = providerOf(settings.cleanup_provider, settings.cleanup_base_url);
  const savedCleanupModel = settings.cleanup_model || defaultModel(currentCleanupProvider, "llm");
  const selectableLocalCleanupModels = localCleanupModels.filter(isSelectableLocalCleanupModel);
  const selectedLocalCleanupModel = localCleanupModels.find((model) => model.id === draft.cleanupModel);
  const selectedSavedLocalCleanupModel = localCleanupModels.find((model) => model.id === savedCleanupModel);
  const draftLocalCleanupModelReady = isSelectableLocalCleanupModel(selectedLocalCleanupModel)
    || (draft.cleanupProvider === "on_device_llama_cpp"
      && currentCleanupProvider === "on_device_llama_cpp"
      && draft.cleanupModel === savedCleanupModel
      && isReadyLocalCleanupModel(selectedLocalCleanupModel));
  const savedLocalCleanupModelReady = isReadyLocalCleanupModel(selectedSavedLocalCleanupModel);
  const localCleanupDeleteTarget = localCleanupModels.find((model) => model.id === confirmDeleteLocalCleanupModel);
  const cleaningOnlyPartialModel = Boolean(localCleanupDeleteTarget && !localCleanupDeleteTarget.managed && localCleanupDeleteTarget.partial_managed);
  const selectedMlxCleanupModel = mlxCleanupModels.find((model) => model.id === draft.cleanupModel);
  const selectedMlxCleanupLanguage = selectedMlxCleanupModel
    ? mlxCleanupLanguageCopy(selectedMlxCleanupModel.language)
    : null;
  const mlxCleanupDeleteTarget = mlxCleanupModels.find((model) => model.id === confirmDeleteMlxCleanupModel);
  const cleaningOnlyPartialMlxCleanupModel = Boolean(mlxCleanupDeleteTarget && !mlxCleanupDeleteTarget.managed && mlxCleanupDeleteTarget.partial_managed);
  const mlxCleanupManageTarget = draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
    ? "local-cleanup-model"
    : selectedMlxCleanupModel ? `mlx-cleanup-model-${selectedMlxCleanupModel.id}` : "local-mlx-cleanup-models";

  const selectedAsrIsLocal = draft.asrProvider === "on_device";
  const cleanupMlxPlatformSupported = cleanupModelStoreStatus?.platform_supported === true;
  const cleanupMlxPlatformUnsupported = cleanupModelStoreStatus?.platform_supported === false;
  const cleanupModelStoreNeedsRefresh = !cleanupModelStoreStatus
    || cleanupModelStoreReadError !== null
    || cleanupModelStoreStatus.platform_supported === null
    || cleanupModelStoreStatus.platform_supported === undefined;
  const cleanupModelStoreIdle = Boolean(cleanupModelStoreStatus && !cleanupModelStoreStatus.operation && !cleanupModelStoreReadError);
  const cleanupMlxInstalled = cleanupMlxPlatformSupported
    && cleanupModelStoreStatus != null
    && cleanupModelStoreStatus.managed
    && ["ready", "installed"].includes(cleanupModelStoreStatus.state);
  const draftMlxCleanupModelReady = draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
    ? cleanupMlxInstalled
    : selectedMlxCleanupModel?.selectable === true
      || (draft.cleanupProvider === "on_device_mlx"
        && currentCleanupProvider === "on_device_mlx"
        && draft.cleanupModel === savedCleanupModel
        && selectedMlxCleanupModel?.state === "ready"
        && selectedMlxCleanupModel.verified
        && selectedMlxCleanupModel.compatible
        && !isMlxCleanupModelOperationActive(selectedMlxCleanupModel));
  const selectedCleanupIsLocal = (draft.cleanupProvider === "ollama" && isLoopbackUrl(draft.ollamaBaseUrl))
    || (draft.cleanupProvider === "on_device_mlx" && draftMlxCleanupModelReady)
    || (draft.cleanupProvider === "on_device_llama_cpp" && draftLocalCleanupModelReady);
  const strictOfflineRoutesAllowed = settings.strict_offline_enabled !== true
    || (selectedAsrIsLocal && (!draftCleanupEnabled || selectedCleanupIsLocal));
  const asrMissing = !hasProviderSecret(draft.asrProvider, draft, settings, onDeviceReady);
  const cleanupMissing = draftCleanupEnabled && !hasProviderSecret(draft.cleanupProvider, draft, settings);
  const hasSupportedAsrLanguageSetting = hasSupportedAsrLanguage(asrModelProfile(draft.asrProvider, draft.asrModel), settings.language);
  const cleanupModelRouteReady = draft.cleanupProvider === "on_device_mlx"
    ? draftMlxCleanupModelReady
    : draft.cleanupProvider === "on_device_llama_cpp"
      ? draftLocalCleanupModelReady
      : true;
  const ready = step2Ready(draft, draftSettings, onDeviceReady)
    && hasSupportedAsrLanguageSetting
    && strictOfflineRoutesAllowed
    && (!draftCleanupEnabled || cleanupModelRouteReady)
    && (!draftCleanupEnabled || draft.cleanupProvider !== "custom" || customParametersParse.ok);
  const saveBlock: { message: string; page: EnginePage; target: string; action: string } | null = !ready
    ? !hasSupportedAsrLanguageSetting
      ? { message: t("当前转写模型不支持所选语言，请更换转写模型。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
      : !strictOfflineRoutesAllowed
        ? { message: t("严格离线模式会阻止草稿中的路线；请选择允许的本机路线或关闭严格离线模式。"), page: "service", target: draft.asrProvider !== "on_device" ? "engine-asr-provider" : "engine-cleanup-provider", action: t("修改路线") }
        : draft.asrProvider === "custom" && !draft.customBaseUrl.trim()
          ? { message: t("请先填写自定义服务地址。"), page: "advanced", target: "advanced-custom-base-url", action: t("管理连接地址") }
          : draft.asrProvider === "on_device" && !onDeviceReady
            ? { message: t("请安装并加载所选本机转写模型。"), page: "local_models", target: "local-asr-models", action: t("管理模型") }
            : asrMissing
              ? { message: t("请先添加当前转写服务的 API Key。"), page: "api_key", target: `provider-key-${draft.asrProvider}`, action: t("管理凭据") }
              : !draft.asrModel.trim()
                ? { message: t("请先选择转写模型。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
                : draftCleanupEnabled && draft.cleanupProvider === "custom" && !draft.customBaseUrl.trim()
                  ? { message: t("请先填写自定义服务地址。"), page: "advanced", target: "advanced-custom-base-url", action: t("管理连接地址") }
                  : draftCleanupEnabled && cleanupMissing && draft.asrProvider !== "assemblyai"
                    ? { message: t("请先添加当前整理服务的 API Key。"), page: "api_key", target: `provider-key-${draft.cleanupProvider}`, action: t("管理凭据") }
                    : draftCleanupEnabled && draft.cleanupProvider === "on_device_llama_cpp" && selectableLocalCleanupModels.length === 0
                      ? { message: t("请先下载并验证兼容的本机整理模型。"), page: "local_models", target: "local-cleanup-models", action: t("管理模型") }
                    : draftCleanupEnabled && !draft.cleanupModel.trim()
                      ? { message: t("请先选择整理模型。"), page: "service", target: "engine-cleanup-provider", action: t("修改路线") }
                      : draftCleanupEnabled && !cleanupModelRouteReady
                        ? draft.cleanupProvider === "on_device_llama_cpp"
                          ? { message: t("请先下载并验证兼容的本机整理模型。"), page: "local_models", target: "local-cleanup-models", action: t("管理模型") }
                          : { message: t("请先安装并验证 VoiceFlow MLX 整理模型。"), page: "local_models", target: mlxCleanupManageTarget, action: t("管理模型") }
                        : draftCleanupEnabled && draft.cleanupProvider === "custom" && !customParametersParse.ok
                          ? { message: customParametersParse.error, page: "advanced", target: "advanced-custom-cleanup-parameters", action: t("检查参数") }
                          : { message: t("请检查当前服务配置后再保存。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
    : null;
  const previousCredentialRoute = useRef({ asr: draft.asrProvider, cleanup: draft.cleanupProvider, cleanupEnabled: draftCleanupEnabled });

  useEffect(() => {
    const previous = previousCredentialRoute.current;
    previousCredentialRoute.current = { asr: draft.asrProvider, cleanup: draft.cleanupProvider, cleanupEnabled: draftCleanupEnabled };
    const cleanupRequiredForSelectedAsr = cleanupMissing && draft.asrProvider !== "assemblyai";
    // Keep the page overview visible on entry; guide to a missing key only
    // after the user changes the corresponding service.
    const missing = previous.asr !== draft.asrProvider && asrMissing
      ? draft.asrProvider
      : (previous.cleanup !== draft.cleanupProvider || !previous.cleanupEnabled && draftCleanupEnabled) && cleanupRequiredForSelectedAsr
        ? draft.cleanupProvider
        : null;
    if (!missing) return;
    if (missing === "on_device") {
      navigateToPage("local_models", "local-asr-models");
    } else {
      navigateToPage("api_key", `provider-key-${missing}`);
    }
  }, [asrMissing, cleanupMissing, draft.asrProvider, draft.cleanupProvider, draftCleanupEnabled, navigateToPage]);

  const currentAsrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const savedAsrModel = settings.asr_model || defaultModel(currentAsrProvider, "asr");
  const legacyModel = onDeviceModels.find((model) => model.id === "sensevoice-small");
  const legacyModelSavedSelected = currentAsrProvider === "on_device" && savedAsrModel === "sensevoice-small";
  const legacyModelDraftSelected = draft.asrProvider === "on_device" && draft.asrModel === "sensevoice-small";
  const legacyModelShouldOpen = Boolean(legacyModel && (
    legacyModelSavedSelected
    || legacyModelDraftSelected
    || legacyModel.state === "downloading"
    || legacyModel.runtime_status === "loading"
    || Boolean(legacyModel.error)
    || onDeviceActionError?.id === legacyModel.id
  ));
  useEffect(() => {
    if (legacyModelShouldOpen) setLegacyModelsOpen(true);
  }, [legacyModelShouldOpen]);
  const savedDraft = draftFromSettings(settings);
  const draftPatch = persistPatch({ ...draft, providerKeys: {} }, draftSettings);
  const savedPatch = persistPatch(savedDraft, draftSettings);
  delete draftPatch.custom_cleanup_parameters;
  delete savedPatch.custom_cleanup_parameters;
  const customParametersDirty = customParametersParse.ok
    ? stableJson(customParametersParse.value) !== stableJson(savedDraft.customCleanupParameters)
    : draft.customCleanupParametersText !== savedDraft.customCleanupParametersText;
  const pendingChanges = draftCleanupEnabled !== settings.cleanup_enabled
    || customParametersDirty
    || JSON.stringify(draftPatch) !== JSON.stringify(savedPatch)
    || Object.values(draft.providerKeys).some((value) => value?.trim());
  const showEngineFooter = pendingChanges || probing;

  useEffect(() => {
    setStageFail({});
    setCommitError(null);
    setRowProbeFeedback(null);
  }, [asrRoute, cleanupRoute, draftCleanupEnabled]);

  const failKind = (kind: string | null | undefined) => {
    if (kind === "key" || kind === "missing_key") return t("密钥无效");
    if (kind) return probeCopy(kind, undefined, t);
    return null;
  };

  const strictOfflineBlocksProbe = (provider: ProviderId, stage: "asr" | "cleanup") => {
    if (settings.strict_offline_enabled !== true) return false;
    if (stage === "asr") return provider !== "on_device";
    if (provider === "ollama") return !isLoopbackUrl(draft.ollamaBaseUrl);
    return provider !== "on_device_mlx" && provider !== "on_device_llama_cpp";
  };

  const applyDraft = async () => {
    setProbing(true);
    setProbeSuccess("");
    setCommitError(null);
    try {
      await commitEngine({ ...persistPatch(draft, draftSettings), cleanup_enabled: draftCleanupEnabled });
      setDraft((current) => ({ ...current, providerKeys: {} }));
      setStageFail({});
      setTestedAsrRoute(null);
      setTestedCleanupRoute(null);
      setProbeSuccess("saved");
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setProbing(false);
    }
  };

  const updateStrictOfflineMode = async (enabled: boolean) => {
    setStrictOfflineSaving(true);
    setStrictOfflineSaveError(null);
    try {
      await commitEngine({ strict_offline_enabled: enabled });
    } catch (reason) {
      setStrictOfflineSaveError(friendlySettingsError(reason, t));
    } finally {
      setStrictOfflineSaving(false);
    }
  };

  const testConnection = async (id: ProviderId, stageScope: "asr" | "cleanup") => {
    const definition = providerById(id);
    if (!definition) return;
    if (!definition.capabilities.includes(stageScope === "asr" ? "asr" : "llm")) return;
    const requestRevision = probeConfigurationRevision;
    setRowProbing(id);
    setRowProbingStage(stageScope);
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setCommitError(null);
    if (stageScope === "cleanup" && id === "custom" && !customParametersParse.ok) {
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: customParametersParse.error });
      setRowProbing(null);
      setRowProbingStage(null);
      return;
    }
    if (strictOfflineBlocksProbe(id, stageScope)) {
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: t("严格离线模式会阻止连接云端服务。") });
      setRowProbing(null);
      setRowProbingStage(null);
      return;
    }
    try {
      if (stageScope === "asr" && id === draft.asrProvider) setTestedAsrRoute(null);
      if (stageScope === "cleanup" && id === draft.cleanupProvider) setTestedCleanupRoute(null);
      if (id === "soniox") return;
      const typedKey = draft.providerKeys[id]?.trim();
      const probeDraft: EngineDraft = {
        ...draft,
        providerKeys: typedKey ? { [id]: typedKey } : {},
      };
      const result = await invoke<ProbeResult>("probe_engine_draft", {
        draft: probePayload(probeDraft, draftSettings, stageScope === "asr"
          ? { asrProvider: id, cleanupEnabled: false, probeStage: "asr" }
          : { cleanupProvider: id, cleanupEnabled: true, probeStage: "cleanup" }),
      });
      if (probeConfigurationRevisionRef.current !== requestRevision) return;
      const stage = stageScope === "asr" ? result.asr : result.cleanup;
      if (stage.skipped) {
        setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: t("此阶段未运行，请检查服务选择后重试。") });
        return;
      }
      if (!stage.ok) {
        setStageFail((current) => (
          stageScope === "asr" && id === draft.asrProvider ? { ...current, asr: stage.error_kind }
            : stageScope === "cleanup" && id === draft.cleanupProvider ? { ...current, cleanup: stage.error_kind } : current
        ));
        setRowProbeFeedback({
          provider: id,
          stage: stageScope,
          tone: "error",
          message: `${definition.label}：${probeCopy(stage.error_kind, stage.message, t)}`,
        });
        return;
      }
      setStageFail((current) => (
        stageScope === "asr" && id === draft.asrProvider ? { ...current, asr: undefined }
          : stageScope === "cleanup" && id === draft.cleanupProvider ? { ...current, cleanup: undefined } : current
      ));
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "success", message: `${stageScope === "asr" ? t("转写") : t("文字整理")} · ${t("测试连接通过")}` });
      if (stageScope === "asr" && id === draft.asrProvider) setTestedAsrRoute(asrRoute);
      if (stageScope === "cleanup" && id === draft.cleanupProvider) setTestedCleanupRoute(cleanupRoute);
    } catch (reason) {
      if (probeConfigurationRevisionRef.current !== requestRevision) return;
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: friendlySettingsError(reason, t) });
    } finally {
      setRowProbing(null);
      setRowProbingStage(null);
    }
  };

  const removeKey = async (id: ProviderId) => {
    setConfirmRemove(null);
    setRemoving(true);
    setRowProbeFeedback(null);
    try {
      await removeProviderKey(id);
      if (id === draft.asrProvider) setTestedAsrRoute(null);
      if (id === draft.cleanupProvider) setTestedCleanupRoute(null);
      setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
      setStageFail({});
    } catch (reason) {
      setExpanded((current) => new Set([...current, id]));
      setRowProbeFeedback({ provider: id, tone: "error", message: `${t("密钥删除失败，请重试。")} ${friendlySettingsError(reason, t)}` });
    } finally {
      setRemoving(false);
    }
  };

  const readCredential = async (provider: "groq" | "custom", purpose?: ProviderCredentialPurpose) => {
    if (!readProviderCredential || credentialReadRequest.current) return;
    const requestId = crypto.randomUUID();
    credentialReadRequest.current = requestId;
    setCredentialReadPending(true);
    setCredentialReadTarget(provider);
    setCredentialReadStatus(null);
    setCredentialReadMessage(null);
    try {
      const result = purpose
        ? await readProviderCredential(provider, requestId, purpose)
        : await readProviderCredential(provider, requestId);
      if (credentialReadRequest.current !== requestId) return;
      setCredentialReadStatus(result.status);
      const messages = provider === "custom" ? {
        session_configured: { tone: "success" as const, text: t("自定义服务密钥已在本次打开期间读取；重启后可能需要再次授权。") },
        missing: { tone: "status" as const, text: t("当前应用钥匙串中没有可读取的自定义服务密钥。") },
        unavailable: { tone: "error" as const, text: t("钥匙串暂时不可读取；当前设置未更改。") },
        cancelled: { tone: "status" as const, text: t("已取消钥匙串授权；当前设置未更改。") },
        busy: { tone: "status" as const, text: t("请等待当前钥匙串读取完成。") },
        foreground_required: { tone: "error" as const, text: t("请在前台打开设置后重试。") },
        stale: { tone: "status" as const, text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") },
        unsupported: { tone: "error" as const, text: t("请先保存一个有效的自定义服务地址，并确保当前转写或整理路线使用自定义服务。") },
      } : {
        session_configured: { tone: "success" as const, text: t("钥匙串密钥已在本次打开期间读取；重启后可能需要再次授权。") },
        missing: { tone: "status" as const, text: t("当前应用的钥匙串中没有可读取的 Groq 密钥。") },
        unavailable: { tone: "error" as const, text: t("钥匙串暂时不可读取；当前设置未更改。") },
        cancelled: { tone: "status" as const, text: t("已取消钥匙串授权；当前设置未更改。") },
        busy: { tone: "status" as const, text: t("请等待当前钥匙串读取完成。") },
        foreground_required: { tone: "error" as const, text: t("请在前台打开设置后重试。") },
        stale: { tone: "status" as const, text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") },
        unsupported: { tone: "error" as const, text: t("此操作仅适用于 Groq 密钥。") },
      } satisfies Record<ProviderCredentialReadResult["status"], { tone: "success" | "status" | "error"; text: string }>;
      setCredentialReadMessage(messages[result.status]);
    } catch {
      if (credentialReadRequest.current === requestId) {
        setCredentialReadStatus("unavailable");
        setCredentialReadTarget(provider);
        setCredentialReadMessage({ tone: "error", text: t("钥匙串暂时不可读取；当前设置未更改。") });
      }
    } finally {
      if (credentialReadRequest.current === requestId) {
        credentialReadRequest.current = null;
        setCredentialReadPending(false);
      }
    }
  };

  const changeAsrProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "asr", value));
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
  };

  const changeDashscopeRegion = (value: string) => {
    if (value !== "beijing" && value !== "singapore") return;
    setDraft((current) => ({ ...current, dashscopeRegion: value }));
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
    setStageFail({});

  };

  const changeCleanupProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "cleanup", value));
    setRowProbeFeedback(null);
    setProbeSuccess("");
  };

  const changeAsrModel = (asrModel: string) => {
    setDraft({ ...draft, asrModel });
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
  };

  const changeCleanupModel = (cleanupModel: string) => {
    setDraft({ ...draft, cleanupModel });
    setRowProbeFeedback(null);
    setStageFail((current) => ({ ...current, cleanup: undefined }));
    setProbeSuccess("");
  };

  const toggleExpanded = (id: ProviderId) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const asrOptions = providersFor("asr", true, true);
  const cleanupOptions = providersFor("llm", true, true);
  const onDeviceAsrModelDefinitions = providerById("on_device")?.asrModels ?? [];
  const reportedOnDeviceAsrModelOptions: ModelOption[] = onDeviceModels
    .filter((model) => model.download_available === true
      || (currentAsrProvider === "on_device" && model.id === savedAsrModel))
    .map((model) => {
      const definition = onDeviceAsrModelDefinitions.find((option) => option.value === model.id);
      return {
        value: model.id,
        label: model.label,
        note: model.download_available === true ? formatModelBytes(model.bytes) : t("旧配置保留；不提供新下载或选择"),
        retiredForNewSelection: model.download_available !== true,
        asrProfile: definition?.asrProfile,
      };
    });
  const onDeviceAsrModelOptions = reportedOnDeviceAsrModelOptions.length > 0
    ? reportedOnDeviceAsrModelOptions
    : onDeviceAsrModelDefinitions.filter((option) => (
      option.value === defaultModel("on_device", "asr")
      || (currentAsrProvider === "on_device" && option.value === savedAsrModel)
    ));
  const cloudAsrOptions = asrOptions.filter((provider) => provider.id !== "custom" && provider.id !== "on_device" && provider.id !== "local_whisper");
  const localAsrOptions = asrOptions.filter((provider) => provider.id === "on_device" || provider.id === "local_whisper");
  const customAsrOptions = asrOptions.filter((provider) => provider.id === "custom");
  const localCleanupModelOptions: ModelOption[] = selectableLocalCleanupModels.map((model) => ({
    value: model.id,
    label: model.label,
    note: formatModelBytes(model.bytes),
  }));
  const selectableMlxCleanupModels = mlxCleanupModels.filter((model) => model.selectable);
  const visibleLocalCleanupModels = localCleanupModels.filter((model) => (
    model.download_available === true
    || model.managed
    || model.partial_managed
    || Boolean(model.operation)
    || ["downloading", "canceling", "deleting"].includes(model.state)
    || (currentCleanupProvider === "on_device_llama_cpp" && model.id === savedCleanupModel)
    || (draft.cleanupProvider === "on_device_llama_cpp" && model.id === draft.cleanupModel)
  ));
  const localCleanupManagementRequired = visibleLocalCleanupModels.length > 0
    || localCleanupModelsReadError !== null
    || currentCleanupProvider === "on_device_llama_cpp"
    || draft.cleanupProvider === "on_device_llama_cpp";
  const visibleMlxCleanupModels = mlxCleanupModels.filter((model) => (
    model.download_available === true
    || model.managed
    || model.partial_managed
    || isMlxCleanupModelOperationActive(model)
    || (currentCleanupProvider === "on_device_mlx" && model.id === savedCleanupModel)
  ));
  const mlxCleanupModelOptions: ModelOption[] = [
    ...(providerById("on_device_mlx")?.llmModels ?? []),
    ...selectableMlxCleanupModels.map((model) => ({
      value: model.id,
      label: model.label,
      note: model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
    })),
  ];
  if (draft.cleanupProvider === "on_device_mlx" && draft.cleanupModel && !mlxCleanupModelOptions.some((option) => option.value === draft.cleanupModel)) {
    const unavailableModel = mlxCleanupModels.find((model) => model.id === draft.cleanupModel);
    mlxCleanupModelOptions.push({
      value: draft.cleanupModel,
      label: unavailableModel?.label ?? draft.cleanupModel,
      note: unavailableModel ? mlxCleanupModelStateCopy(unavailableModel, t) : t("模型状态暂不可用"),
      retiredForNewSelection: true,
    });
  }
  if (draft.cleanupProvider === "on_device_llama_cpp"
    && currentCleanupProvider === "on_device_llama_cpp"
    && draft.cleanupModel === savedCleanupModel
    && !localCleanupModelOptions.some((option) => option.value === draft.cleanupModel)) {
    const savedModel = localCleanupModels.find((model) => model.id === draft.cleanupModel);
    if (savedModel) {
      localCleanupModelOptions.push({
        value: savedModel.id,
        label: savedModel.label,
        note: localCleanupModelStateCopy(savedModel.state, savedModel.verified, t, savedModel.partial_managed),
        retiredForNewSelection: true,
      });
    }
  }
  const cloudCleanupOptions = cleanupOptions.filter((provider) => !["custom", "ollama", "on_device_llama_cpp", "on_device_mlx"].includes(provider.id));
  const localCleanupOptions = cleanupOptions.filter((provider) => ["ollama", "on_device_llama_cpp", "on_device_mlx"].includes(provider.id));
  const customCleanupOptions = cleanupOptions.filter((provider) => provider.id === "custom");
  const renderAsrOption = (provider: typeof PROVIDERS[number]) => (
    <option
      key={provider.id}
      value={provider.id}
      disabled={provider.id === "on_device" && onDeviceModels.length > 0 && !hasSupportedDeviceModel && draft.asrProvider !== "on_device"}
    >{provider.id === "custom" ? t("自定义语音转写") : provider.id === "on_device" ? t("本机模型") : provider.label}</option>
  );
  const renderCleanupOption = (provider: typeof PROVIDERS[number]) => (
    <option
      key={provider.id}
      value={provider.id}
      disabled={provider.id === "on_device_llama_cpp"
        && selectableLocalCleanupModels.length === 0
        && !(currentCleanupProvider === "on_device_llama_cpp" && draft.cleanupModel === savedCleanupModel)}
    >{provider.id === "custom" ? t("自定义 AI 整理") : provider.label}</option>
  );
  const customRecoveryPurpose: ProviderCredentialPurpose | null = settings.asr_provider === "custom"
    ? "asr"
    : settings.cleanup_enabled && settings.cleanup_provider === "custom"
      ? "cleanup"
      : null;
  const customReadState = settings.provider_keys?.custom?.keychain_state;
  const groqReadState = settings.provider_keys?.groq?.keychain_state;
  const groqLatestReadStatus = credentialReadTarget === "groq" ? credentialReadStatus : null;
  const customLatestReadStatus = credentialReadTarget === "custom" ? credentialReadStatus : null;
  const asrProfile = asrModelProfile(draft.asrProvider, draft.asrModel);
  const hasSupportedDeviceModel = onDeviceModels.some((model) => model.download_available === true && model.platform_supported);
  const savedOllamaBaseUrl = settings.ollama_base_url || providerById("ollama")?.defaultBaseUrl || "";
  const localCleanupRouteSaved = draft.ollamaBaseUrl.trim() === savedOllamaBaseUrl.trim();
  const savedAsrBlocked = settings.strict_offline_enabled === true && currentAsrProvider !== "on_device";
  const savedCleanupBlocked = settings.strict_offline_enabled === true && settings.cleanup_enabled
    && !(currentCleanupProvider === "ollama" && isLoopbackUrl(savedOllamaBaseUrl))
    && currentCleanupProvider !== "on_device_mlx"
    && !(currentCleanupProvider === "on_device_llama_cpp" && savedLocalCleanupModelReady);
  const asrFailLabel = failKind(stageFail.asr);
  const credentialStatus = (provider: ProviderId, tested: boolean) => tested
    ? { label: t("服务检查通过"), tone: "success" as const }
    : draft.providerKeys[provider]?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : providerConfigured(settings, provider)
        ? { label: t("凭据已保存 · 本次尚未检查"), tone: "neutral" as const }
        : { label: t("本机服务尚未检查"), tone: "neutral" as const };
  const asrStatus = draft.asrProvider === "on_device"
    ? {
        label: selectedOnDeviceModel?.runtime_status === "loading"
          ? t("本机模型正在加载")
          : onDeviceReady
          ? selectedOnDeviceModel?.loaded ? t("模型已加载") : t("本机运行时握手通过")
          : selectedOnDeviceModel?.platform_supported === false
            ? t("本机模型不适用于当前设备")
            : selectedOnDeviceModel?.state === "ready"
              ? onDeviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t)
              : onDeviceStateCopy(selectedOnDeviceModel?.state ?? "missing", t),
        tone: onDeviceReady && selectedOnDeviceModel?.runtime_status !== "loading" ? "success" as const : "warning" as const,
      }
    : settings.strict_offline_enabled && !selectedAsrIsLocal
    ? { label: t("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用"), tone: "warning" as const }
    : !hasSupportedAsrLanguageSetting
      ? { label: t("此模型要求固定使用中文或 English"), tone: "warning" as const }
    : asrMissing
    ? { label: t("未配置密钥"), tone: "warning" as const }
    : draft.asrProvider === "soniox"
      ? { label: t(draft.providerKeys.soniox?.trim() || !providerConfigured(settings, "soniox") ? "密钥待保存" : "凭据已保存 · 听写时连接"), tone: draft.providerKeys.soniox?.trim() || !providerConfigured(settings, "soniox") ? "warning" as const : "neutral" as const }
    : asrFailLabel
      ? { label: asrFailLabel, tone: "error" as const }
    : credentialStatus(draft.asrProvider, testedAsrRoute === asrRoute);
  const cleanupFailLabel = failKind(stageFail.cleanup);
  const mlxCleanupRouteStatus = (() => {
    if (draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID) {
      if (!cleanupMlxPlatformSupported) {
        return { label: cleanupMlxPlatformUnsupported ? t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。") : t("无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"), tone: "warning" as const };
      }
      if (cleanupModelStoreStatus?.operation) {
        return { label: cleanupModelStoreStatus.operation === "download" ? t("正在下载本机模型") : cleanupModelStoreStatus.operation === "import" ? t("正在导入本机模型") : t("正在删除本机模型"), tone: "warning" as const };
      }
      if (cleanupMlxInstalled && cleanupModelStoreStatus?.state === "ready") return { label: t("本机 MLX 模型已验证"), tone: "success" as const };
      if (cleanupMlxInstalled) return { label: t("已安装；首次使用时会重新校验"), tone: "neutral" as const };
      return { label: t("先导入或下载本机模型"), tone: "warning" as const };
    }
    if (!selectedMlxCleanupModel) {
      return mlxCleanupModelsReadError
        ? { label: t("本机整理模型状态读取失败"), tone: "error" as const }
        : { label: t("先下载并验证兼容的本机整理模型"), tone: "warning" as const };
    }
    if (selectedMlxCleanupModel.selectable) return { label: t("本机 MLX 模型文件已校验"), tone: "success" as const };
    if (isMlxCleanupModelOperationActive(selectedMlxCleanupModel)) {
      return { label: selectedMlxCleanupModel.operation === "verify"
        ? t("正在校验模型文件")
        : selectedMlxCleanupModel.operation === "delete" || selectedMlxCleanupModel.state === "deleting"
          ? t("正在删除")
          : t("正在下载 MLX 整理模型"), tone: "warning" as const };
    }
    if (!selectedMlxCleanupModel.compatible) {
      return { label: mlxCleanupCompatibilityCopy(selectedMlxCleanupModel.compatibility_error, t), tone: "warning" as const };
    }
    return { label: mlxCleanupModelStateCopy(selectedMlxCleanupModel, t), tone: "warning" as const };
  })();
  const cleanupStatus = !draftCleanupEnabled
    ? { label: t("关闭 · 只用本地规则"), tone: "unused" as const }
    : draft.cleanupProvider === "on_device_llama_cpp"
      ? draftLocalCleanupModelReady
        ? { label: t("本机整理模型已下载、校验并兼容"), tone: "success" as const }
        : selectedLocalCleanupModel?.operation === "download" || selectedLocalCleanupModel?.state === "downloading"
          ? { label: t("正在下载本机整理模型"), tone: "warning" as const }
          : selectedLocalCleanupModel?.compatible === false
            ? { label: localCleanupCompatibilityCopy(selectedLocalCleanupModel.compatibility_error, t), tone: "warning" as const }
            : selectedLocalCleanupModel
              ? { label: localCleanupModelStateCopy(selectedLocalCleanupModel.state, selectedLocalCleanupModel.verified, t, selectedLocalCleanupModel.partial_managed), tone: "warning" as const }
              : localCleanupModelsReadError
                ? { label: t("本机整理模型状态读取失败"), tone: "error" as const }
                : { label: selectableLocalCleanupModels.length > 0 ? t("请选择已下载并验证的模型") : t("先下载并验证兼容的本机整理模型"), tone: "warning" as const }
    : draft.cleanupProvider === "on_device_mlx"
      ? mlxCleanupRouteStatus
    : settings.strict_offline_enabled && !["ollama", "on_device_llama_cpp", "on_device_mlx"].includes(draft.cleanupProvider)
      ? { label: t("严格离线模式会阻止此云端整理服务"), tone: "warning" as const }
      : draft.cleanupProvider === "ollama" && localCleanupStatus && !localCleanupStatus.available
        ? { label: t("本机整理服务不可用"), tone: "warning" as const }
        : draft.asrProvider === "assemblyai"
          ? cleanupMissing
            ? { label: t("120 秒内的融合 Dictation 候选不需要此密钥；长录音和候选失败后可选用共同整理，无密钥时使用本地回退"), tone: "warning" as const }
            : cleanupFailLabel
              ? { label: cleanupFailLabel, tone: "error" as const }
              : credentialStatus(draft.cleanupProvider, testedCleanupRoute === cleanupRoute)
          : cleanupMissing
            ? { label: t("未配置密钥"), tone: "warning" as const }
            : cleanupFailLabel
              ? { label: cleanupFailLabel, tone: "error" as const }
              : credentialStatus(draft.cleanupProvider, testedCleanupRoute === cleanupRoute);

  const renderProvider = (provider: typeof PROVIDERS[number]) => {
    const inUse = currentAsrProvider === provider.id || (settings.cleanup_enabled && currentCleanupProvider === provider.id);
    const inDraft = draft.asrProvider === provider.id || (draftCleanupEnabled && draft.cleanupProvider === provider.id);
    const configured = providerConfigured(settings, provider.id) || Boolean(draft.providerKeys[provider.id]?.trim());
    const requestedOpen = expanded.has(provider.id);
    const customAsrActive = draft.asrProvider === "custom";
    const customCleanupActive = draftCleanupEnabled && draft.cleanupProvider === "custom";
    const customProviderTitle = customAsrActive && customCleanupActive
      ? t("自定义服务")
      : customCleanupActive
        ? t("自定义 AI 整理")
        : customAsrActive
          ? t("自定义语音转写")
          : t("自定义服务");
    const rowKind = [
      draft.asrProvider === provider.id ? stageFail.asr : undefined,
      draftCleanupEnabled && draft.cleanupProvider === provider.id
        ? stageFail.cleanup
        : undefined,
    ].find((kind) => kind);
    const rowFail = failKind(rowKind);
    const rowError = Boolean(rowFail) || (rowProbeFeedback?.provider === provider.id && rowProbeFeedback.tone === "error");
    const open = requestedOpen || rowError;
    const providerProbeStages: Array<"asr" | "cleanup"> = provider.id === "soniox" ? [] : [
      ...(provider.capabilities.includes("asr") ? ["asr" as const] : []),
      ...(provider.capabilities.includes("llm") ? ["cleanup" as const] : []),
    ];
    const groqCredentialStatus = draft.providerKeys.groq?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : credentialReadPending && credentialReadTarget === "groq"
        ? { label: t("正在读取"), tone: "warning" as const }
        : groqLatestReadStatus === "missing"
          ? providerConfigured(settings, "groq")
            ? { label: t("本次会话有凭据 · 钥匙串中未找到密钥"), tone: "warning" as const }
            : { label: t("未添加"), tone: "unused" as const }
          : groqLatestReadStatus === null && groqReadState === "unavailable"
            ? providerConfigured(settings, "groq")
              ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
              : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
            : groqLatestReadStatus === null && groqReadState === "missing"
              ? { label: t("未添加"), tone: "unused" as const }
              : groqLatestReadStatus === null
                ? { label: t("未读取"), tone: "neutral" as const }
                : { label: t("待确认"), tone: "neutral" as const };
    const customCredentialStatus = draft.providerKeys.custom?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : credentialReadPending && credentialReadTarget === "custom"
        ? { label: t("正在读取"), tone: "warning" as const }
        : customLatestReadStatus === "missing"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串中未找到密钥"), tone: "warning" as const }
            : { label: t("未添加"), tone: "unused" as const }
        : customLatestReadStatus === "unavailable"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
            : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
        : customReadState === "unavailable"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
            : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
          : customReadState === "missing"
            ? { label: t("未添加"), tone: "unused" as const }
            : { label: t("待确认"), tone: "neutral" as const };
    const rowTone = rowFail
      ? { label: rowFail, tone: "error" as const }
      : provider.id === "groq" && !providerConfigured(settings, "groq")
        ? groqCredentialStatus
      : provider.id === "groq" && groqReadState === "unavailable" && providerConfigured(settings, "groq")
        ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
      : provider.id === "custom" && customRecoveryPurpose !== null && customReadState !== "configured"
        ? customCredentialStatus
      : inUse
        ? { label: t("使用中"), tone: "accent" as const }
        : inDraft
          ? { label: t("待应用"), tone: "warning" as const }
        : providerConfigured(settings, provider.id)
          ? { label: t("已配置"), tone: "unused" as const }
          : { label: t("未添加"), tone: "unused" as const };
    const hasTypedKey = Boolean(draft.providerKeys[provider.id]?.trim());
    const rowCredentialStatus = inUse || inDraft || hasTypedKey
      ? (() => {
          if (!providerHasCredentialInput(provider.id)) return null;
          const credentialDraft = inDraft || hasTypedKey ? draft : savedDraft;
          if (hasTypedKey) {
            return { label: t("密钥待保存"), tone: "warning" as const };
          }
          if (provider.id === "groq" || (provider.id === "custom" && customRecoveryPurpose !== null)) return null;
          if (provider.id === "custom" && customHostChanged(credentialDraft, settings)) {
            return { label: t("未配置密钥"), tone: "warning" as const };
          }
          if (!hasProviderSecret(provider.id, credentialDraft, settings, onDeviceReady)) {
            return { label: t("未配置密钥"), tone: "warning" as const };
          }
          if (provider.id === "soniox") {
            return { label: t("凭据已保存 · 听写时连接"), tone: "neutral" as const };
          }
          const testedForCurrentStage = provider.id === draft.asrProvider && testedAsrRoute === asrRoute
            || draftCleanupEnabled && provider.id === draft.cleanupProvider && testedCleanupRoute === cleanupRoute;
          return providerConfigured(settings, provider.id)
            ? { label: t(testedForCurrentStage ? "已配置" : "凭据已保存 · 本次尚未检查"), tone: "neutral" as const }
            : { label: t("本机服务尚未检查"), tone: "neutral" as const };
        })()
      : null;
    const distinctCredentialStatus = rowCredentialStatus?.label === rowTone.label ? null : rowCredentialStatus;
    const providerTitle = provider.id === "custom" ? customProviderTitle : provider.label;
    return (
      <div key={provider.id} data-provider={provider.id} data-selected={inDraft} className="vf-provider-row">
        <SettingsRow title={providerTitle} description={capabilityText(provider.id, t)}>
          <SettingsStatus label={rowTone.label} tone={rowTone.tone} />
          {distinctCredentialStatus && <SettingsStatus label={distinctCredentialStatus.label} tone={distinctCredentialStatus.tone} />}
          {provider.id === "groq"
            && (!providerConfigured(settings, "groq") || groqReadState === "unavailable")
            && !draft.providerKeys.groq?.trim()
            && readProviderCredential && (
            <button
              type="button"
              onClick={() => {
                setExpanded((current) => new Set([...current, "groq"]));
                void readCredential("groq");
              }}
              aria-busy={credentialReadPending}
              disabled={credentialReadPending || rowProbing !== null || probing || removing}
              className={compactButtonClass}
            >
              {credentialReadPending ? t("正在请求钥匙串授权…") : t("从钥匙串重新读取")}
            </button>
          )}
          {provider.id === "custom"
            && customRecoveryPurpose !== null
            && (!providerConfigured(settings, "custom") || customReadState === "unavailable")
            && !draft.providerKeys.custom?.trim()
            && readProviderCredential && (
            <button
              type="button"
              onClick={() => {
                setExpanded((current) => new Set([...current, "custom"]));
                void readCredential("custom", customRecoveryPurpose);
              }}
              aria-busy={credentialReadPending && credentialReadTarget === "custom"}
              disabled={credentialReadPending || rowProbing !== null || probing || removing}
              className={compactButtonClass}
            >
              {credentialReadPending && credentialReadTarget === "custom" ? t("正在请求钥匙串授权…") : t("从钥匙串重新读取")}
            </button>
          )}
          <button type="button" aria-label={`${open ? t("收起") : configured ? t("编辑") : t("添加")} ${providerTitle}`} aria-expanded={open} aria-controls={`provider-details-${provider.id}`} onClick={() => toggleExpanded(provider.id)} className={compactButtonClass}>
            {open ? t("收起") : configured ? t("编辑") : t("添加")}
          </button>
        </SettingsRow>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} id={`provider-details-${provider.id}`} hidden={!open} inert={!open} className="min-w-0 space-y-3 px-5 pb-4">
            {provider.id === "custom" && (
              <div>
                <p className="text-sm font-medium text-primary">{t("API Key")}</p>
                <SecretField
                  id="provider-key-custom"
                  ariaLabel={t("自定义服务 API Key")}
                  typed={draft.providerKeys.custom ?? ""}
                  hint={customHostChanged(draft, settings) ? "" : providerHint(settings, "custom")}
                  placeholder={!isLoopbackUrl(draft.customBaseUrl) ? t("远程服务必填") : t("本机可不填")}
                  onTypedChange={(value) => {
                    setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, custom: value } }));
                    setRowProbeFeedback(null);
                    setProbeSuccess("");
                    setTestedAsrRoute(null);
                    setTestedCleanupRoute(null);
                  }}
                />
                <p className="mt-2 text-xs leading-5 text-tertiary">{t("凭据绑定的自定义服务地址")}: <code className="break-all font-mono">{draft.customBaseUrl || t("尚未设置")}</code></p>
                <button type="button" className={`${compactButtonClass} mt-2`} onClick={() => navigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>
                <p className="mt-2 text-xs leading-5 text-tertiary">{t("远程服务需要填写服务商或 Router 提供的 API Key；只有这台 Mac 上的本机服务可不填。")}</p>
                <p className="mt-2 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
                  <KeyRound size={14} aria-hidden="true" />
                  {t("自定义 API Key 会在保存时写入钥匙串；明确测试时只发送到此处显示的服务地址。")}
                </p>
                {customHostChanged(draft, settings) && !draft.providerKeys.custom?.trim() && !isLoopbackUrl(draft.customBaseUrl) && (
                  <p role="status" className="mt-2 text-xs text-warning-ink">{t("服务地址目标已更改；请填写新地址对应的 API Key。")}</p>
                )}
                {providerConfigured(settings, "custom") && (
                  <div className="mt-3 flex flex-wrap gap-2">
                    <button type="button" onClick={() => setConfirmRemove("custom")} disabled={removing || rowProbing !== null || probing} className={compactDangerButtonClass}>{t("删除密钥")}</button>
                  </div>
                )}
                {rowProbeFeedback?.provider === "custom" && (
                  <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={`mt-2 break-words text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}>
                    {rowProbeFeedback.message}
                  </p>
                )}
                {credentialReadTarget === "custom" && credentialReadMessage && (
                  <p role={credentialReadMessage.tone === "error" ? "alert" : "status"} aria-live="polite" className={`mt-2 break-words text-xs leading-5 ${credentialReadMessage.tone === "error" ? "text-error-ink" : credentialReadMessage.tone === "success" ? "text-success-ink" : "text-secondary"}`}>
                    {credentialReadMessage.text}
                  </p>
                )}
                <div className="mt-3 flex flex-wrap gap-2">
                  {providerProbeStages.map((stage) => {
                    const busy = rowProbing === "custom" && rowProbingStage === stage;
                    return (
                      <button key={stage} type="button" onClick={() => void testConnection("custom", stage)} aria-busy={busy} disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe("custom", stage) || (stage === "cleanup" && !customParametersParse.ok)} className={`${compactButtonClass} min-w-36`}>
                        {busy ? t("测试中…") : t(stage === "asr" ? "测试转写" : "测试整理")}
                      </button>
                    );
                  })}
                </div>
              </div>
            )}
            {provider.id !== "custom" && (
            <div>
              <p className="text-sm font-medium text-primary">{t("API Key")}</p>
              <SecretField
                id={`provider-key-${provider.id}`}
                ariaLabel={`${provider.label} API Key`}
                typed={draft.providerKeys[provider.id] ?? ""}
                hint={providerHint(settings, provider.id)}
                placeholder={provider.allowsEmptyKey ? t("本机可不填") : `${provider.label} key`}
                onTypedChange={(value) => {
                  setDraft({ ...draft, providerKeys: { ...draft.providerKeys, [provider.id]: value } });
                  setRowProbeFeedback(null);
                  setProbeSuccess("");
                  setStageFail((current) => ({ ...current,
                    asr: provider.id === draft.asrProvider ? null : current.asr,
                    cleanup: provider.id === draft.cleanupProvider ? null : current.cleanup,
                  }));
                  if (provider.id === draft.asrProvider) setTestedAsrRoute(null);
                  if (provider.id === draft.cleanupProvider) setTestedCleanupRoute(null);
                }}
              />
              {provider.allowsEmptyKey && (
                <p className="mt-2 text-xs leading-5 text-tertiary">{t("本机地址可不填密钥")}</p>
              )}
              <p className="mt-2 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
                <KeyRound size={14} aria-hidden="true" />
                {provider.id === "soniox"
                  ? t("Soniox 密钥仅保存在钥匙串中；设置时不会连接服务，实时连接只在开始听写时建立。")
                  : t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到所选服务。")}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
              {provider.id === "soniox" ? (
                <p className="text-xs leading-5 text-tertiary">{t("实时连接在听写时建立；API Key 通过下方共用操作保存。")}</p>
              ) : (
                  providerProbeStages.map((stage) => {
                    const busy = rowProbing === provider.id && rowProbingStage === stage;
                    return (
                      <button
                        key={stage}
                        type="button"
                        onClick={() => void testConnection(provider.id, stage)}
                        aria-busy={busy}
                        disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(provider.id, stage)}
                        className={`${compactButtonClass} min-w-36`}
                      >
                        {busy ? t("测试中…") : t(stage === "asr" ? "测试转写" : "测试整理")}
                      </button>
                    );
                  })
                )}
                {providerConfigured(settings, provider.id) && (
                  <button
                    type="button"
                    onClick={() => setConfirmRemove(provider.id)}
                    disabled={removing || rowProbing !== null || probing}
                    className={compactDangerButtonClass}
                  >
                    {t("删除密钥")}
                  </button>
                )}
              </div>
                {rowProbeFeedback?.provider === provider.id && (
                  <p
                    role={rowProbeFeedback.tone === "error" ? "alert" : "status"}
                    className={`mt-2 break-words text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}
                  >
                    {rowProbeFeedback.message}
                  </p>
                )}
              {provider.id === "groq"
                && credentialReadTarget === "groq"
                && credentialReadMessage && (
                <p
                  role={credentialReadMessage.tone === "error" ? "alert" : "status"}
                  aria-live="polite"
                  className={`mt-2 break-words text-xs leading-5 ${credentialReadMessage.tone === "error" ? "text-error-ink" : credentialReadMessage.tone === "success" ? "text-success-ink" : "text-secondary"}`}
                >
                  {credentialReadMessage.text}
                </p>
              )}
            </div>
            )}
        </fieldset>
      </div>
    );
  };

  const activePage = ENGINE_PAGES.find((item) => item.id === page) ?? ENGINE_PAGES[0];
  const renderOnDeviceModelRow = (model: OnDeviceModelStatus, legacy = false) => {
    const downloading = model.state === "downloading";
    const unsupported = !legacy && (!model.platform_supported || model.runtime_status === "unsupported_platform");
    const actionBusy = onDeviceActionId === model.id;
    const savedSelected = currentAsrProvider === "on_device" && savedAsrModel === model.id;
    const draftSelected = draft.asrProvider === "on_device" && draft.asrModel === model.id && !savedSelected;
    const downloadCancelled = model.error?.trim().toLowerCase() === "download cancelled";
    const downloadAvailable = model.download_available === true && !isRetiredOnDeviceModel(model.id);
    const retired = !downloadAvailable || isRetiredOnDeviceModel(model.id);
    const memoryCompatibilityIssue = onDeviceMemoryCompatibilityCopy(model.memory_compatibility_error, t);
    const canDownloadOnThisDevice = downloadAvailable && !unsupported && !memoryCompatibilityIssue;
    const runtimeLabel = model.runtime_status === "loading"
      ? t("加载中")
      : model.loaded || model.runtime_status === "loaded"
        ? t("已加载")
        : null;
    const runtimeIssue = legacy
      ? t("SenseVoice 是旧版文件保留项；没有 MLX 推理支持，不能用于本机听写。")
      : !model.platform_supported || model.runtime_status === "unsupported_platform"
        ? t("需要 Apple Silicon 与 macOS 14 或更新版本")
        : model.runtime_status === "sidecar_missing"
          ? t("本机推理组件未安装")
          : model.runtime_status === "runtime_failed"
            ? t("本机运行时启动失败")
            : null;
    const showRuntimeIssue = legacy || unsupported || Boolean(runtimeIssue && (savedSelected || draftSelected));
    const detailId = `local-asr-details-${model.id}`;
    const summary = [
      model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      onDeviceFileSummaryCopy(model.state, t),
      runtimeLabel,
      downloadCancelled ? t("已取消") : null,
    ].filter(Boolean).join(" · ");

    return (
      <div id={`local-asr-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{summary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {model.id === "qwen3-asr-0.6b" && <SettingsStatus label={t("默认选项")} tone="neutral" />}
                {canDownloadOnThisDevice && <SettingsStatus label={t("可下载")} tone="neutral" />}
                {!downloadAvailable && (savedSelected || draftSelected) && <SettingsStatus label={t("旧配置保留")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("本机运行时状态")}: {onDeviceRuntimeCopy(model.runtime_status, t)}</p>
                    <p>{t("运行要求")}: {t("Apple Silicon 与 macOS 14 或更新版本")}</p>
                    {model.language_note && <p>{t("语言能力说明")}: {model.language_note}</p>}
                    {model.revision && <p className="break-all">{t("模型版本")}: {model.revision}</p>}
                    {model.file_count > 0 && <p>{t("模型文件数")}: {model.file_count}</p>}
                    {model.sha256 && <p className="break-all">SHA-256: {model.sha256}</p>}
                  </div>
                </details>
              </div>
              {downloading && model.bytes > 0 && (
                <progress
                  aria-label={t("模型下载进度")}
                  className="h-1.5 w-full accent-accent"
                  max={model.bytes}
                  value={Math.min(model.bytes, model.downloaded_bytes)}
                />
              )}
              {showRuntimeIssue && runtimeIssue && <p role={legacy || retired || unsupported ? "note" : undefined} className="text-warning-ink">{runtimeIssue}</p>}
              {memoryCompatibilityIssue && <p role="note" className="text-warning-ink">{memoryCompatibilityIssue}</p>}
              {model.id === "cohere-transcribe-2b" && <p role="note" className="text-warning-ink">{t("需固定选择中文或 English；不支持自动语言识别。")}</p>}
              {model.error_category === "disk_full" && <p className="text-error-ink">{t("检查可用空间后重试下载。")}</p>}
              {model.error && !downloadCancelled && <p role="alert" className="break-words text-error-ink">{model.error}</p>}
              {onDeviceActionError?.id === model.id && <p role="alert" className="text-error-ink">{onDeviceActionError.message}</p>}
            </div>
          )}
        >
              {downloading ? <button type="button" disabled={actionBusy} onClick={() => void runOnDeviceAction(model.id, "cancel")} className={compactButtonClass}>{t("取消下载")}</button>
            : model.runtime_status === "loading" ? <button type="button" disabled={actionBusy} onClick={() => void runOnDeviceAction(model.id, "cancel_load")} className={compactButtonClass}>{actionBusy ? t("正在取消…") : t("取消模型加载")}</button>
                : model.state === "ready" || (retired && model.state !== "missing") ? <button type="button" aria-label={`${t("删除模型")}: ${model.label}`} disabled={actionBusy} onClick={() => setConfirmDeleteModel(model.id)} className={compactDangerButtonClass}>{t("删除模型")}</button>
                : retired ? null
                : <button type="button" disabled={actionBusy || unsupported || Boolean(memoryCompatibilityIssue)} onClick={() => void runOnDeviceAction(model.id, "download")} className={compactButtonClass}>{unsupported ? t("当前设备不支持") : actionBusy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
        </SettingsRow>
      </div>
    );
  };

  const renderLocalCleanupModelRow = (model: LocalCleanupModelStatus) => {
    const savedSelected = settings.cleanup_enabled
      && currentCleanupProvider === "on_device_llama_cpp"
      && savedCleanupModel === model.id;
    const draftSelected = draftCleanupEnabled
      && draft.cleanupProvider === "on_device_llama_cpp"
      && draft.cleanupModel === model.id
      && !savedSelected;
    const selected = savedSelected || draftSelected;
    const busy = Boolean(localCleanupModelBusy[model.id]);
    const downloading = model.operation === "download" || model.state === "downloading";
    const deleting = model.operation === "delete" || model.state === "deleting";
    const detailId = `local-cleanup-details-${model.id}`;
    const transferBytes = model.bytes_total > 0 ? model.bytes_total : model.bytes;
    const partialOnly = !model.managed && Boolean(model.partial_managed);
    const statusSummary = [
      partialOnly && !downloading
        ? `${formatModelBytes(model.bytes_done)} / ${formatModelBytes(transferBytes)}`
        : model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      deleting ? t("正在删除") : localCleanupModelStateCopy(model.state, model.verified, t, model.partial_managed),
      !model.operation && /cancel/i.test(`${model.error ?? ""} ${model.last_operation_error ?? ""}`) ? t("已取消") : null,
    ].filter(Boolean).join(" · ");
    const operationErrors = [...new Set([model.error, model.last_operation_error].filter((error): error is string => Boolean(error?.trim())))];
    const canOfferDownload = model.download_available === true
      && (model.state === "missing" || model.state === "corrupt");

    return (
      <div id={`local-cleanup-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row={model.id} className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{statusSummary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {isSelectableLocalCleanupModel(model) && <SettingsStatus label={t("可用于服务配置")} tone="success" />}
                {!model.compatible && <SettingsStatus label={t("当前设备不兼容")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationErrors.length > 0 || !model.compatible ? t("查看技术信息与错误") : t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("用途")}: {t("通用文字整理")}</p>
                    <p>{t("运行时")}: {t("由 VoiceFlow 随包提供的 llama.cpp")}</p>
                    <p>{t("最低系统与内存要求")}: {t("该模型最低支持档尚待实机验收；兼容状态按本机检查结果显示。")}</p>
                    <p>{t("目标验收档")}: {t("Apple M1 · 16 GB；待实机验证")}</p>
                    <p>{t("文件大小")}: {formatModelBytes(model.bytes)}</p>
                    <p>{t("文件状态")}: {localCleanupModelStateCopy(model.state, model.verified, t, model.partial_managed)}</p>
                    <p>{t("文件校验")}: {model.verified ? t("已通过") : t("尚未通过")}</p>
                    <p>{t("设备兼容性")}: {model.compatible ? t("兼容检查通过") : localCleanupCompatibilityCopy(model.compatibility_error, t)}</p>
                    <p>{t("来源仓库")}: {model.repository}</p>
                    <p className="break-all">{t("固定版本")}: {model.revision}</p>
                    {model.sha256 && <p className="break-all">SHA-256: {model.sha256}</p>}
                    {model.compatibility_error && <p>{t("兼容性错误代码")}: {model.compatibility_error}</p>}
                    {operationErrors.map((error) => <p key={error} className="break-words text-error-ink">{error}</p>)}
                  </div>
                </details>
              </div>
              {downloading && transferBytes > 0 && (
                <div className="space-y-1">
                  <p role="status" className="text-xs text-tertiary">{formatModelBytes(model.bytes_done)} / {formatModelBytes(transferBytes)}</p>
                  <progress
                    aria-label={`${t("模型下载进度")}: ${model.label}`}
                    className="h-1.5 w-full accent-accent"
                    max={transferBytes}
                    value={Math.min(transferBytes, model.bytes_done)}
                  />
                </div>
              )}
              {partialOnly && !downloading && <p role="note" className="text-secondary">{t(model.download_available ? "下载已中断；可以重新下载或清理部分文件。" : "此版本不再提供重新下载；可以清理未完成文件。")}</p>}
              {!model.compatible && <p role="note" className="text-warning-ink">{localCleanupCompatibilityCopy(model.compatibility_error, t)}</p>}
              {operationErrors.map((error) => <p key={error} role="alert" className="break-words text-error-ink">{error}</p>)}
              {localCleanupModelActionErrors[model.id] && <p role="alert" className="break-words text-error-ink">{localCleanupModelActionErrors[model.id]}</p>}
              {selected && <p role="note" className="text-warning-ink">{t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")}</p>}
            </div>
          )}
        >
          {downloading && <button type="button" disabled={busy} onClick={() => void runLocalCleanupModelAction(model.id, "cancel")} className={compactButtonClass}>{busy ? t("处理中…") : t("取消下载")}</button>}
          {!downloading && !deleting && canOfferDownload && <button type="button" disabled={busy || model.downloadable !== true} onClick={() => void runLocalCleanupModelAction(model.id, "download")} className={compactButtonClass}>{busy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
          {!downloading && !deleting && (model.managed || model.partial_managed) && <button type="button" aria-label={`${t(partialOnly ? "清理未完成文件" : "删除模型")}: ${model.label}`} disabled={busy || selected} onClick={() => setConfirmDeleteLocalCleanupModel(model.id)} className={compactDangerButtonClass}>{t(partialOnly ? "清理未完成文件" : "删除模型")}</button>}
        </SettingsRow>
      </div>
    );
  };

  const renderMlxCleanupModelRow = (model: MlxCleanupModelStatus) => {
    const savedSelected = settings.cleanup_enabled
      && currentCleanupProvider === "on_device_mlx"
      && savedCleanupModel === model.id;
    const draftSelected = draftCleanupEnabled
      && draft.cleanupProvider === "on_device_mlx"
      && draft.cleanupModel === model.id
      && !savedSelected;
    const busy = Boolean(mlxCleanupModelBusy[model.id]);
    const downloading = model.operation === "download" || ["downloading", "canceling"].includes(model.state);
    const canceling = model.state === "canceling";
    const verifying = model.operation === "verify";
    const operationActive = isMlxCleanupModelOperationActive(model);
    const partialOnly = !model.managed && model.partial_managed;
    const transferBytes = model.bytes_total > 0 ? model.bytes_total : model.bytes;
    const canOfferDownload = model.download_available === true
      && ["missing", "corrupt", "error"].includes(model.state);
    const canVerify = model.state === "installed" && model.managed && !model.verified
      && (!model.operation || model.operation === "verify");
    const showProgress = downloading || verifying;
    const operationErrors = [...new Set([model.error, model.last_operation_error]
      .filter((error): error is string => Boolean(error?.trim()) && !isCancellationError(error)))];
    const operationWasCancelled = !isMlxCleanupModelOperationActive(model)
      && isCancellationError(model.error ?? model.last_operation_error);
    const statusSummary = [
      model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      mlxCleanupModelStateCopy(model, t),
      operationWasCancelled ? t("已取消") : null,
    ].filter(Boolean).join(" · ");
    const language = mlxCleanupLanguageCopy(model.language);
    const detailId = `mlx-cleanup-details-${model.id}`;

    return (
      <div id={`mlx-cleanup-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row={model.id} className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{statusSummary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {model.selectable && <SettingsStatus label={t("可用于服务配置")} tone="success" />}
                {!model.compatible && <SettingsStatus label={t("当前设备不兼容")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationErrors.length > 0 || !model.compatible ? t("查看技术信息与错误") : t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("用途")}: {t("通用文字整理")}</p>
                    {language && <p>{t("语言与模式")}: {language}</p>}
                    <p>{t("运行时")}: VoiceFlow MLX</p>
                    <p>{t("文件大小")}: {model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知")}</p>
                    <p>{t("文件状态")}: {mlxCleanupModelStateCopy(model, t)}</p>
                    <p>{t("文件校验")}: {model.verified ? t("已通过") : t("尚未通过")}</p>
                    <p>{t("设备兼容性")}: {model.compatible ? t("兼容检查通过") : mlxCleanupCompatibilityCopy(model.compatibility_error, t)}</p>
                    <p>{t("来源仓库")}: {model.repository}</p>
                    <p className="break-all">{t("固定版本")}: {model.revision}</p>
                    {model.compatibility_error && <p>{t("兼容性错误代码")}: {model.compatibility_error}</p>}
                    {operationErrors.map((error) => <p key={error} className="break-words text-error-ink">{error}</p>)}
                  </div>
                </details>
              </div>
              {showProgress && (verifying || transferBytes > 0) && (
                <div className="space-y-1">
                  {verifying ? (
                    <>
                      <p role="status" className="text-xs text-tertiary">{t("正在校验模型文件")}</p>
                      <progress aria-label={`${t("模型校验进度")}: ${model.label}`} className="h-1.5 w-full accent-accent" />
                    </>
                  ) : (
                    <>
                      <p role="status" className="text-xs text-tertiary">{formatModelBytes(model.bytes_done)} / {formatModelBytes(transferBytes)}</p>
                      <progress
                        aria-label={`${t("模型下载进度")}: ${model.label}`}
                        className="h-1.5 w-full accent-accent"
                        max={transferBytes}
                        value={Math.min(transferBytes, model.bytes_done)}
                      />
                    </>
                  )}
                </div>
              )}
              {partialOnly && !downloading && <p role="note" className="text-secondary">{t(model.download_available ? "下载已中断；可以重新下载或清理部分文件。" : "此版本不再提供重新下载；可以清理未完成文件。")}</p>}
              {!model.compatible && <p role="note" className="text-warning-ink">{mlxCleanupCompatibilityCopy(model.compatibility_error, t)}</p>}
              {model.download_available && !model.downloadable && canOfferDownload && model.compatible && <p role="note" className="text-warning-ink">{t("下载暂不可用")}</p>}
              {!model.download_available && (model.managed || model.partial_managed || savedSelected) && <p role="note" className="text-tertiary">{t("仅保留已安装文件管理和当前已保存路线；不提供新的下载或选择。")}</p>}
              {operationErrors.map((error) => <p key={error} role="alert" className="break-words text-error-ink">{error}</p>)}
              {mlxCleanupModelActionErrors[model.id] && <p role="alert" className="break-words text-error-ink">{mlxCleanupModelActionErrors[model.id]}</p>}
              {savedSelected && <p role="note" className="text-warning-ink">{t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")}</p>}
            </div>
          )}
        >
          {downloading && <button type="button" disabled={busy || canceling} onClick={() => void runMlxCleanupModelAction(model.id, "cancel")} className={compactButtonClass}>{busy || canceling ? t("正在取消下载") : t("取消下载")}</button>}
          {canVerify && <button type="button" disabled={busy || verifying} onClick={() => void runMlxCleanupModelAction(model.id, "verify")} className={compactButtonClass}>{busy || verifying ? t("正在校验…") : t("校验模型")}</button>}
          {!operationActive && canOfferDownload && <button type="button" disabled={busy || !model.downloadable} onClick={() => void runMlxCleanupModelAction(model.id, "download")} className={compactButtonClass}>{busy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
          {!operationActive && (model.managed || model.partial_managed) && <button type="button" aria-label={`${t(partialOnly ? "清理未完成文件" : "删除模型")}: ${model.label}`} disabled={busy || savedSelected} onClick={() => setConfirmDeleteMlxCleanupModel(model.id)} className={compactDangerButtonClass}>{t(partialOnly ? "清理未完成文件" : "删除模型")}</button>}
        </SettingsRow>
      </div>
    );
  };
  return (
    <SettingsShell className={showEngineFooter ? "vf-settings-shell--flush-bottom" : ""}>
      <div className={`vf-engine-settings${showEngineFooter ? " vf-engine-settings--with-footer" : ""}`} onFocusCapture={(event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) return;
        window.requestAnimationFrame(() => {
          if (document.activeElement === target) keepEngineControlInsideRails(target);
        });
      }}>
      <SettingsPageHeader
        title={t(activePage.label)}
        description={t(activePage.description)}
        saveHint={page === "service" ? t("转写和整理按阶段显示当前路线与检查状态；草稿在本窗口切页后保留。") : undefined}
      />
      {settings.strict_offline_enabled && (
        <div role="status" className="flex flex-wrap items-center justify-between gap-2 border-b border-warning/30 bg-warning/5 px-5 py-2 text-xs leading-5 text-warning-ink">
          <span>{t("严格离线模式已开启")}</span>
          <button type="button" className={compactButtonClass} onClick={() => navigateToPage("advanced", "strict-offline-toggle")}>{t("查看策略")}</button>
        </div>
      )}
      <div
        id="engine-panel-service"
        role="region"
        aria-label={t("服务配置")}
        hidden={page !== "service"}
        inert={page !== "service"}
        className="vf-engine-page-content"
      >
        <SettingsGroup title={t("当前路线与草稿")} description={t(pendingChanges ? "有待应用的更改；当前听写仍使用已保存路线。" : "调整服务或模型后，可分别测试，再保存并使用。")}>
        <fieldset aria-label={t("当前生效路线")} disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="min-w-0">
        <div className="px-5 py-4">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-primary">{t("转写")}</p>
            <SettingsStatus label={asrStatus.label} tone={asrStatus.tone} />
          </div>
          <p className="mt-1 text-xs text-secondary">
            {t(savedAsrBlocked ? "已保存，当前被阻止" : "当前使用")} · {providerById(currentAsrProvider)?.label} · {settings.asr_model || defaultModel(currentAsrProvider, "asr")}
          </p>
          {savedAsrBlocked && <p className="mt-1 flex flex-wrap items-center gap-2 text-xs text-warning-ink"><span>{t("严格离线模式已阻止此路线")}</span><button type="button" className={compactButtonClass} onClick={() => navigateToPage("service", "engine-asr-provider")}>{t("修改路线")}</button></p>}
          {(draft.asrProvider !== currentAsrProvider || draft.asrModel !== (settings.asr_model || defaultModel(currentAsrProvider, "asr"))) && (
            <p className="mt-1 text-xs text-warning-ink">{t("待应用")} · {providerById(draft.asrProvider)?.label} · {draft.asrModel}</p>
          )}
          <div className="mt-2 flex flex-wrap gap-2">
            {providerHasCredentialInput(draft.asrProvider) && (
              <button type="button" className={compactButtonClass} onClick={() => navigateToPage("api_key", `provider-key-${draft.asrProvider}`)}>{t("管理凭据")}</button>
            )}
            {draft.asrProvider === "on_device" && <button type="button" className={compactButtonClass} onClick={() => navigateToPage("local_models", selectedOnDeviceModel ? `local-asr-model-${selectedOnDeviceModel.id}` : "local-asr-models")}>{t("管理模型")}</button>}
            {draft.asrProvider === "custom" && <button type="button" className={compactButtonClass} onClick={() => navigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>}
            {draft.asrProvider === "soniox" ? (
              <p className="text-xs leading-5 text-tertiary">{t("实时连接在听写时建立；API Key 通过下方共用操作保存。")}</p>
            ) : (
              <button
                type="button"
                onClick={() => void testConnection(draft.asrProvider, "asr")}
                aria-busy={rowProbing === draft.asrProvider && rowProbingStage === "asr"}
                disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(draft.asrProvider, "asr")}
                className={compactButtonClass}
              >
                {rowProbing === draft.asrProvider && rowProbingStage === "asr" ? t("测试中…") : t("测试转写")}
              </button>
            )}
          </div>
          {rowProbeFeedback?.provider === draft.asrProvider && rowProbeFeedback.stage === "asr" && (
            <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={`mt-2 text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}>
              {rowProbeFeedback.message}
            </p>
          )}
          <div className="vf-service-fields mt-3 flex w-full flex-wrap gap-2">
            <Select id="engine-asr-provider" aria-label={t("转写服务")} value={draft.asrProvider} onValueChange={(value) => changeAsrProvider(value)} className={`${controlClass} w-40 max-w-full`}>
              <optgroup label={t("云端服务")}>{cloudAsrOptions.map(renderAsrOption)}</optgroup>
              <optgroup label={t("本机服务")}>{localAsrOptions.map(renderAsrOption)}</optgroup>
              <optgroup label={t("自定义服务")}>{customAsrOptions.map(renderAsrOption)}</optgroup>
            </Select>
            <ModelControl
              id={draft.asrProvider}
              side="asr"
              value={draft.asrModel}
              ariaLabel={t("ASR 模型")}
              onChange={changeAsrModel}
              deviceModels={onDeviceModels}
              optionsOverride={draft.asrProvider === "on_device" ? onDeviceAsrModelOptions : undefined}
            />
          </div>
          {draft.asrProvider === "local_whisper" && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <p className="text-xs leading-5 text-tertiary">{t("本机 Whisper 走本机 HTTP；先自行运行 whisper.cpp 或 speaches。")}</p>
              <button type="button" className={compactButtonClass} onClick={() => navigateToPage("advanced", "advanced-local-whisper-url")}>{t("管理连接地址")}</button>
            </div>
          )}
          {draft.asrProvider === "on_device" && (
            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs leading-5">
              <span role={onDeviceModelReadError ? "alert" : "status"} className={onDeviceModelReadError ? "text-warning-ink" : "text-secondary"}>
                {selectedOnDeviceModel
                  ? `${onDeviceStateCopy(selectedOnDeviceModel.state, t)} · ${onDeviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t)}`
                  : onDeviceModelsLoading
                    ? t("正在读取本机模型状态…")
                    : onDeviceModelReadError ?? t("本机模型状态尚未读取")}
              </span>
              {selectedOnDeviceModel?.error_category === "disk_full" && <span className="text-error-ink">{t("检查可用空间后重试下载。")}</span>}
              {selectedOnDeviceModel?.error && <span className="text-tertiary">{selectedOnDeviceModel.error}</span>}
              <button type="button" className={compactButtonClass} onClick={() => navigateToPage("local_models", "local-asr-models")}>{t("管理模型")}</button>
            </div>
          )}
          {draft.asrProvider !== "on_device" && (
            <SettingsDisclosure variant="plain" title={t("模型使用说明")}>
            <div role="note" className="mt-3 space-y-1 rounded-lg bg-elevated px-3 py-2.5 text-xs leading-5 text-tertiary">
              <p>{asrLanguageDescription(asrProfile, settings.language, t)}</p>
              {asrProfile?.requestNote && <p>{t(asrProfile.requestNote)}</p>}
              {asrProfile?.responseNote && <p>{t(asrProfile.responseNote)}</p>}
              {asrProfile?.limitNote && <p>{t(asrProfile.limitNote)}</p>}
              {asrProfile?.contextNote && <p>{t(asrProfile.contextNote)}</p>}
              {asrProfile?.capabilityNote && <p>{t(asrProfile.capabilityNote)}</p>}
              <p>{t("转写请求会发送到当前配置的服务；不会静默切换到其他服务。")}</p>
            </div>
            </SettingsDisclosure>
          )}
          {draft.asrProvider !== "on_device" && asrProfile?.availabilityNote && <p className="mt-3 text-xs leading-5 text-warning-ink">{t(asrProfile.availabilityNote)}</p>}
        </div>
        <div className="px-5 py-4">
          <div className="flex items-center justify-between gap-3">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <p className="text-sm font-medium text-primary">{t("AI 文字整理")}</p>
              <SettingsStatus label={cleanupStatus.label} tone={cleanupStatus.tone} />
            </div>
            <Toggle id="engine-cleanup-toggle" checked={draftCleanupEnabled} onChange={(checked) => { setDraftCleanupEnabled(checked); setProbeSuccess(""); }} label={t("AI 文字整理")} />
          </div>
          <p className="mt-1 text-xs text-secondary">
            {t(savedCleanupBlocked ? "已保存，当前被阻止" : "当前使用")} · {settings.cleanup_enabled
              ? `${providerById(currentCleanupProvider)?.label} · ${settings.cleanup_model || defaultModel(currentCleanupProvider, "llm")}`
              : t("关闭 · 只用本地规则")}
          </p>
          {savedCleanupBlocked && <p className="mt-1 flex flex-wrap items-center gap-2 text-xs text-warning-ink"><span>{t("严格离线模式已阻止此路线")}</span><button type="button" className={compactButtonClass} onClick={() => navigateToPage("service", "engine-cleanup-provider")}>{t("修改路线")}</button></p>}
          {draftCleanupEnabled !== settings.cleanup_enabled || (draftCleanupEnabled && (draft.cleanupProvider !== currentCleanupProvider || draft.cleanupModel !== (settings.cleanup_model || defaultModel(currentCleanupProvider, "llm")))) ? (
            <p className="mt-1 text-xs text-warning-ink">{t("待应用")} · {draftCleanupEnabled ? `${providerById(draft.cleanupProvider)?.label} · ${draft.cleanupModel}` : t("关闭 · 只用本地规则")}</p>
          ) : null}
          <div className="mt-2 flex flex-wrap gap-2">
            {draftCleanupEnabled && providerHasCredentialInput(draft.cleanupProvider) && (
              <button type="button" className={compactButtonClass} onClick={() => navigateToPage("api_key", `provider-key-${draft.cleanupProvider}`)}>{t("管理凭据")}</button>
            )}
            {draft.cleanupProvider === "on_device_llama_cpp" && <button type="button" className={compactButtonClass} onClick={() => navigateToPage("local_models", "local-cleanup-models")}>{t("管理模型")}</button>}
            {draft.cleanupProvider === "on_device_mlx" && <button type="button" className={compactButtonClass} onClick={() => navigateToPage("local_models", mlxCleanupManageTarget)}>{t("管理模型")}</button>}
            {draft.cleanupProvider === "custom" && <button type="button" className={compactButtonClass} onClick={() => navigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>}
            {draftCleanupEnabled && (
              <button
                type="button"
                onClick={() => void testConnection(draft.cleanupProvider, "cleanup")}
                aria-busy={rowProbing === draft.cleanupProvider && rowProbingStage === "cleanup"}
                disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(draft.cleanupProvider, "cleanup") || (draft.cleanupProvider === "custom" && !customParametersParse.ok) || (draft.cleanupProvider === "on_device_llama_cpp" && !draftLocalCleanupModelReady)}
                className={compactButtonClass}
              >
                {rowProbing === draft.cleanupProvider && rowProbingStage === "cleanup" ? t("测试中…") : t("测试整理")}
              </button>
            )}
          </div>
          {draftCleanupEnabled && rowProbeFeedback?.provider === draft.cleanupProvider && rowProbeFeedback.stage === "cleanup" && (
            <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={`mt-2 text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}>
              {rowProbeFeedback.message}
            </p>
          )}
          {draftCleanupEnabled && (
            <div className="vf-service-fields mt-3 flex w-full flex-wrap gap-2">
              <Select id="engine-cleanup-provider" aria-label={t("整理服务")} value={draft.cleanupProvider} onValueChange={(value) => changeCleanupProvider(value)} className={`${controlClass} w-40 max-w-full`}>
                <optgroup label={t("云端服务")}>{cloudCleanupOptions.map(renderCleanupOption)}</optgroup>
                <optgroup label={t("本机服务")}>{localCleanupOptions.map(renderCleanupOption)}</optgroup>
                <optgroup label={t("自定义服务")}>{customCleanupOptions.map(renderCleanupOption)}</optgroup>
              </Select>
              <ModelControl
                id={draft.cleanupProvider}
                side="llm"
                value={draft.cleanupModel}
                ariaLabel={t("AI 文字整理模型")}
                onChange={changeCleanupModel}
                optionsOverride={draft.cleanupProvider === "on_device_llama_cpp"
                  ? localCleanupModelOptions
                  : draft.cleanupProvider === "on_device_mlx" ? mlxCleanupModelOptions : undefined}
              />
            </div>
          )}
          {draftCleanupEnabled && draft.cleanupProvider === "on_device_llama_cpp" && (
            <div className="mt-2 space-y-1 text-xs leading-5 text-tertiary">
              <p>{t("只列出已下载、校验且兼容本机的模型；下载不会自动切换当前使用模型。")}</p>
              {selectedLocalCleanupModel?.compatible === false && <p role="note" className="text-warning-ink">{localCleanupCompatibilityCopy(selectedLocalCleanupModel.compatibility_error, t)}</p>}
              {localCleanupModelsReadError && <p role="alert" className="text-error-ink">{localCleanupModelsReadError}</p>}
            </div>
          )}
          {draft.asrProvider === "assemblyai" && (
            <div role="note" className="mt-2 rounded-lg bg-elevated px-3 py-2.5 text-xs leading-5 text-tertiary">
              <p>{t("启用 AI 整理且满足条件的 120 秒内 Dictation 会提供 AssemblyAI 整理候选；不需要共同整理密钥，候选通过本地保护检查后才会采用。")}</p>
              <span className="mt-1 block">{t("候选缺失或失败时，只有已选共同整理服务的密钥已配置才会再请求一次整理；否则保留原始转写并使用本地保护回退。保护检查拒绝候选时也保留原稿，不再发起第二次整理。")}</span>
              <span className="mt-1 block">{t("AI Off、本地-only 和超过 120 秒的录音使用 AssemblyAI Sync 原始转写。长录音开启云端整理时需要已选的共同整理服务；服务不可用时保留原文并回退到本地或原始结果。保存密钥不代表服务访问已验证。")}</span>
            </div>
          )}
          {draft.cleanupProvider === "ollama" && (
            <div className="mt-3 rounded-lg bg-elevated px-3 py-3 text-xs leading-5 text-tertiary">
              <p className="font-medium text-primary">{t("本机整理依赖")}: Ollama · qwen3.5:4b</p>
              {localCleanupStatus && <p role="status" className={localCleanupStatus.available ? "text-success-ink" : "text-warning-ink"}>{localCleanupStatus.available ? t("本机整理服务可用") : t("本机整理服务不可用")} · {localCleanupStatus.status}</p>}
              {localCleanupError && <p role="alert" className="text-error-ink">{localCleanupError}</p>}
              <button type="button" className={`${compactButtonClass} mt-2`} onClick={() => navigateToPage("advanced", "advanced-ollama-url")}>{t("管理本机连接")}</button>
            </div>
          )}
          {draft.cleanupProvider === "on_device_mlx" && (
            <div className="mt-3 space-y-1 text-xs leading-5 text-tertiary">
              <p>{t("只列出可用于服务配置的本机 MLX 模型；下载不会自动切换当前模型。")}</p>
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-secondary">{draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
                  ? cleanupModelStoreStatus ? cleanupModelStoreStateCopy(cleanupModelStoreStatus.state, t) : t("本机模型状态尚未读取")
                  : selectedMlxCleanupModel ? mlxCleanupModelStateCopy(selectedMlxCleanupModel, t) : t("本机模型状态尚未读取")}</span>
                {draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID && cleanupModelStoreStatus?.operation && cleanupModelStoreStatus.operation !== "delete"
                  ? <span>{formatModelBytes(cleanupModelStoreStatus.bytes_done)} / {formatModelBytes(cleanupModelStoreStatus.bytes_total)}</span>
                  : null}
                {selectedMlxCleanupLanguage && <span>{t("语言与模式")}: {selectedMlxCleanupLanguage}</span>}
              </div>
              {selectedMlxCleanupModel?.compatible === false && <p role="note" className="text-warning-ink">{mlxCleanupCompatibilityCopy(selectedMlxCleanupModel.compatibility_error, t)}</p>}
              {mlxCleanupModelsReadError && <p role="alert" className="text-error-ink">{mlxCleanupModelsReadError}</p>}
              <button type="button" className={compactButtonClass} onClick={() => navigateToPage("local_models", mlxCleanupManageTarget)}>{t("管理模型")}</button>
            </div>
          )}
        </div>
        </fieldset>
        {probeSuccess === "saved" && <p role="status" className="px-5 py-3 text-xs leading-5 text-success-ink">{t("设置已保存")}</p>}
      </SettingsGroup>
      </div>
      <div
        id="engine-panel-api_key"
        role="region"
        aria-label={t("API Key")}
        hidden={page !== "api_key"}
        inert={page !== "api_key"}
        className="vf-engine-page-content"
      >
        <SettingsGroup title={t("服务商凭据")} description={t("密钥按服务商保存在钥匙串中；转写和整理共用同一服务商密钥。")}>
          <div id="provider-list" className="vf-provider-list flex flex-col">
            {PROVIDERS
              .filter((provider) => providerHasCredentialInput(provider.id))
              .slice()
              .sort((left, right) => {
                const priority = (provider: typeof left) => {
                  if (currentAsrProvider === provider.id || settings.cleanup_enabled && currentCleanupProvider === provider.id) return 0;
                  if (draft.asrProvider === provider.id || draftCleanupEnabled && draft.cleanupProvider === provider.id) return 1;
                  if (providerConfigured(settings, provider.id)) return 2;
                  return 3;
                };
                return priority(left) - priority(right);
              })
              .map(renderProvider)}
          </div>
        </SettingsGroup>
      </div>
      <div
        id="engine-panel-local_models"
        role="region"
        aria-label={t("本地模型")}
        hidden={page !== "local_models"}
        inert={page !== "local_models"}
        className="vf-engine-page-content"
      >
        <SettingsGroup title={t("本机转写")} description={t("把录音变成文字。")}>
          <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="w-full min-w-0 border-0 p-0">
            <div id="local-asr-models" tabIndex={-1} className="divide-y divide-border outline-none">
              {onDeviceModelReadError && onDeviceModels.length > 0 && (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                  <p role="alert" className="text-xs leading-5 text-error-ink">{onDeviceModelReadError}</p>
                  <button type="button" onClick={() => void refreshOnDeviceModels()} className={compactButtonClass}>{t("重新读取模型状态")}</button>
                </div>
              )}
              {onDeviceModelsLoading && onDeviceModels.length === 0 ? (
                <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机模型状态…")}</p>
              ) : onDeviceModelReadError && onDeviceModels.length === 0 ? (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p role="alert" className="text-xs leading-5 text-error-ink">{onDeviceModelReadError}</p>
                  <button type="button" onClick={() => void refreshOnDeviceModels()} className={compactButtonClass}>{t("重新读取模型状态")}</button>
                </div>
              ) : onDeviceModels.length === 0 ? (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p className="text-xs leading-5 text-tertiary">{t("本机模型状态尚未读取")}</p>
                  <button type="button" onClick={() => void refreshOnDeviceModels()} className={compactButtonClass}>{t("重新读取模型状态")}</button>
                </div>
              ) : (
                <>
                  {onDeviceModels.filter((model) => model.id !== "sensevoice-small" && (model.download_available === true
                    || model.state !== "missing"
                    || (currentAsrProvider === "on_device" && savedAsrModel === model.id)
                    || (draft.asrProvider === "on_device" && draft.asrModel === model.id))).map((model) => renderOnDeviceModelRow(model))}
                  {onDeviceModels.some((model) => model.id !== "sensevoice-small" && !model.platform_supported) && (
                    <p role="note" className="px-5 py-2 text-xs leading-5 text-warning-ink">{t("本机转写模型需要 Apple Silicon 与 macOS 14 或更新版本。")}</p>
                  )}
                  {legacyModel && (legacyModel.state !== "missing" || legacyModelSavedSelected || legacyModelDraftSelected) && (
                    <>
                      {(legacyModelSavedSelected || legacyModelDraftSelected) && (
                        <p role="note" className="px-5 py-2 text-xs leading-5 text-warning-ink">
                          {legacyModelSavedSelected ? t("服务配置仍选用了旧版 SenseVoice；此模型不能用于本机听写。") : t("草稿选用了旧版 SenseVoice；此模型不能用于本机听写。")}
                        </p>
                      )}
                      <details
                        open={legacyModelsOpen || legacyModelShouldOpen}
                        onToggle={(event) => setLegacyModelsOpen(event.currentTarget.open)}
                        className="px-5 py-1"
                      >
                        <summary className={`cursor-pointer py-2 text-xs font-medium text-secondary ${focusRingClass}`}>{t("旧版文件管理")}</summary>
                        <div className="-mx-5 border-t border-border">{renderOnDeviceModelRow(legacyModel, true)}</div>
                      </details>
                    </>
                  )}
                </>
              )}
              {onDeviceActionError && !onDeviceModels.some((model) => model.id === onDeviceActionError.id) && <p role="alert" className="px-5 py-3 text-xs text-error-ink">{onDeviceActionError.message}</p>}
            </div>
          </fieldset>
        </SettingsGroup>
        <SettingsGroup title={t("本机文字整理")} description={t("在这台 Mac 上调整转写文字和排版。")}>
          <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="w-full min-w-0 border-0 p-0">
            <div id="local-mlx-cleanup-models" tabIndex={-1} className="divide-y divide-border outline-none">
              {mlxCleanupModelsReadError && visibleMlxCleanupModels.length > 0 && (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                  <p role="alert" className="text-xs leading-5 text-error-ink">{mlxCleanupModelsReadError}</p>
                  <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => void refreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("重新读取 MLX 整理模型状态")}</button>
                </div>
              )}
              {mlxCleanupModelsLoading && visibleMlxCleanupModels.length === 0 ? (
                <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机 MLX 整理模型状态…")}</p>
              ) : mlxCleanupModelsReadError && visibleMlxCleanupModels.length === 0 ? (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p role="alert" className="text-xs leading-5 text-error-ink">{mlxCleanupModelsReadError}</p>
                  <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => void refreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("刷新 MLX 整理模型状态")}</button>
                </div>
              ) : visibleMlxCleanupModels.length === 0 ? (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p role="status" className="text-xs leading-5 text-tertiary">{t("没有其他可下载或需要管理的本机 MLX 整理模型。")}</p>
                  <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => void refreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("刷新 MLX 整理模型状态")}</button>
                </div>
              ) : (
                visibleMlxCleanupModels.map(renderMlxCleanupModelRow)
              )}
            <div id="local-cleanup-model" tabIndex={-1} className="outline-none">
              {cleanupModelStoreStatus ? (() => {
                const status = cleanupModelStoreStatus;
                const savedSelected = settings.cleanup_enabled && currentCleanupProvider === "on_device_mlx" && savedCleanupModel === status.id;
                const draftSelected = draftCleanupEnabled && draft.cleanupProvider === "on_device_mlx" && draft.cleanupModel === status.id && !savedSelected;
                const transferInProgress = Boolean(status.operation && status.operation !== "delete");
                const isCancellation = (message: string | null | undefined) => /^(download|import) cancelled$/i.test(message ?? "");
                const statusError = isCancellation(status.error) ? null : status.error;
                const lastOperationError = isCancellation(status.last_operation_error) ? null : status.last_operation_error;
                const operationError = statusError || lastOperationError;
                const operationWasCancelled = !status.operation && !operationError && (isCancellation(status.error) || isCancellation(status.last_operation_error));
                const statusSummary = [
                  status.bytes_total > 0 ? formatModelBytes(status.bytes_total) : t("大小未知"),
                  cleanupModelStoreSummaryCopy(status.state, t),
                  operationWasCancelled && status.state !== "cancelled" ? t("已取消") : null,
                ].filter(Boolean).join(" · ");
                const modelName = status.label.replace(/\s+·\s+MLX(?:\s+.*)?$/i, "");

                return (
                  <SettingsRow
                    className="items-center py-3"
                    title={modelName}
                    description={(
                      <div className="space-y-1">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                          <span>{statusSummary}</span>
                          {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                          {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                          <details className="inline-block max-w-full text-xs leading-5">
                            <summary className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationError ? t("查看技术信息与错误") : t("技术信息")}</summary>
                            <div className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                              <p>{t("模型构建")}: {status.label}</p>
                              <p>{t("来源仓库")}: {status.repository}</p>
                              <p className="break-all">{t("固定版本")}: {status.revision}</p>
                              <p>{t("文件状态")}: {cleanupModelStoreStateCopy(status.state, t)}</p>
                              <p>{t("文件系统说明")}: {t("导入模型会使用 APFS 共享块；原始文件夹不会更改。")}</p>
                              {statusError && <p className="break-words text-error-ink">{statusError}</p>}
                              {lastOperationError && lastOperationError !== statusError && <p className="break-words text-error-ink">{lastOperationError}</p>}
                            </div>
                          </details>
                        </div>
                        {transferInProgress && status.bytes_total > 0 && (
                          <div className="space-y-1">
                            <p role="status" className="text-xs text-tertiary">{formatModelBytes(status.bytes_done)} / {formatModelBytes(status.bytes_total)}</p>
                            <progress aria-label={t("本机模型操作进度")} className="h-1.5 w-full accent-accent" max={100} value={Math.min(100, status.bytes_done / status.bytes_total * 100)} />
                          </div>
                        )}
                        {status.platform_supported === false && <p className="text-warning-ink">{t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。")}</p>}
                        {(status.platform_supported === null || status.platform_supported === undefined) && <p className="text-warning-ink">{t("无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。")}</p>}
                        {statusError && <p role="alert" className="text-error-ink">{t("模型状态有错误；查看技术信息与错误。")}</p>}
                        {lastOperationError && !statusError && <p role="alert" className="text-error-ink">{t("上次模型操作失败；查看技术信息与错误。")}</p>}
                        {cleanupModelStoreReadError && <p role="alert" className="text-error-ink">{cleanupModelStoreReadError}</p>}
                        {cleanupModelStoreActionError && <p role="alert" className="text-error-ink">{cleanupModelStoreActionError}</p>}
                      </div>
                    )}
                  >
                    {transferInProgress ? (
                      <button type="button" disabled={cleanupModelStoreBusy} onClick={() => void runCleanupModelStoreAction("cancel")} className={compactButtonClass}>{cleanupModelStoreBusy ? t("处理中…") : t("取消模型操作")}</button>
                    ) : null}
                    {cleanupModelStoreIdle && !status.managed && (
                      <>
                        <button type="button" disabled={cleanupModelStoreBusy || !cleanupMlxPlatformSupported} onClick={() => void runCleanupModelStoreAction("download")} className={compactButtonClass}>{cleanupModelStoreBusy ? t("准备中…") : t("下载模型")}</button>
                        <details className="max-w-full text-left">
                          <summary className={`cursor-pointer px-2 py-1 text-xs text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{t("导入已有模型")}</summary>
                          <div className="mt-1 space-y-1 text-right">
                            <p className="text-xs leading-5 text-tertiary">{t("导入匹配的 MLX 模型文件夹；原始文件保留。")}</p>
                            <button type="button" disabled={cleanupModelStoreBusy || !cleanupMlxPlatformSupported} onClick={() => void runCleanupModelStoreAction("import")} className={ghostButtonClass}>{cleanupModelStoreBusy ? t("正在打开文件夹…") : t("选择文件夹并导入")}</button>
                          </div>
                        </details>
                      </>
                    )}
                    {cleanupModelStoreIdle && (status.managed || status.partial_managed) && <button type="button" aria-label={`${t("删除模型")}: ${modelName}`} disabled={cleanupModelStoreBusy} onClick={() => setConfirmDeleteCleanupModel(true)} className={compactDangerButtonClass}>{t("删除模型")}</button>}
                    {cleanupModelStoreNeedsRefresh && <button type="button" disabled={cleanupModelStoreBusy} onClick={() => void refreshCleanupModelStore()} className={compactButtonClass}>{t("刷新模型状态")}</button>}
                  </SettingsRow>
                );
              })() : (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p role={cleanupModelStoreReadError ? "alert" : "status"} className={`text-xs leading-5 ${cleanupModelStoreReadError ? "text-error-ink" : "text-tertiary"}`}>{cleanupModelStoreReadError ?? t("正在读取模型状态")}</p>
                  {cleanupModelStoreNeedsRefresh && <button type="button" disabled={cleanupModelStoreBusy} onClick={() => void refreshCleanupModelStore()} className={compactButtonClass}>{t("刷新模型状态")}</button>}
                </div>
              )}
            </div>
            </div>
            <div id="local-cleanup-models" tabIndex={-1} className="outline-none">
              {localCleanupManagementRequired && (
                <div className="border-t border-border">
                  <h3 className="px-5 pb-2 pt-4 text-xs font-medium text-tertiary">{t("旧版 GGUF 文件管理")}</h3>
                  <div className="divide-y divide-border">
                    {localCleanupModelsLoading && visibleLocalCleanupModels.length === 0 ? (
                      <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机整理模型状态…")}</p>
                    ) : localCleanupModelsReadError && visibleLocalCleanupModels.length === 0 ? (
                      <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                        <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupModelsReadError}</p>
                        <button type="button" disabled={localCleanupModelsLoading} onClick={() => void refreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("重新读取本机整理模型状态")}</button>
                      </div>
                    ) : (
                      <>
                        {localCleanupModelsReadError && (
                          <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                            <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupModelsReadError}</p>
                            <button type="button" disabled={localCleanupModelsLoading} onClick={() => void refreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("重新读取本机整理模型状态")}</button>
                          </div>
                        )}
                        {visibleLocalCleanupModels.length === 0 ? (
                          <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                            <p role="status" className="text-xs leading-5 text-tertiary">{t("没有可管理的 GGUF 模型。")}</p>
                            <button type="button" disabled={localCleanupModelsLoading} onClick={() => void refreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("刷新本机整理模型状态")}</button>
                          </div>
                        ) : (
                          visibleLocalCleanupModels.map(renderLocalCleanupModelRow)
                        )}
                      </>
                    )}
                  </div>
                </div>
              )}
            </div>
          </fieldset>
        </SettingsGroup>
      </div>
      <div
        id="engine-panel-advanced"
        role="region"
        aria-label={t("高级设置")}
        hidden={page !== "advanced"}
        inert={page !== "advanced"}
        className="vf-engine-page-content"
      >
        {!pendingChanges && saveBlock?.page === "advanced" && <p role="status" className="mb-4 flex flex-wrap items-center gap-2 text-xs leading-5 text-warning-ink"><span>{saveBlock.message}</span><button type="button" className={compactButtonClass} onClick={() => navigateToPage(saveBlock.page, saveBlock.target)}>{saveBlock.action}</button></p>}
        <SettingsGroup title={t("严格离线策略")} description={t("本机性能诊断只在本机显示。")}>
          <div id="advanced-strict-offline" className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-primary">{t("严格离线模式")} <span className="ml-1 text-xs font-normal text-tertiary">{t("立即生效")}</span></p>
              <p id="strict-offline-description" className="mt-1 text-xs leading-5 text-tertiary">{t("开启后会阻止云端回退和视觉请求；可继续使用经检查的本机 Ollama、VoiceFlow llama.cpp 或 VoiceFlow MLX 整理。本机模型下载仍是单独的联网操作。整理不可用时会回退到本地规则或关闭整理。")}</p>
              {strictOfflineSaveError && <p role="alert" className="mt-1 text-xs leading-5 text-error-ink">{strictOfflineSaveError}</p>}
            </div>
            <Toggle
              id="strict-offline-toggle"
              checked={settings.strict_offline_enabled === true}
              disabled={probing || rowProbing !== null || removing || strictOfflineSaving}
              onChange={(checked) => void updateStrictOfflineMode(checked)}
              label={t("严格离线模式")}
              aria-describedby="strict-offline-description"
            />
          </div>
        </SettingsGroup>
        <section className="vf-settings-disclosure vf-settings-disclosure--surface vf-advanced-connections mt-6 rounded-xl border border-border bg-card">
          <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="min-w-0">
          <details open={advancedConnectionsOpen} onToggle={(event) => setAdvancedConnectionsOpen(event.currentTarget.open)}>
            <summary id="advanced-connections-toggle" tabIndex={0} className="flex w-full cursor-pointer list-none items-center gap-3 rounded-xl px-5 py-4 text-left transition-colors enabled:hover:bg-elevated/40">
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium text-primary">{t("连接地址与高级请求参数")}</span>
                <span className="mt-1 block text-[13px] leading-5 text-secondary">{t("编辑自定义、本机服务地址、区域和整理请求参数；连接检查由你手动启动。")}</span>
              </span>
              <ChevronDown size={16} aria-hidden="true" className={`shrink-0 text-tertiary transition-transform duration-150 ${advancedConnectionsOpen ? "rotate-180" : ""}`} />
            </summary>
            <div className="vf-settings-disclosure-content space-y-4 border-t border-border px-5 pb-5 pt-4">
              <section id="advanced-custom-endpoint" className="space-y-3 rounded-lg bg-elevated p-4">
                <div>
                  <h3 className="text-sm font-medium text-primary">{t("自定义服务地址")}</h3>
                  <p className="mt-1 text-xs leading-5 text-tertiary">{t("转写和 AI 整理共用此地址与 API Key；两个阶段分别使用各自的模型。")}</p>
                </div>
                <label className="block text-xs text-secondary">
                  {t("自定义服务地址")}
                  <input
                    id="advanced-custom-base-url"
                    aria-label={t("自定义服务地址")}
                    value={draft.customBaseUrl}
                    onChange={(event) => {
                      const nextUrl = event.target.value;
                      const originChanged = customOriginChanged(draft.customBaseUrl, nextUrl);
                      setDraft((current) => ({
                        ...current,
                        customBaseUrl: nextUrl,
                        ...(originChanged ? { providerKeys: { ...current.providerKeys, custom: "" } } : {}),
                      }));
                      setRowProbeFeedback(null);
                      setProbeSuccess("");
                      setTestedAsrRoute(null);
                      setTestedCleanupRoute(null);
                    }}
                    placeholder="https://api.example.com/v1"
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <div className="flex flex-wrap gap-2">
                  <button type="button" className={compactButtonClass} onClick={() => navigateToPage("api_key", "provider-key-custom")}>{t("管理凭据")}</button>
                </div>
                {customHostChanged(draft, settings) && !draft.providerKeys.custom?.trim() && !isLoopbackUrl(draft.customBaseUrl) && (
                  <p role="status" className="text-xs leading-5 text-warning-ink">{t("服务地址目标已更改；请填写新地址对应的 API Key。")}</p>
                )}
                <details>
                  <summary className="cursor-pointer text-xs font-medium text-secondary">{t("服务预设")}</summary>
                  <p className="mt-2 text-xs leading-5 text-tertiary">{t("预设会选择对应的转写服务、模型和地址；同一 origin 下保留候选密钥，跨 origin 时清除候选密钥。")}</p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {[
                      ["阿里云百炼 Qwen3-ASR", "https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen3-asr-flash"],
                      ["国际站 Qwen3-ASR", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", "qwen3-asr-flash"],
                      ["本机 FunASR", "http://127.0.0.1:10095/v1", "paraformer-zh"],
                      ["本机 Qwen3-ASR (MLX)", "http://127.0.0.1:8765/v1", "Qwen/Qwen3-ASR-0.6B"],
                    ].map(([label, url, model]) => (
                      <button key={label} type="button" className={compactButtonClass} onClick={() => {
                        setDraft((current) => ({
                          ...current,
                          asrProvider: "custom",
                          customAsr: true,
                          customBaseUrl: url,
                          asrModel: model,
                          ...(customOriginChanged(current.customBaseUrl, url) ? { providerKeys: { ...current.providerKeys, custom: "" } } : {}),
                        }));
                        setRowProbeFeedback(null);
                        setProbeSuccess("");
                        setTestedAsrRoute(null);
                        setTestedCleanupRoute(null);
                      }}>{t(label)}</button>
                    ))}
                    <button type="button" className={compactButtonClass} onClick={() => {
                      setDraft((current) => ({ ...current, asrProvider: "local_whisper", localWhisperBaseUrl: "http://127.0.0.1:9000/v1", asrModel: "ggml-large-v3-turbo.bin" }));
                      setRowProbeFeedback(null);
                      setProbeSuccess("");
                      setTestedAsrRoute(null);
                    }}>{t("本机 Whisper.cpp")}</button>
                  </div>
                </details>
              </section>

              {draft.asrProvider === "dashscope" && (
                <section id="advanced-dashscope-region" tabIndex={-1} className="rounded-lg bg-elevated p-4">
                  <label className="block text-xs font-medium text-secondary">
                    {t("DashScope 服务区域")}
                    <Select aria-label={t("服务区域")} value={draft.dashscopeRegion} onValueChange={changeDashscopeRegion} className={`${controlClass} mt-2 w-full text-xs`}>
                      <option value="beijing">{t("北京")} · dashscope.aliyuncs.com</option>
                      <option value="singapore">{t("新加坡")} · dashscope-intl.aliyuncs.com</option>
                    </Select>
                  </label>
                  <p className="mt-1 text-xs leading-5 text-tertiary">{t("地区会与转写服务设置一起保存；Message WebSocket 使用该地区的固定官方地址。")}</p>
                </section>
              )}

              <section id="advanced-local-whisper-connection" className="rounded-lg bg-elevated p-4">
                <label className="block text-xs text-secondary">
                  {t("本机 Whisper 地址")}
                  <input
                    id="advanced-local-whisper-url"
                    aria-label={t("本机 Whisper 地址")}
                    value={draft.localWhisperBaseUrl}
                    onChange={(event) => {
                      setDraft((current) => ({ ...current, localWhisperBaseUrl: event.target.value }));
                      setRowProbeFeedback(null);
                      setProbeSuccess("");
                      setTestedAsrRoute(null);
                    }}
                    placeholder={providerById("local_whisper")?.defaultBaseUrl}
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <p className="mt-1 text-xs leading-5 text-tertiary">{t("先自行运行 whisper.cpp 或 speaches，并填写兼容的本机 HTTP 地址。")}</p>
              </section>

              <section id="advanced-ollama-connection" className="space-y-3 rounded-lg bg-elevated p-4">
                <label className="block text-xs text-secondary">
                  {t("Ollama 本机地址")}
                  <input
                    id="advanced-ollama-url"
                    aria-label={`Ollama ${t("本机地址")}`}
                    value={draft.ollamaBaseUrl}
                    onChange={(event) => {
                      setDraft((current) => ({ ...current, ollamaBaseUrl: event.target.value }));
                      setRowProbeFeedback(null);
                      setProbeSuccess("");
                      setTestedCleanupRoute(null);
                      setLocalCleanupStatus(null);
                    }}
                    placeholder={providerById("ollama")?.defaultBaseUrl}
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <p className="text-xs leading-5 text-tertiary">{t("VoiceFlow 不会安装 Ollama 或下载模型。检查状态只读取本机服务。")}</p>
                {localCleanupStatus && <p role="status" className={`text-xs leading-5 ${localCleanupStatus.available ? "text-success-ink" : "text-warning-ink"}`}>{localCleanupStatus.available ? t("本机整理服务可用") : t("本机整理服务不可用")} · {localCleanupStatus.status}{localCleanupStatus.message ? ` · ${localCleanupStatus.message}` : ""}</p>}
                {localCleanupError && <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupError}</p>}
                {!localCleanupRouteSaved && <p className="text-xs leading-5 text-warning-ink">{t("先保存本机 Ollama 地址，再检查状态。")}</p>}
                <div className="flex flex-wrap gap-2">
                  <button type="button" disabled={localCleanupChecking || !localCleanupRouteSaved} onClick={() => void checkLocalCleanupStatus()} className={compactButtonClass}>{localCleanupChecking ? t("检查中…") : t("检查本机 Ollama")}</button>
                  <button type="button" onClick={chooseLocalCleanup} className={compactButtonClass}>{t("使用本机 Qwen3.5:4b 整理")}</button>
                </div>
              </section>

              {draftCleanupEnabled && draft.cleanupProvider === "custom" && (
                <section id="advanced-custom-cleanup-parameters-section" className="rounded-lg bg-elevated p-4">
                  <label className="block text-xs font-medium text-secondary">
                    {t("高级请求参数 JSON")}
                    <p className="mt-1 font-normal leading-5 text-tertiary">{t("填写 JSON 对象，作为自定义 OpenAI-compatible 整理请求的顶层字段。留空表示使用当前请求默认值；不要填写 API Key、密钥、headers、messages、model 或 stream。")}</p>
                    <textarea
                      id="advanced-custom-cleanup-parameters"
                      aria-label={t("高级请求参数 JSON")}
                      value={draft.customCleanupParametersText}
                      onChange={(event) => {
                        const source = event.target.value;
                        const parsed = parseCustomCleanupParameters(source);
                        setDraft((current) => ({
                          ...current,
                          customCleanupParametersText: source,
                          ...(parsed.ok ? { customCleanupParameters: parsed.value } : {}),
                        }));
                        setProbeSuccess("");
                        setTestedCleanupRoute(null);
                      }}
                      rows={6}
                      spellCheck={false}
                      autoComplete="off"
                      className={`${controlClass} mt-2 min-h-28 w-full resize-y font-mono text-xs leading-5`}
                      placeholder={'{\n  "temperature": 0.2,\n  "max_completion_tokens": 2048\n}'}
                    />
                  </label>
                  {!customParametersParse.ok && <p role="alert" className="mt-2 text-xs leading-5 text-error-ink">{customParametersParse.error}</p>}
                </section>
              )}
            </div>
          </details>
          </fieldset>
        </section>
        <SettingsDisclosure title={t("本机性能诊断")} description={t("查看阶段延迟和交付计数，仅保存在本机。")} error={metricsError}>
          <LatencyMetricsViewer onErrorChange={setMetricsError} />
        </SettingsDisclosure>
      </div>
      {showEngineFooter && <div className="vf-engine-footer vf-save-bar flex flex-wrap items-center justify-end gap-3 px-5 py-4">
        {pendingChanges && (
          <button type="button" disabled={probing || rowProbing !== null || removing || strictOfflineSaving} onClick={() => {
            setDraft(draftFromSettings(settings));
            setDraftCleanupEnabled(settings.cleanup_enabled);
            setStageFail({});
            setCommitError(null);
            setProbeSuccess("");
            setTestedAsrRoute(null);
            setTestedCleanupRoute(null);
            navigateToPage("service", "engine-asr-provider");
          }} className={ghostButtonClass}>{t("放弃更改")}</button>
        )}
        {saveBlock && pendingChanges && <p role="status" className="flex w-full flex-wrap items-center gap-2 text-xs leading-5 text-warning-ink"><span>{saveBlock.message}</span><button type="button" className={compactButtonClass} onClick={() => navigateToPage(saveBlock.page, saveBlock.target)}>{saveBlock.action}</button></p>}
        {commitError && <p role="alert" className="w-full text-xs leading-5 text-error-ink">{commitError}</p>}
        <button type="button" onClick={() => void applyDraft()} aria-busy={probing} disabled={probing || rowProbing !== null || removing || strictOfflineSaving || !ready} className={`${buttonClass} min-w-36`}>
          {probing ? t("保存中…") : t("保存并使用")}
        </button>
      </div>}
      </div>
      <ConfirmDialog
        open={confirmRemove !== null}
        title={t("删除密钥")}
        description={t("删除后需要重新填写这个服务商的密钥。确定删除吗？")}
        confirmLabel={t("删除密钥")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemove(null)}
        onConfirm={() => confirmRemove && void removeKey(confirmRemove)}
      />
      <ConfirmDialog
        open={confirmDeleteModel !== null}
        title={t("删除本机模型")}
        description={t("这会从本机删除所选模型文件。下载仍需你之后手动启动。确定删除吗？")}
        confirmLabel={t("删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteModel(null)}
        onConfirm={() => {
          if (!confirmDeleteModel) return;
          const id = confirmDeleteModel;
          setConfirmDeleteModel(null);
          void runOnDeviceAction(id, "delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteCleanupModel}
        title={t("删除 VoiceFlow MLX 模型")}
        description={t("VoiceFlow 会先请求 cleanup runtime 卸载模型，再删除它管理的文件。原始导入文件夹不会更改。确定删除吗？")}
        confirmLabel={t("删除模型文件")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteCleanupModel(false)}
        onConfirm={() => {
          setConfirmDeleteCleanupModel(false);
          void runCleanupModelStoreAction("delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteMlxCleanupModel !== null}
        title={t(cleaningOnlyPartialMlxCleanupModel ? "清理未完成模型文件" : "删除本机整理模型")}
        description={t(cleaningOnlyPartialMlxCleanupModel
          ? "这会从本机删除此模型尚未完成的下载文件；之后可以重新启动下载。确定清理吗？"
          : "这会从本机删除 VoiceFlow 管理的 MLX 整理模型文件；之后可以重新下载。确定删除吗？")}
        confirmLabel={t(cleaningOnlyPartialMlxCleanupModel ? "清理文件" : "删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteMlxCleanupModel(null)}
        onConfirm={() => {
          const id = confirmDeleteMlxCleanupModel;
          if (!id) return;
          const savedSelected = settings.cleanup_enabled
            && currentCleanupProvider === "on_device_mlx"
            && savedCleanupModel === id;
          if (savedSelected) {
            setMlxCleanupModelActionErrors((current) => ({
              ...current,
              [id]: t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。"),
            }));
            setConfirmDeleteMlxCleanupModel(null);
            return;
          }
          setConfirmDeleteMlxCleanupModel(null);
          void runMlxCleanupModelAction(id, "delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteLocalCleanupModel !== null}
        title={t(cleaningOnlyPartialModel ? "清理未完成模型文件" : "删除本机整理模型")}
        description={t(cleaningOnlyPartialModel
          ? "这会从本机删除此模型尚未完成的下载文件；之后可以重新启动下载。确定清理吗？"
          : "这会从本机删除 VoiceFlow 管理的模型文件；之后需要你再次手动下载。确定删除吗？")}
        confirmLabel={t(cleaningOnlyPartialModel ? "清理文件" : "删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteLocalCleanupModel(null)}
        onConfirm={() => {
          const id = confirmDeleteLocalCleanupModel;
          if (!id) return;
          const savedSelected = settings.cleanup_enabled
            && currentCleanupProvider === "on_device_llama_cpp"
            && savedCleanupModel === id;
          const draftSelected = draftCleanupEnabled
            && draft.cleanupProvider === "on_device_llama_cpp"
            && draft.cleanupModel === id;
          if (savedSelected || draftSelected) {
            setLocalCleanupModelActionErrors((current) => ({
              ...current,
              [id]: t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。"),
            }));
            setConfirmDeleteLocalCleanupModel(null);
            return;
          }
          setConfirmDeleteLocalCleanupModel(null);
          void runLocalCleanupModelAction(id, "delete");
        }}
      />
    </SettingsShell>
  );
}

function ModelControl({
  id,
  side,
  value,
  ariaLabel,
  onChange,
  deviceModels = [],
  optionsOverride,
}: {
  id: ProviderId;
  side: "asr" | "llm";
  value: string;
  ariaLabel: string;
  onChange: (value: string) => void;
  deviceModels?: OnDeviceModelStatus[];
  optionsOverride?: readonly ModelOption[];
}) {
  const { t } = useI18n();
  const definition = providerById(id);
  if (!definition) return null;
  const field = side === "asr" ? definition.asrModelField : definition.llmModelField;
  const options = optionsOverride ?? (side === "asr" ? definition.asrModels : definition.llmModels);
  if (id === "on_device_llama_cpp") {
    const selectedValue = options.some((option) => option.value === value) ? value : "";
    return (
      <Select
        aria-label={ariaLabel}
        value={selectedValue}
        onValueChange={(next) => onChange(next)}
        disabled={options.length === 0}
        className={`${controlClass} w-[220px] max-w-full`}
      >
        {selectedValue === "" && <option value="" disabled>{t("请选择已下载并验证的模型")}</option>}
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={Boolean(option.retiredForNewSelection && option.value !== value)}
          >{option.note ? `${option.label} · ${t(option.note)}` : option.label}</option>
        ))}
      </Select>
    );
  }
  if (field === "select" && options.length > 0) {
    return (
      <Select aria-label={ariaLabel} value={value} onValueChange={(value) => onChange(value)} className={`${controlClass} w-[220px] max-w-full`}>
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={Boolean(option.retiredForNewSelection && option.value !== value)
              || (id === "on_device"
                && option.value !== "sensevoice-small"
                && deviceModels.find((model) => model.id === option.value)?.platform_supported === false)}
          >
            {option.note ? `${option.label} · ${t(option.note)}` : option.label}
          </option>
        ))}
      </Select>
    );
  }
  return (
    <>
      <Autocomplete
        aria-label={ariaLabel}
        value={value}
        options={options}
        onValueChange={onChange}
        placeholder={defaultModel(id, side === "asr" ? "asr" : "llm") || "model"}
        autoComplete="off"
        spellCheck={false}
        className={`${controlClass} w-[220px] max-w-full font-mono text-sm`}
      />

    </>
  );
}

function SecretField({
  id,
  ariaLabel,
  typed,
  hint,
  placeholder,
  onTypedChange,
}: {
  id: string;
  ariaLabel: string;
  typed: string;
  hint: string;
  placeholder: string;
  onTypedChange: (value: string) => void;
}) {
  const showingHint = !typed && Boolean(hint);
  return (
    <PasswordInput
      id={id}
      ariaLabel={ariaLabel}
      value={typed || hint}
      onChange={(value) => {
        if (showingHint && value === hint) return;
        onTypedChange(value === hint ? "" : value);
      }}
      placeholder={placeholder}
      monospace
      plain={showingHint}
      className="mt-2"
    />
  );
}
