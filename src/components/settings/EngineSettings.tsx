import { Autocomplete } from "../Autocomplete";
import { Select } from "../Select";
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, Copy, KeyRound, RefreshCw } from "lucide-react";
import { ConfirmDialog } from "../ConfirmDialog";
import { PasswordInput } from "../PasswordInput";
import { SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { useI18n } from "../../lib/i18n";
import {
  asrLanguageDescription,
  draftFromSettings,
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
import {
  PROVIDERS,
  asrModelProfile,
  capabilityLabel,
  defaultModel,
  isLoopbackUrl,
  isProviderId,
  providerById,
  providersFor,
  type ProviderId,
} from "../../lib/providers";
import { friendlySettingsError } from "../../lib/settingsError";
import { buttonClass, ghostButtonClass, compactButtonClass, compactDangerButtonClass, colors, focusRingClass, radius } from "../../lib/theme";
import type { LocalCleanupStatus, OnDeviceModelStatus, SaveSettings, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;

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
  ["stop_to_insert", "停止到插入"],
] as const satisfies ReadonlyArray<readonly [keyof Pick<DiagnosticGroup, "prefetch_asr" | "final_asr" | "cleanup" | "validation" | "paste_submission" | "readback_confirmation" | "stop_to_insert">, string]>;

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
    ["stop_to_insert", "停止到插入", metrics.stop_to_insert],
  ] as const : [];

  return (
    <div className="vf-diagnostics pt-4">
      <p className="text-xs leading-5 text-secondary">{t("查看本机听写阶段延迟和交付计数。仅包含受限的服务商、模型与路径标签，不包含文字、音频或上下文。")}</p>
      <div className="flex flex-wrap items-center justify-between gap-3 py-4">
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" onClick={() => void refresh()} disabled={loading} className={compactButtonClass}>
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} aria-hidden="true" />
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
  if (kind === "path") return t("地址路径不对");
  if (kind === "missing_key") return t("缺少密钥");
  if (kind === "provider") return t("服务返回错误");
  if (kind === "on_device_model_missing") return t("本地模型未就绪");
  if (kind === "on_device_inference_unavailable") return t("本机推理不可用；请检查运行时或模型状态后重试");
  return message?.trim() || t("服务返回错误");
}

function capabilityText(id: ProviderId, draft: EngineDraft, t: (key: string) => string): string {
  const definition = providerById(id);
  const capabilities = id === "custom"
    ? [
        ...(draft.customAsr ? ["asr" as const] : []),
        ...(draft.customLlm ? ["llm" as const] : []),
      ]
    : definition?.capabilities ?? [];
  const kind = capabilityLabel(capabilities);
  if (kind === "both") return `${t("转写")} · ${t("润色")}`;
  if (kind === "asr") return t("转写");
  return t("润色");
}

