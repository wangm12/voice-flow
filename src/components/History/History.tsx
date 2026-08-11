import { invoke } from "@tauri-apps/api/core";
import { Check, CheckCircle2, CircleHelp, CircleX, Copy, Download, Eraser, History as HistoryIcon, LoaderCircle, RotateCcw, Trash2, TriangleAlert, X } from "lucide-react";
import { useMemo, useState } from "react";
import { colors, radius } from "../../lib/theme";
import { IconButton } from "../IconButton";
import { useI18n } from "../../lib/i18n";
import { SettingsAlert, SettingsGroup, SettingsPageHeader, SettingsShell } from "../SettingsLayout";

export type HistoryItem = {
  id: number;
  created_at: string;
  raw_text: string;
  final_text: string;
  cleanup_status?: string | null;
  duration: number;
  degraded?: boolean;
  degraded_reason?: string | null;
  status: string;
  delivery_method?: string | null;
  fallback_reason?: string | null;
  context_profile_id?: string | null;
  retryable?: boolean;
};

export function History({ items, reload, hasMore, loading, onLoadMore, error, onRetry }: { items: HistoryItem[]; reload: () => void; hasMore: boolean; loading: boolean; onLoadMore: () => void; error?: string | null; onRetry?: () => void }) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [dataError, setDataError] = useState<string | null>(null);
  const [dataMessage, setDataMessage] = useState<string | null>(null);
  const visible = useMemo(
    () => items.filter((item) => `${item.raw_text} ${item.final_text}`.toLowerCase().includes(query.toLowerCase())),
    [items, query],
  );

  const exportData = async () => {
    setDataError(null);
    setDataMessage(null);
    try {
      const path = await invoke<string>("export_history");
      setDataMessage(t("已导出到：") + path);
    } catch (reason) {
      setDataError(t("导出失败：") + String(reason));
    }
  };

  const clearAll = async () => {
    if (!window.confirm(t("这会删除全部历史文字、恢复音频和本地用量，且无法撤销。确定继续吗？"))) return;
    setDataError(null);
    setDataMessage(null);
    try {
      await invoke("clear_all_data");
      reload();
    } catch (reason) {
      setDataError(t("清空失败：") + String(reason));
    }
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("历史")} description={t("每一次表达，都留在这里。")} actions={<div className="flex shrink-0 items-center gap-1"><IconButton label={t("下载")} aria-label={t("下载历史记录")} icon={<Download size={16} aria-hidden="true" />} onClick={() => void exportData()} /><IconButton label={t("清空")} aria-label={t("清空全部数据")} tone="danger" icon={<Eraser size={16} aria-hidden="true" />} disabled={!items.length || loading} onClick={() => void clearAll()} /></div>} />
      <div className="mt-7 flex flex-wrap items-center justify-between gap-3 border-y border-border py-3">
        <input aria-label={t("搜索历史记录")} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("搜索历史…")} className={`h-9 w-64 max-w-full ${radius.control} border ${colors.border} ${colors.bg.elevated} px-3 text-sm text-primary outline-none transition-colors placeholder:text-tertiary focus:border-accent`} />
        <p className="text-xs text-tertiary">{items.length ? `${items.length}${hasMore ? "+" : ""} ${t("条")} · ${hasMore ? t("还有更早记录") : t("已显示全部")}` : ""}</p>
      </div>
      {dataMessage && <p role="status" className="mt-4 text-xs text-success">{dataMessage}</p>}
      {dataError && <p role="alert" className="mt-4 text-xs text-error">{dataError}</p>}
      {error && <div className="mt-4"><SettingsAlert onRetry={onRetry ?? reload} retryLabel={t("重新加载")}>{error}</SettingsAlert></div>}
      {loading && items.length === 0 ? (
        <HistoryLoading />
      ) : visible.length ? (
        <>
          <SettingsGroup title={t("记录")} description={`${visible.length}${hasMore ? "+" : ""} ${t("条")}`}>
            {visible.map((item) => <HistoryRow key={item.id} item={item} reload={reload} />)}
          </SettingsGroup>
          {hasMore && <LoadMoreButton loading={loading} searching={Boolean(query.trim())} onClick={onLoadMore} />}
        </>
      ) : items.length ? (
        <>
          <p role="status" className="mt-8 text-sm text-secondary">{t("当前已加载的记录中没有匹配项。")}</p>
          {hasMore && <LoadMoreButton loading={loading} searching={Boolean(query.trim())} onClick={onLoadMore} />}
        </>
      ) : (
        <Empty />
      )}
    </SettingsShell>
  );
}

