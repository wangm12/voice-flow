import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Copy, RefreshCw } from "lucide-react";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass } from "../../lib/theme";

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
        const val = amount(entry.amount);
        return unit === "unknown" ? [] : [{ unit, amount: val }];
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
  const normalizeDelivery = (candidate: unknown): DiagnosticDelivery => {
    const delivery = isRecord(candidate) ? candidate : {};
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
  const normalizeGroup = (candidate: unknown): DiagnosticGroup => {
    const group = isRecord(candidate) ? candidate : {};
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

export function LatencyMetricsViewer({ onErrorChange }: { onErrorChange?: (error: string | null) => void }) {
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
    onErrorChange?.(error ? t("诊断数据未能读取或复制，请重试。") : null);
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