export function EngineSettings({
  settings,
  removeProviderKey,
  commitEngine,
}: {
  settings: Settings;
  save: SaveSettings;
  saveApiKey?: (apiKey: string) => Promise<void>;
  removeApiKey?: () => Promise<void>;
  saveAsrApiKey?: (apiKey: string, asrBaseUrl?: string) => Promise<void>;
  removeAsrApiKey?: () => Promise<void>;
  removeCleanupApiKey?: () => Promise<void>;
  removeProviderKey: (provider: string) => Promise<void>;
  commitEngine: (patch: Record<string, unknown>) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<EngineDraft>(() => draftFromSettings(settings));
  const [draftCleanupEnabled, setDraftCleanupEnabled] = useState(settings.cleanup_enabled);
  const [metricsError, setMetricsError] = useState<string | null>(null);
  const [showAvailableProviders, setShowAvailableProviders] = useState(false);
  const draftSettings = { ...settings, cleanup_enabled: draftCleanupEnabled };
  const asrRoute = JSON.stringify([draft.asrProvider, draft.asrModel.trim(), draft.customBaseUrl.trim(), draft.localWhisperBaseUrl.trim(), draft.dashscopeRegion]);
  const cleanupRoute = JSON.stringify([draft.cleanupProvider, draft.cleanupModel.trim(), draft.customBaseUrl.trim(), draft.ollamaBaseUrl.trim()]);
  const [probing, setProbing] = useState(false);
  const [rowProbing, setRowProbing] = useState<ProviderId | null>(null);
  const [rowProbeFeedback, setRowProbeFeedback] = useState<{
    provider: ProviderId;
    tone: "success" | "error";
    message: string;
  } | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [probeSuccess, setProbeSuccess] = useState<"tested" | "saved" | "">("");
  const [testedCleanupRoute, setTestedCleanupRoute] = useState<string | null>(null);
  const [testedAsrRoute, setTestedAsrRoute] = useState<string | null>(null);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [stageFail, setStageFail] = useState<{ asr?: string | null; cleanup?: string | null }>({});
  const [expanded, setExpanded] = useState<Set<ProviderId>>(() => {
    const next = new Set<ProviderId>([providerOf(settings.asr_provider, settings.asr_base_url)]);
    if (settings.cleanup_enabled) next.add(providerOf(settings.cleanup_provider, settings.cleanup_base_url));
    return next;
  });
  const [confirmRemove, setConfirmRemove] = useState<ProviderId | null>(null);
  const [removing, setRemoving] = useState(false);
  const [onDeviceModels, setOnDeviceModels] = useState<OnDeviceModelStatus[]>([]);
  const [onDeviceActionId, setOnDeviceActionId] = useState<string | null>(null);
  const [onDeviceActionError, setOnDeviceActionError] = useState<string | null>(null);
  const [confirmDeleteModel, setConfirmDeleteModel] = useState<string | null>(null);
  const [localCleanupStatus, setLocalCleanupStatus] = useState<LocalCleanupStatus | null>(null);
  const [localCleanupChecking, setLocalCleanupChecking] = useState(false);
  const [localCleanupError, setLocalCleanupError] = useState<string | null>(null);
  const [strictOfflineSaving, setStrictOfflineSaving] = useState(false);
  const [strictOfflineSaveError, setStrictOfflineSaveError] = useState<string | null>(null);
  const previousSettings = useRef(settings);
  useEffect(() => {
    const previous = previousSettings.current;
    previousSettings.current = settings;
    setDraft((current) => JSON.stringify({ ...current, providerKeys: {} }) === JSON.stringify(draftFromSettings(previous))
      ? { ...draftFromSettings(settings), providerKeys: current.providerKeys } : current);
    setDraftCleanupEnabled((current) => current === previous.cleanup_enabled ? settings.cleanup_enabled : current);
  }, [settings]);

  const rowRefs = useRef<Partial<Record<ProviderId, HTMLDivElement | null>>>({});
  const asrProviderRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let cancelled = false;
    const apply = (models: OnDeviceModelStatus[]) => {
      if (cancelled) return;
      setOnDeviceModels(Array.isArray(models) ? models : []);
    };
    void Promise.resolve(invoke<OnDeviceModelStatus[]>("list_on_device_models"))
      .then(apply)
      .catch(() => {
        if (!cancelled) {
          setOnDeviceModels([]);
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
  }, []);

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
    try {
      const models = await invoke<OnDeviceModelStatus[]>("list_on_device_models");
      setOnDeviceModels(Array.isArray(models) ? models : []);
    } catch {
      setOnDeviceActionError(t("无法读取本机模型状态，请重试。"));
    }
  };

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
      setOnDeviceActionError(t("本机模型操作失败，请重试。"));
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

  const selectedAsrIsLocal = draft.asrProvider === "on_device";
  const selectedCleanupIsLocal = draft.cleanupProvider === "ollama" && isLoopbackUrl(draft.ollamaBaseUrl);
  const strictOfflineRoutesAllowed = settings.strict_offline_enabled !== true
    || (selectedAsrIsLocal && (!draftCleanupEnabled || selectedCleanupIsLocal));
  const asrMissing = !hasProviderSecret(draft.asrProvider, draft, settings, onDeviceReady);
  const cleanupMissing = draftCleanupEnabled && !hasProviderSecret(draft.cleanupProvider, draft, settings);
  const hasSupportedAsrLanguageSetting = hasSupportedAsrLanguage(asrModelProfile(draft.asrProvider, draft.asrModel), settings.language);
  const ready = step2Ready(draft, draftSettings, onDeviceReady) && hasSupportedAsrLanguageSetting && strictOfflineRoutesAllowed;
  const previousCredentialRoute = useRef({ asr: draft.asrProvider, cleanup: draft.cleanupProvider, cleanupEnabled: draftCleanupEnabled });

  useEffect(() => {
    setExpanded((current) => {
      const next = new Set(current);
      next.add(draft.asrProvider);
      if (draftCleanupEnabled) next.add(draft.cleanupProvider);
      return next;
    });
  }, [draft.asrProvider, draft.cleanupProvider, draftCleanupEnabled]);

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
    rowRefs.current[missing]?.scrollIntoView?.({ block: "nearest" });
    document.getElementById(`provider-key-${missing}`)?.focus();
  }, [asrMissing, cleanupMissing, draft.asrProvider, draft.cleanupProvider, draftCleanupEnabled]);

  const currentAsrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const currentCleanupProvider = providerOf(settings.cleanup_provider, settings.cleanup_base_url);
  const savedDraft = draftFromSettings(settings);
  const pendingChanges = draftCleanupEnabled !== settings.cleanup_enabled
    || JSON.stringify(persistPatch({ ...draft, providerKeys: {} }, draftSettings)) !== JSON.stringify(persistPatch(savedDraft, draftSettings))
    || Object.values(draft.providerKeys).some((value) => value?.trim());

  const selectedProviders = new Set<ProviderId>([draft.asrProvider, ...(draftCleanupEnabled ? [draft.cleanupProvider] : [])]);
  const otherConfiguredCount = PROVIDERS.filter((provider) => !selectedProviders.has(provider.id) && providerConfigured(settings, provider.id)).length;
  const otherProviderError = rowProbeFeedback?.tone === "error" && !selectedProviders.has(rowProbeFeedback.provider);
  useEffect(() => { if (otherProviderError) setShowAvailableProviders(true); }, [otherProviderError]);

  useEffect(() => {
    setStageFail({});
    setProbeError(null);
    setCommitError(null);
    setProbeSuccess("");
    setRowProbeFeedback(null);
  }, [asrRoute, cleanupRoute, draftCleanupEnabled]);

  const failKind = (kind: string | null | undefined) => {
    if (kind === "key" || kind === "missing_key") return t("密钥无效");
    if (kind) return probeCopy(kind, undefined, t);
    return null;
  };

  const runProbe = async () => {
    setProbing(true);
    setProbeError(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
    setTestedCleanupRoute(null);
    setCommitError(null);
    try {
      if (draft.asrProvider === "soniox") {
        await commitEngine({ ...persistPatch(draft, draftSettings), cleanup_enabled: draftCleanupEnabled });
        setDraft((current) => ({ ...current, providerKeys: {} }));
        setStageFail({});
        setProbeSuccess("saved");
        return;
      }
      const result = await invoke<ProbeResult>("probe_engine_draft", {
        draft: probePayload(draft, draftSettings, draft.asrProvider === "assemblyai" ? { cleanupEnabled: false } : undefined),
      });
      if (!result.asr.ok) {
        setStageFail({ asr: result.asr.error_kind, cleanup: undefined });
        setProbeError(`${t("转写")}：${probeCopy(result.asr.error_kind, result.asr.message, t)}`);
        return;
      }
      setTestedAsrRoute(asrRoute);
      if (!result.cleanup.ok) {
        setStageFail({ asr: undefined, cleanup: result.cleanup.error_kind });
        setProbeError(`${t("文字整理")}：${probeCopy(result.cleanup.error_kind, result.cleanup.message, t)}`);
        return;
      }
      setStageFail({});
      if (!result.cleanup.skipped) setTestedCleanupRoute(cleanupRoute);
      await commitEngine({ ...persistPatch(draft, draftSettings), cleanup_enabled: draftCleanupEnabled });
      setDraft((current) => ({ ...current, providerKeys: {} }));
      setProbeSuccess("tested");
      setTestedAsrRoute(asrRoute);
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

  const testConnection = async (id: ProviderId) => {
    const definition = providerById(id);
    if (!definition) return;
    setRowProbing(id);
    setRowProbeFeedback(null);
    setProbeError(null);
    setProbeSuccess("");
    setCommitError(null);
    if (settings.strict_offline_enabled === true && id !== "soniox" && id !== "on_device") {
      setRowProbeFeedback({ provider: id, tone: "error", message: t("严格离线模式会阻止连接云端服务。") });
      setRowProbing(null);
      return;
    }
    try {
      if (id === draft.asrProvider) setTestedAsrRoute(null);
      if (id === "soniox") {
        const typed = draft.providerKeys[id]?.trim();
        if (!typed && !providerConfigured(settings, id)) {
          setRowProbeFeedback({ provider: id, tone: "error", message: t("请先填写服务商 API Key。") });
          return;
        }
        if (typed) {
          await commitEngine({ provider_keys: { [id]: typed } });
          setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
        }
        setStageFail((current) => ({ ...current, asr: undefined }));
        setRowProbeFeedback({ provider: id, tone: "success", message: t("Soniox 凭据已保存；实时连接尚未测试。") });
        return;
      }
      const asrOnly = definition.capabilities.includes("asr");
      const result = await invoke<ProbeResult>("probe_engine_draft", {
        draft: probePayload(draft, draftSettings, asrOnly
          ? { asrProvider: id, cleanupEnabled: false }
          : { cleanupProvider: id, cleanupEnabled: true }),
      });
      const stage = asrOnly ? result.asr : result.cleanup;
      if (!stage.ok) {
        setStageFail((current) => (
          asrOnly && id === draft.asrProvider ? { ...current, asr: stage.error_kind }
            : !asrOnly && id === draft.cleanupProvider ? { ...current, cleanup: stage.error_kind } : current
        ));
        setRowProbeFeedback({
          provider: id,
          tone: "error",
          message: `${definition.label}：${probeCopy(stage.error_kind, stage.message, t)}`,
        });
        return;
      }
      setStageFail((current) => (
        asrOnly && id === draft.asrProvider ? { ...current, asr: undefined }
          : !asrOnly && id === draft.cleanupProvider ? { ...current, cleanup: undefined } : current
      ));
      const typed = draft.providerKeys[id]?.trim();
      if (typed) {
        await commitEngine({ provider_keys: { [id]: typed } });
        setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
      }
      setRowProbeFeedback({ provider: id, tone: "success", message: t("测试连接通过") });
      if (asrOnly && id === draft.asrProvider) setTestedAsrRoute(asrRoute);
    } catch (reason) {
      setRowProbeFeedback({ provider: id, tone: "error", message: friendlySettingsError(reason, t) });
    } finally {
      setRowProbing(null);
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

  const asrOptions = providersFor("asr", draft.customAsr, draft.customLlm);
  const cleanupOptions = providersFor("llm", draft.customAsr, draft.customLlm);
  const asrProfile = asrModelProfile(draft.asrProvider, draft.asrModel);
  const hasSupportedDeviceModel = onDeviceModels.some((model) => model.id !== "sensevoice-small" && model.platform_supported);
  const savedOllamaBaseUrl = settings.ollama_base_url || providerById("ollama")?.defaultBaseUrl || "";
  const localCleanupRouteSaved = draft.ollamaBaseUrl.trim() === savedOllamaBaseUrl.trim();
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
  const cleanupStatus = !draftCleanupEnabled
    ? { label: t("关闭 · 只用本地规则"), tone: "unused" as const }
    : settings.strict_offline_enabled && draft.cleanupProvider !== "ollama"
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
    const rowKind = [
      draft.asrProvider === provider.id ? stageFail.asr : undefined,
      draftCleanupEnabled && draft.cleanupProvider === provider.id
        ? stageFail.cleanup
        : undefined,
    ].find((kind) => kind);
    const rowFail = failKind(rowKind);
    const rowError = Boolean(rowFail) || (rowProbeFeedback?.provider === provider.id && rowProbeFeedback.tone === "error");
    const open = inDraft || requestedOpen || rowError;
    const rowTone = rowFail
      ? { label: rowFail, tone: "error" as const }
      : inUse
        ? { label: t("使用中"), tone: "accent" as const }
        : inDraft
          ? { label: t("待应用"), tone: "warning" as const }
        : providerConfigured(settings, provider.id)
          ? { label: t("已配置"), tone: "unused" as const }
          : { label: t("未添加"), tone: "unused" as const };
    const strictOfflineBlocksProviderProbe = settings.strict_offline_enabled === true
      && provider.id !== "soniox"
      && provider.id !== "on_device";
    return (
      <div key={provider.id} data-provider={provider.id} data-selected={inDraft} hidden={!inDraft && !showAvailableProviders} inert={!inDraft && !showAvailableProviders} className="vf-provider-row" ref={(node) => { rowRefs.current[provider.id] = node; }}>
        <div className="flex flex-wrap items-center gap-3 px-5 py-4">
          {inDraft ? <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-primary">{provider.id === "custom" ? t("兼容接口") : provider.id === "on_device" ? t("本机模型") : provider.label}</p>
            <p className="mt-1 text-xs text-secondary">{capabilityText(provider.id, draft, t)}</p>
          </div> : <button type="button" className="min-w-0 flex-1 text-left" aria-expanded={open} aria-controls={`provider-details-${provider.id}`} onClick={() => toggleExpanded(provider.id)}>
            <p className="text-sm font-medium text-primary">{provider.id === "custom" ? t("兼容接口") : provider.id === "on_device" ? t("本机模型") : provider.label}</p>
            <p className="mt-0.5 text-xs text-tertiary">{capabilityText(provider.id, draft, t)}</p>
          </button>}
          <SettingsStatus label={rowTone.label} tone={rowTone.tone} />
          {inDraft ? null : !configured && !open ? (
            <button type="button" aria-expanded={open} aria-controls={`provider-details-${provider.id}`} onClick={() => toggleExpanded(provider.id)} className={compactButtonClass}>{t("添加")}</button>
          ) : (
            <button type="button" aria-label={`${open ? t("收起") : t("展开")} ${provider.label}`} aria-expanded={open} aria-controls={`provider-details-${provider.id}`} onClick={() => toggleExpanded(provider.id)} className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-secondary hover:bg-elevated ${focusRingClass}`}>
              <ChevronDown size={16} className={open ? "rotate-180" : ""} aria-hidden="true" />
            </button>
          )}
        </div>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} id={`provider-details-${provider.id}`} hidden={!open} inert={!open} className="min-w-0 space-y-3 px-5 pb-4">
            {provider.id === "custom" && (
              <>
                <label className="block text-xs text-secondary">
                  {t("兼容地址")}
                  <input
                    aria-label={t("兼容地址")}
                    value={draft.customBaseUrl}
                    onChange={(event) => {
                      setDraft({ ...draft, customBaseUrl: event.target.value });
                      setRowProbeFeedback(null);
                    }}
                    placeholder="https://api.example.com/v1"
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <div className="flex flex-wrap gap-2">
                  <p className="w-full text-xs text-tertiary">{t("中文转写预设")}</p>
                  <p className="w-full text-xs leading-5 text-tertiary">
                    {t("这条 Qwen 路径走 chat completions，需要带 ASR 权限的百炼密钥，不是 Groq Whisper。")}
                  </p>
                  <button
                    type="button"
                    className={compactButtonClass}
                    onClick={() => setDraft((current) => ({
                      ...current,
                      asrProvider: "custom",
                      customAsr: true,
                      customBaseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
                      asrModel: "qwen3-asr-flash",
                    }))}
                  >
                    {t("阿里云百炼 Qwen3-ASR")}
                  </button>
                  <button
                    type="button"
                    className={compactButtonClass}
                    onClick={() => setDraft((current) => ({
                      ...current,
                      asrProvider: "custom",
                      customAsr: true,
                      customBaseUrl: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
                      asrModel: "qwen3-asr-flash",
                    }))}
                  >
                    {t("国际站 Qwen3-ASR")}
                  </button>
                  <button
                    type="button"
                    className={compactButtonClass}
                    onClick={() => setDraft((current) => ({
                      ...current,
                      asrProvider: "custom",
                      customAsr: true,
                      customBaseUrl: "http://127.0.0.1:10095/v1",
                      asrModel: "paraformer-zh",
                    }))}
                  >
                    {t("本机 FunASR")}
                  </button>
                  <button
                    type="button"
                    className={compactButtonClass}
                    onClick={() => setDraft((current) => ({
                      ...current,
                      asrProvider: "custom",
                      customAsr: true,
                      customBaseUrl: "http://127.0.0.1:8765/v1",
                      asrModel: "Qwen/Qwen3-ASR-0.6B",
                    }))}
                  >
                    {t("本机 Qwen3-ASR (MLX)")}
                  </button>
                  <button
                    type="button"
                    className={compactButtonClass}
                    onClick={() => setDraft((current) => ({
                      ...current,
                      asrProvider: "local_whisper",
                      localWhisperBaseUrl: "http://127.0.0.1:9000/v1",
                      asrModel: "ggml-large-v3-turbo.bin",
                    }))}
                  >
                    {t("本机 Whisper.cpp")}
                  </button>
                </div>
                <label className="flex items-center gap-2 text-xs text-secondary">
                  <input type="checkbox" checked={draft.customAsr} onChange={(event) => {
                    const customAsr = event.target.checked;
                    setDraft((current) => {
                      const next = { ...current, customAsr };
                      return !customAsr && current.asrProvider === "custom" ? switchProvider(next, "asr", "groq") : next;
                    });
                  }} />
                  {t("用于转写")}
                </label>
                <label className="flex items-center gap-2 text-xs text-secondary">
                  <input type="checkbox" checked={draft.customLlm} onChange={(event) => {
                    const customLlm = event.target.checked;
                    setDraft((current) => {
                      const next = { ...current, customLlm };
                      return !customLlm && current.cleanupProvider === "custom" ? switchProvider(next, "cleanup", "groq") : next;
                    });
                  }} />
                  {t("用于润色")}
                </label>
              </>
            )}
            {provider.editableBaseUrl && provider.id !== "custom" && (
              <label className="block text-xs text-secondary">
                {t("本机地址")}
                <input
                  aria-label={`${provider.label} ${t("本机地址")}`}
                  value={provider.id === "ollama" ? draft.ollamaBaseUrl : draft.localWhisperBaseUrl}
                  onChange={(event) => {
                    setDraft(provider.id === "ollama"
                      ? { ...draft, ollamaBaseUrl: event.target.value }
                      : { ...draft, localWhisperBaseUrl: event.target.value });
                    setRowProbeFeedback(null);
                    if (provider.id === "ollama") setLocalCleanupStatus(null);
                  }}
                  placeholder={provider.defaultBaseUrl}
                  autoComplete="off"
                  spellCheck={false}
                  className={`${controlClass} mt-2 w-full font-mono text-xs`}
                />
              </label>
            )}
            {provider.id === "on_device" && (
              <p className="text-xs leading-5 text-tertiary">
                {t("在上方选择本机模型、查看运行时状态，并手动下载、取消或删除模型文件。SenseVoice 仅保留旧文件，不支持 MLX 推理。")}
              </p>
            )}
            {provider.id !== "on_device" && (
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
                  draft.providerKeys[provider.id]?.trim() && (
                    <button
                      type="button"
                      onClick={() => void testConnection(provider.id)}
                      aria-busy={rowProbing === provider.id}
                      disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProviderProbe}
                      className={`${compactButtonClass} min-w-36`}
                    >
                      {rowProbing === provider.id ? t("保存中…") : t("保存密钥")}
                    </button>
                  )
                ) : (
                  <button
                    type="button"
                    onClick={() => void testConnection(provider.id)}
                    aria-busy={rowProbing === provider.id}
                    disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProviderProbe}
                    className={`${compactButtonClass} min-w-36`}
                  >
                    {rowProbing === provider.id ? t("测试中…") : t("测试连接")}
                  </button>
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
            </div>
            )}
        </fieldset>
      </div>
    );
  };

  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t("语音服务")}
        description={t("选择转写和整理服务，测试并应用后生效。密钥可单独管理；Soniox 在听写时建立连接。")}
      />
      <SettingsGroup title={t("当前生效")}>
        <div className="space-y-2 px-4 py-4 text-sm text-secondary sm:px-5" aria-label={t("当前生效路线")}>
          <p>{t("转写")} · {providerById(currentAsrProvider)?.label} · {settings.asr_model || defaultModel(currentAsrProvider, "asr")}
            {settings.strict_offline_enabled && currentAsrProvider !== "on_device" && <span className="mt-1 block text-xs text-warning-ink">{t("严格离线模式已阻止此路线")}</span>}
          </p>
          <p>{t("AI 文字整理")} · {settings.cleanup_enabled
            ? `${providerById(currentCleanupProvider)?.label} · ${settings.cleanup_model || defaultModel(currentCleanupProvider, "llm")}`
            : t("关闭 · 只用本地规则")}
            {settings.strict_offline_enabled && settings.cleanup_enabled && (currentCleanupProvider !== "ollama" || !isLoopbackUrl(savedOllamaBaseUrl)) && <span className="mt-1 block text-xs text-warning-ink">{t("严格离线模式已阻止此路线")}</span>}
          </p>
        </div>
      </SettingsGroup>
      <SettingsGroup variant="surface" title={t("服务配置")} description={t(pendingChanges ? "有待应用的更改；当前听写仍使用上方路线。" : "调整服务或模型后，测试并应用使其生效。")}>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="min-w-0">
        <div className="px-5 py-4">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium text-primary">{t("严格离线模式")}</p>
              <p className="mt-1 text-xs leading-5 text-tertiary">{t("开启后会阻止云端回退和视觉请求；本机模型下载仍是单独的联网操作。不可用的本机整理会回退到本地规则或关闭整理。")}</p>
              {strictOfflineSaveError && <p role="alert" className="mt-1 text-xs leading-5 text-error-ink">{strictOfflineSaveError}</p>}
            </div>
            <Toggle
              checked={settings.strict_offline_enabled === true}
              disabled={strictOfflineSaving}
              onChange={(checked) => void updateStrictOfflineMode(checked)}
              label={t("严格离线模式")}
            />
          </div>
        </div>
        <div className="px-5 py-4">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-primary">{t("转写")}</p>
            <SettingsStatus label={asrStatus.label} tone={asrStatus.tone} />
          </div>
          <div className="vf-service-fields mt-3 flex w-full flex-wrap gap-2">
            <Select ref={asrProviderRef} aria-label={t("转写服务")} value={draft.asrProvider} onValueChange={(value) => changeAsrProvider(value)} className={`${controlClass} w-40 max-w-full`}>
              {asrOptions.map((provider) => (
                <option
                  key={provider.id}
                  value={provider.id}
                  disabled={provider.id === "on_device" && onDeviceModels.length > 0 && !hasSupportedDeviceModel && draft.asrProvider !== "on_device"}
                >{provider.id === "custom" ? t("兼容接口") : provider.id === "on_device" ? t("本机模型") : provider.label}</option>
              ))}
            </Select>
            <ModelControl
              id={draft.asrProvider}
              side="asr"
              value={draft.asrModel}
              ariaLabel={t("ASR 模型")}
              onChange={changeAsrModel}
              deviceModels={onDeviceModels}
            />
          </div>
          {draft.asrProvider === "dashscope" && (
            <div className="mt-3 max-w-sm">
              <label className="block text-xs font-medium text-secondary">
                {t("服务区域")}
                <Select
                  aria-label={t("服务区域")}
                  value={draft.dashscopeRegion}
                  onValueChange={(value) => changeDashscopeRegion(value)}
                  className={`${controlClass} mt-2 w-full text-xs`}
                >
                  <option value="beijing">{t("北京")} · dashscope.aliyuncs.com</option>
                  <option value="singapore">{t("新加坡")} · dashscope-intl.aliyuncs.com</option>
                </Select>
              </label>
              <p className="mt-1 text-xs leading-5 text-tertiary">{t("地区会与转写服务设置一起保存；Message WebSocket 使用该地区的固定官方地址。")}</p>
            </div>
          )}
          {draft.asrProvider === "local_whisper" && (
            <p className="mt-2 text-xs leading-5 text-tertiary">
              {t("本机 Whisper 走本机 HTTP，不会像 Handy 那样把 ggml 下进 App。先自己跑 whisper.cpp 或 speaches，模型名用对方接口要的名字。")}
            </p>
          )}
          {draft.asrProvider === "on_device" && (
            <div className="mt-3 space-y-3 rounded-lg bg-elevated px-3 py-3 text-xs leading-5">
              <div className="space-y-1 text-tertiary">
                <p>{selectedOnDeviceModel ? onDeviceStateCopy(selectedOnDeviceModel.state, t) : t("正在读取本机模型状态…")}</p>
                <p>{selectedOnDeviceModel ? onDeviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t) : t("模型文件状态尚未读取")}</p>
                <p>{selectedOnDeviceModel?.runtime_status === "loading"
                  ? t("本机模型正在加载；加载完成后会单独显示运行时状态。")
                  : onDeviceReady
                  ? selectedOnDeviceModel?.loaded ? t("能力握手通过，模型当前已加载。") : t("能力握手通过；模型是否已加载会单独显示。")
                  : t("只有运行时能力握手通过后，这个模型才会被标记为可用。")}</p>
                <p>{t("MLX 模型仅支持 Apple Silicon 和 macOS 14 或更新版本。下载模型需要网络；VoiceFlow 不会自动下载或安装模型。")}</p>
                {draft.asrModel === "sensevoice-small" && <p>{t("SenseVoice 是旧版文件保留项；没有 MLX 推理支持，不能用于本机听写。")}</p>}
                {selectedOnDeviceModel?.error && <p className="text-error-ink">{selectedOnDeviceModel.error}</p>}
              </div>
              {onDeviceActionError && <p role="alert" className="text-error-ink">{onDeviceActionError}</p>}
              {onDeviceModels.length === 0 ? (
                <button type="button" onClick={() => void refreshOnDeviceModels()} className={compactButtonClass}>{t("重新读取模型状态")}</button>
              ) : (
                <div className="space-y-2">
                  {onDeviceModels.map((model) => {
                    const downloading = model.state === "downloading";
                    const unsupported = model.id !== "sensevoice-small" && !model.platform_supported;
                    const actionBusy = onDeviceActionId === model.id;
                    return (
                      <div key={model.id} className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-base px-3 py-2">
                        <div className="min-w-0">
                          <p className="font-medium text-primary">{model.label} <span className="font-normal text-tertiary">· {formatModelBytes(model.bytes)}</span></p>
                          <p className="text-tertiary">{onDeviceStateCopy(model.state, t)}{downloading ? ` · ${formatModelBytes(model.downloaded_bytes)} / ${formatModelBytes(model.bytes)}` : ""}</p>
                          {model.runtime_status === "loading" && <p className="text-warning-ink">{t("本机模型正在加载")}</p>}
                          {model.id === "sensevoice-small" && <p className="text-warning-ink">{t("旧版文件保留项；没有 MLX 推理支持。")}</p>}
                          {model.error && <p className="text-error-ink">{model.error}</p>}
                        </div>
                        <div className="flex shrink-0 gap-2">
                          {downloading ? (
                            <button type="button" disabled={actionBusy} onClick={() => void runOnDeviceAction(model.id, "cancel")} className={compactButtonClass}>{t("取消下载")}</button>
                          ) : model.runtime_status === "loading" ? (
                            <button type="button" disabled={actionBusy} onClick={() => void runOnDeviceAction(model.id, "cancel_load")} className={compactButtonClass}>{actionBusy ? t("正在取消…") : t("取消模型加载")}</button>
                          ) : model.state === "ready" ? (
                            <button type="button" disabled={actionBusy} onClick={() => setConfirmDeleteModel(model.id)} className={compactDangerButtonClass}>{t("删除模型")}</button>
                          ) : (
                            <button type="button" disabled={actionBusy || unsupported} onClick={() => void runOnDeviceAction(model.id, "download")} className={compactButtonClass}>{unsupported ? t("当前设备不支持") : actionBusy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
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
            <Toggle checked={draftCleanupEnabled} onChange={(checked) => { setDraftCleanupEnabled(checked); setProbeSuccess(""); }} label={t("AI 文字整理")} />
          </div>
          {draftCleanupEnabled && (
            <div className="vf-service-fields mt-3 flex w-full flex-wrap gap-2">
              <Select aria-label={t("整理服务")} value={draft.cleanupProvider} onValueChange={(value) => changeCleanupProvider(value)} className={`${controlClass} w-40 max-w-full`}>
                {cleanupOptions.map((provider) => (
                  <option key={provider.id} value={provider.id}>{provider.id === "custom" ? t("兼容接口") : provider.label}</option>
                ))}
              </Select>
              <ModelControl
                id={draft.cleanupProvider}
                side="llm"
                value={draft.cleanupModel}
                ariaLabel={t("AI 文字整理模型")}
                onChange={changeCleanupModel}
              />
            </div>
          )}
          {draft.asrProvider === "assemblyai" && (
            <div role="note" className="mt-2 rounded-lg bg-elevated px-3 py-2.5 text-xs leading-5 text-tertiary">
              <p>{t("启用 AI 整理且满足条件的 120 秒内 Dictation 会提供 AssemblyAI 整理候选；不需要共同整理密钥，候选通过本地保护检查后才会采用。")}</p>
              <span className="mt-1 block">{t("候选缺失或失败时，只有已选共同整理服务的密钥已配置才会再请求一次整理；否则保留原始转写并使用本地保护回退。保护检查拒绝候选时也保留原稿，不再发起第二次整理。")}</span>
              <span className="mt-1 block">{t("AI Off、本地-only 和超过 120 秒的录音使用 AssemblyAI Sync 原始转写。长录音开启云端整理时需要已选的共同整理服务；服务不可用时保留原文并回退到本地或原始结果。保存密钥不代表服务访问已验证。")}</span>
            </div>
          )}
          {(draft.cleanupProvider === "ollama" || settings.strict_offline_enabled) && (
            <div className="mt-3 space-y-2 rounded-lg bg-elevated px-3 py-3 text-xs leading-5 text-tertiary">
              <p className="font-medium text-primary">{t("本机整理依赖")}: Ollama · qwen3.5:4b</p>
              <p>{t("VoiceFlow 不会安装 Ollama 或下载这个模型。不可用时会使用本地规则，或关闭 AI 整理；严格离线模式会阻止云端整理回退。")}</p>
              {localCleanupStatus && (
                <div role="status" className={localCleanupStatus.available ? "text-success-ink" : "text-warning-ink"}>
                  <p>{localCleanupStatus.available ? t("本机整理服务可用") : t("本机整理服务不可用")} · {localCleanupStatus.status}</p>
                  {localCleanupStatus.message && <p>{localCleanupStatus.message}</p>}
                </div>
              )}
              {localCleanupError && <p role="alert" className="text-error-ink">{localCleanupError}</p>}
              {!localCleanupRouteSaved && <p className="text-warning-ink">{t("先保存本机 Ollama 地址，再检查状态。")}</p>}
              <div className="flex flex-wrap gap-2">
                <button type="button" disabled={localCleanupChecking || !localCleanupRouteSaved} onClick={() => void checkLocalCleanupStatus()} className={compactButtonClass}>
                  {localCleanupChecking ? t("检查中…") : t("检查本机 Ollama")}
                </button>
                <button type="button" onClick={chooseLocalCleanup} className={compactButtonClass}>{t("使用本机 Qwen3.5:4b 整理")}</button>
              </div>
            </div>
          )}
        </div>
        </fieldset>
      <div id="provider-list" className="vf-provider-list flex flex-col">
        {[
          ...PROVIDERS.filter((provider) => selectedProviders.has(provider.id)).map(renderProvider),
          <div key="apply-service" className="flex flex-wrap items-center justify-end gap-3 px-5 py-4">
        {pendingChanges && (
          <button type="button" disabled={probing || rowProbing !== null || removing || strictOfflineSaving} onClick={() => {
            setDraft(draftFromSettings(settings));
            setDraftCleanupEnabled(settings.cleanup_enabled);
            setStageFail({});
            setProbeError(null);
            setCommitError(null);
            setProbeSuccess("");
            setTestedAsrRoute(null);
            setTestedCleanupRoute(null);
            asrProviderRef.current?.focus({ preventScroll: true });
          }} className={ghostButtonClass}>{t("放弃更改")}</button>
        )}
        {probeError && <p role="alert" className="text-xs leading-5 text-error-ink">{probeError}</p>}
        {probeSuccess && <p role="status" className={`text-xs leading-5 ${probeSuccess === "saved" ? "text-secondary" : "text-success-ink"}`}>{t(probeSuccess === "saved" ? "Soniox 凭据已保存；实时连接尚未测试。" : "测试通过，设置已保存")}</p>}
        {commitError && <p role="alert" className="text-xs leading-5 text-error-ink">{commitError}</p>}
        <button type="button" onClick={() => void runProbe()} aria-busy={probing} disabled={probing || rowProbing !== null || removing || strictOfflineSaving || !ready} className={`${buttonClass} min-w-36`}>
          {probing ? t("测试中…") : draft.asrProvider === "soniox" ? t("保存并应用") : t("测试并应用")}
        </button>
      </div>,
          <div key="provider-management" className="px-5 py-4">
          <button type="button" aria-expanded={showAvailableProviders} aria-controls="provider-list" aria-disabled={Boolean(otherProviderError)} onClick={() => {
            if (otherProviderError) return;
            setShowAvailableProviders((value) => !value);
          }} className={compactButtonClass}>
            <ChevronDown size={16} className={showAvailableProviders ? "rotate-180" : ""} aria-hidden="true" />{t("管理其他服务")}<span className="text-xs text-secondary">{otherConfiguredCount} {t("已配置")}</span>
          </button>
          <p className="mt-2 text-xs leading-5 text-secondary">{t("密钥按服务保存，转写和整理共用同一份。")}</p>
        </div>,
          ...PROVIDERS.filter((provider) => !selectedProviders.has(provider.id)).map(renderProvider),
        ]}
      </div>
      </SettingsGroup>
      <SettingsDisclosure title={t("本机性能诊断")} description={t("查看阶段延迟和交付计数，仅保存在本机。")} error={metricsError}>
        <LatencyMetricsViewer onErrorChange={setMetricsError} />
      </SettingsDisclosure>
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
}: {
  id: ProviderId;
  side: "asr" | "llm";
  value: string;
  ariaLabel: string;
  onChange: (value: string) => void;
  deviceModels?: OnDeviceModelStatus[];
}) {
  const { t } = useI18n();
  const definition = providerById(id);
  if (!definition) return null;
  const field = side === "asr" ? definition.asrModelField : definition.llmModelField;
  const options = side === "asr" ? definition.asrModels : definition.llmModels;
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