function HistoryRow({ item, reload }: { item: HistoryItem; reload: () => void }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [showRaw, setShowRaw] = useState(false);
  const status = statusPresentation(item.status, t);
  const StatusIcon = status.icon;
  const reason = displayReason(item, t);
  const cleanupLabel = cleanupStatusLabel(item.cleanup_status, t);

  const run = async (command: string, reloadAfter = false) => {
    setBusy(true);
    setError(null);
    try {
      await invoke(command, { id: item.id });
      if (reloadAfter) reload();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-3 border-t border-border px-4 py-4 first:border-t-0 sm:px-5">
      <span role="img" aria-label={busy ? t("处理中") : status.label} className="flex h-7 w-7 shrink-0 items-center justify-center">
        {busy ? <LoaderCircle size={16} className="animate-spin text-secondary" aria-hidden="true" /> : <StatusIcon size={16} className={status.iconClass} aria-hidden="true" />}
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm text-primary">{item.final_text || item.raw_text || t("识别失败")}</p>
        <p className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-tertiary"><span>{relativeTime(item.created_at, t)} · {item.duration.toFixed(1)} {t("秒")}</span>{cleanupLabel && <><span aria-hidden="true">·</span><span>{cleanupLabel}</span></>}</p>
        {item.raw_text && item.final_text && <button type="button" className="mt-2 text-xs text-secondary underline decoration-border underline-offset-2 transition-colors hover:text-primary" onClick={() => setShowRaw((value) => !value)}>{showRaw ? t("隐藏原文") : t("查看原文")}</button>}
        {showRaw && item.raw_text && <div className="mt-2 space-y-1 rounded-lg bg-elevated px-3 py-2 text-xs"><p className="text-tertiary">{t("清理前")}</p><p className="whitespace-pre-wrap text-secondary">{item.raw_text}</p><p className="pt-1 text-tertiary">{t("清理后")}</p><p className="whitespace-pre-wrap text-primary">{item.final_text || item.raw_text}</p></div>}
        {reason && <p className={`mt-1 text-xs ${status.detailClass}`}>{reason}</p>}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <IconButton label={t("复制")} aria-label={t("复制这条历史记录到剪贴板")} icon={<Copy size={15} aria-hidden="true" />} disabled={busy} onClick={() => void run("repaste_history")} />
        {item.retryable && (item.status === "failed" || item.status === "degraded") && (
          <IconButton label={t("重试")} aria-label={t("重试这条历史记录")} icon={<RotateCcw size={15} aria-hidden="true" />} disabled={busy} onClick={() => void run("retry_dictation", true)} />
        )}
        {confirmingDelete ? (
          <span className="flex items-center gap-1">
            <IconButton label={t("确认删除")} tone="warning" aria-label={t("确认删除这条历史记录")} icon={<Check size={15} aria-hidden="true" />} disabled={busy} onClick={() => { setConfirmingDelete(false); void run("delete_history", true); }} />
            <IconButton label={t("取消删除")} aria-label={t("取消删除")} icon={<X size={15} aria-hidden="true" />} disabled={busy} onClick={() => setConfirmingDelete(false)} />
          </span>
        ) : (
          <IconButton label={t("删除")} tone="danger" aria-label={`${t("删除")} ${item.final_text || item.raw_text || item.id}`} icon={<Trash2 size={15} aria-hidden="true" />} disabled={busy} onClick={() => setConfirmingDelete(true)} />
        )}
      </div>
      {error && <p role="alert" className="basis-full text-xs text-error">{error}</p>}
    </div>
  );
}

const degradedReasonLabels: Record<string, string> = {
  asr_failed: "语音识别失败，没有生成可用文字，可重试",
  crash_recovery: "上次录音未完成，已保留音频，可重试",
  interrupted_recording: "录音中断，未完成识别",
  llm_cleanup_failed: "文字整理失败，原始转录已保留，可重试",
  llm_cleanup_empty: "文字整理没有返回结果，原始转录已保留，可重试",
  partial_asr_failure: "部分语音未识别，已保留已识别内容，可重试",
  partial_llm_cleanup_failure: "部分内容未完成整理，原始转录已保留，可重试",
};

const deliveryReasonLabels: Record<string, string> = {
  delivery_failed: "自动插入失败，文字已保存，可复制",
  accessibility_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  browser_permission_required: "未能自动粘贴，文字已复制到剪贴板，请手动粘贴",
  input_unavailable: "未找到可编辑输入框，文字已复制到剪贴板，请手动粘贴",
  input_changed: "输入框已变化，文字已复制到剪贴板，请手动粘贴",
  target_changed: "输入目标已变化，文字已复制到剪贴板，请手动粘贴",
  target_unavailable: "未能确认输入目标，文字已复制到剪贴板，请手动粘贴",
  paste_failed: "自动粘贴未完成，文字已复制到剪贴板，请手动粘贴",
  retry_clipboard_only: "自动插入失败，文字已复制到剪贴板",
};

function statusPresentation(status: string, t: (source: string) => string) {
  if (status === "failed") {
    return { label: t("识别失败"), icon: CircleX, surfaceClass: "bg-error/10", iconClass: "text-error", detailClass: "text-error" };
  }
  if (status === "degraded") {
    return { label: t("已保留原文"), icon: TriangleAlert, surfaceClass: "bg-warning/10", iconClass: "text-warning", detailClass: "text-warning" };
  }
  if (status === "copied") {
    return { label: t("已复制"), icon: CheckCircle2, surfaceClass: "bg-success/10", iconClass: "text-success", detailClass: "text-secondary" };
  }
  if (status === "ok") {
    return { label: t("已完成"), icon: CheckCircle2, surfaceClass: "bg-success/10", iconClass: "text-success", detailClass: "text-secondary" };
  }
  return { label: t("状态未知"), icon: CircleHelp, surfaceClass: "bg-elevated", iconClass: "text-tertiary", detailClass: "text-secondary" };
}

function displayReason(item: HistoryItem, t: (source: string) => string): string | null {
  const messages = [
    item.degraded_reason ? degradedReasonLabels[item.degraded_reason] : undefined,
    item.fallback_reason ? deliveryReasonLabels[item.fallback_reason] : undefined,
  ].filter((message, index, all): message is string => Boolean(message) && all.indexOf(message) === index);
  if (messages.length) return messages.map((message) => t(message)).join(" · ");
  if (item.status === "failed") return t("处理失败，没有生成可用文字，可重试");
  if (item.status === "degraded" || item.degraded) return t("处理未完成，原始转录已保留，可重试");
  return null;
}

function cleanupStatusLabel(status: string | null | undefined, t: (source: string) => string): string | null {
  if (status === "ai_success") return t("AI 已整理");
  if (status === "local_only") return t("仅本地整理");
  if (status === "snippet_bypass") return t("语音片段");
  return null;
}

function LoadMoreButton({ loading, searching, onClick }: { loading: boolean; searching: boolean; onClick: () => void }) {
  const { t } = useI18n();
  return (
    <button type="button" disabled={loading} onClick={onClick} className={`mt-4 w-full ${radius.control} border ${colors.border} px-4 py-3 text-sm text-secondary transition-colors hover:bg-elevated hover:text-primary disabled:opacity-50`}>
      {loading ? (searching ? t("正在加载更多记录…") : t("正在加载更早的记录…")) : (searching ? t("加载更多记录以继续搜索") : t("加载更早的记录"))}
    </button>
  );
}

function HistoryLoading() {
  const { t } = useI18n();
  return (
    <div role="status" aria-label={t("正在加载历史记录")} aria-busy="true" className="mt-6 space-y-3">
      <span className="sr-only">{t("正在加载历史记录")}…</span>
      {[0, 1, 2].map((index) => (
        <div key={index} aria-hidden="true" className="flex items-center gap-3 border-t border-border py-4 first:border-t-0">
          <div className="h-7 w-7 shrink-0 animate-pulse rounded-full bg-elevated" />
          <div className="min-w-0 flex-1 space-y-2">
            <div className="h-4 w-3/5 animate-pulse rounded bg-elevated" />
            <div className="h-3 w-2/5 animate-pulse rounded bg-elevated" />
          </div>
        </div>
      ))}
    </div>
  );
}

function relativeTime(value: string, t: (source: string) => string) {
  const seconds = Math.max(0, (Date.now() - new Date(`${value}Z`).getTime()) / 1000);
  if (seconds < 60) return t("刚刚");
  if (seconds < 3600) return `${Math.floor(seconds / 60)} ${t("分钟")}${t("前")}`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)} ${t("小时")}${t("前")}`;
  return `${Math.floor(seconds / 86400)} ${t("天")}${t("前")}`;
}

function Empty() {
  const { t } = useI18n();
  return (
    <div className="flex min-h-[260px] flex-col items-center justify-center border-t border-border text-center">
      <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-xl bg-elevated text-tertiary"><HistoryIcon size={19} /></div>
      <p className="text-base font-medium text-primary">{t("还没有记录，按热键说一句吧")}</p>
      <p className="mt-2 text-sm text-tertiary">{t("你的语音记录会出现在这里。")}</p>
    </div>
  );
}
