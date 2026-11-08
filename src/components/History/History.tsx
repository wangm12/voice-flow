import { Select } from "../Select";
import { invoke } from "@tauri-apps/api/core";
import { Check, CheckCircle2, CircleHelp, CircleX, Copy, Download, Eraser, History as HistoryIcon, LoaderCircle, Pencil, RotateCcw, Save, Trash2, TriangleAlert, Wand2 } from "lucide-react";
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { friendlyDictionaryLearningError } from "../../lib/settingsError";
import { secondaryButtonClass, compactButtonClass, dangerActionButtonClass, ghostButtonClass, inputClass, colors, radius, focusRingClass } from "../../lib/theme";
import { ActionMenu } from "../ActionMenu";
import { IconButton } from "../IconButton";
import { useI18n } from "../../lib/i18n";
import { deliveryReasonLabels } from "../../lib/deliveryCopy";
import { cleanupCandidateLabel, cleanupGuardReasonLabel } from "../../lib/cleanupRejection";
import { ConfirmDialog } from "../ConfirmDialog";
import { SettingsAlert, SettingsPageHeader, SettingsShell } from "../SettingsLayout";

export type HistoryItem = {
  id: number;
  created_at: string;
  raw_text: string;
  asr_text?: string | null;
  engine?: string | null;
  provider_cleaned_candidate?: string | null;
  cleanup_rejected_candidate?: string | null;
  cleanup_guard_reason?: string | null;
  cleanup_attempt_kind?: string | null;
  cleanup_transformations?: string[] | null;
  cleanup_failure_reason?: string | null;
  cleanup_final_guard_reason?: string | null;
  final_text: string;
  cleanup_status?: string | null;
  duration: number;
  degraded?: boolean;
  degraded_reason?: string | null;
  status: string;
  delivery_method?: string | null;
  fallback_reason?: string | null;
  delivery_error_code?: string | null;
  delivery_user_reason?: string | null;
  context_profile_id?: string | null;
  retryable?: boolean;
  revision_count?: number;
};

type DictionarySuggestion = {
  pair_key: string;
  before_span: string;
  after: string;
};

type HistoryRevision = {
  revision_id: number;
  created_at: string;
  final_text: string;
  cleanup_status?: string | null;
  revision_reason: string;
};

type ManualHistorySaveResponse = {
  revision_id?: number | null;
  changed?: boolean;
  learning?: { status?: string; hits?: number; required?: number };
};

const cleanupAttemptLabels: Record<string, string> = {
  success: "已完成整理请求",
  not_called: "未调用整理服务",
  credentials_unavailable: "缺少整理服务凭据",
  network: "网络连接失败",
  timeout: "整理请求超时",
  context_authorization_changed: "上下文授权已变化",
  unauthorized: "整理服务未授权",
  rate_limited: "整理服务限流",
  server: "整理服务暂时不可用",
  protocol: "整理服务响应无法确认",
  provider_guard: "服务商候选未通过保真检查",
  local_native: "本机整理失败",
  other: "整理请求失败",
  final_guard: "最终整理结果未通过保真检查",
};

const cleanupFailureLabels: Record<string, string> = {
  network_failure: "网络连接失败",
  request_timeout: "整理请求超时",
  context_authorization_changed: "上下文授权已变化",
  remote_unauthorized: "整理服务未授权",
  remote_rate_limited: "整理服务限流",
  remote_server_error: "整理服务暂时不可用",
  credentials_unavailable: "缺少整理服务凭据",
  empty_completion: "整理服务没有返回可用内容",
  other_provider_failure: "整理请求失败",
  native_input_too_large: "本机整理输入超出支持范围",
  native_invalid_response: "本机整理响应格式无效",
  native_memory_pressure_critical: "本机内存压力过高，未启动整理",
  native_local_memory_unavailable: "本机可用内存不足，未启动整理",
  native_instance_not_ready: "本机整理实例尚未就绪，未启动请求",
  native_invalid_endpoint: "本机整理服务地址无法使用",
  native_unavailable: "本机整理服务不可用",
  native_response_too_large: "本机整理响应超出支持范围",
  protocol_invalid_response: "整理服务响应无法确认",
  protocol_missing_finish: "整理服务响应未完整结束",
  protocol_missing_stop: "整理服务响应未完整结束",
  protocol_truncated: "整理服务响应被截断",
  protocol_tool_call: "整理服务返回了不支持的工具调用",
  protocol_multiple_choices: "整理服务返回了多个结果",
  protocol_continued_after_finish: "整理服务响应格式无法确认",
  protocol_continued_after_done: "整理服务响应格式无法确认",
  protocol_unsupported_content: "整理服务返回了不支持的内容",
  protocol_unavailable_text: "整理服务没有返回可用文字",
  protocol_response_budget: "整理服务响应超过允许长度",
  provider_guard_explicit_layout_changed: "服务商候选的列表或结构未通过保真检查",
  provider_guard_source_script_changed: "服务商候选的语言或文字体系未通过保真检查",
  provider_guard_protected_token_changed: "服务商候选的名称、数字或事实未通过保真检查",
};

const cleanupTransformationLabels: Record<string, string> = {
  strip_internal_cleanup_metadata: "移除整理用内部标记",
  restore_spoken_layout: "恢复口述中的列表或结构",
  restore_prepared_preamble: "恢复本机准备稿中的前置内容",
};

function cleanupFactLabel(value: string | null | undefined, labels: Record<string, string>, t: (source: string) => string, unknown: string): string | null {
  if (value == null) return null;
  const known = labels[value];
  if (known) return t(known);
  if (value.startsWith("final_") || value.startsWith("provider_guard_") || value.startsWith("protocol_")) {
    return t(cleanupGuardReasonLabel(value));
  }
  return t(unknown);
}

function historyCommandErrorMessage(reason: unknown, t: (source: string) => string): string {
  const dictionaryError = friendlyDictionaryLearningError(reason, t);
  if (dictionaryError) return dictionaryError;
  const message = reason instanceof Error ? reason.message : String(reason);
  if (message === "history_dictionary_learning_retry") {
    return t("历史版本已保存，但词典学习暂时未完成；请再次保存重试。");
  }
  if (message === "System memory pressure is critical; selected text and existing History were left unchanged") {
    return t("系统内存压力过高，未运行本地整理；所选历史文本和已有历史记录均未更改");
  }
  if (message === "Local memory budget is unavailable; selected text and existing History were left unchanged") {
    return t("本机内存预算不足，未运行本地整理；所选历史文本和已有历史记录均未更改");
  }
  const capacity = /^history_native_input_too_large:(\d+):(\d+)$/.exec(message);
  if (!capacity) return message;
  return `${t("本机整理输入超出支持上限")}：${t("实际")} ${capacity[1]} / ${t("上限")} ${capacity[2]} ${t("UTF-8 字节")}。${t("原文已保留，可缩短后重试。")}`;
}

export function History({ items, reload, hasMore, loading, onLoadMore, error, onRetry, onQueryChange }: { items: HistoryItem[]; reload: () => void; hasMore: boolean; loading: boolean; onLoadMore: () => void; error?: string | null; onRetry?: () => void; onQueryChange?: (query: string) => void }) {
  const { t, language } = useI18n();
  const [query, setQuery] = useState("");
  const [dataError, setDataError] = useState<string | null>(null);
  const [dataMessage, setDataMessage] = useState<string | null>(null);
  const [pendingExport, setPendingExport] = useState<"records" | "audio" | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [clearError, setClearError] = useState<string | null>(null);
  const clearingRef = useRef(false);
  const queryInitializedRef = useRef(false);
  const pendingDeleteFocusRef = useRef<{ id: number; order: number[]; moved: boolean; initialFocus: Element | null; onFocusIn: (event: FocusEvent) => void; awaitingRemoval?: boolean } | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const normalizedQuery = query.trim().toLowerCase();
  const visible = useMemo(
    () => items.filter((item) => !normalizedQuery || `${item.asr_text ?? ""} ${item.provider_cleaned_candidate ?? ""} ${item.cleanup_rejected_candidate ?? ""} ${item.raw_text} ${item.final_text}`.toLowerCase().includes(normalizedQuery)),
    [items, normalizedQuery],
  );

  const beginDeleteFocusTracking = (id: number) => {
    const initialFocus = document.activeElement;
    const pending = { id, order: visible.map((entry) => entry.id), moved: false, initialFocus, onFocusIn: (_event: FocusEvent) => {}, awaitingRemoval: false };
    pending.onFocusIn = (event) => { if (event.target !== initialFocus) pending.moved = true; };
    pendingDeleteFocusRef.current = pending;
    document.addEventListener("focusin", pending.onFocusIn);
  };

  const restoreDeleteFocusAfterRemoval = (id: number) => {
    const pending = pendingDeleteFocusRef.current;
    if (pending?.id === id) pending.awaitingRemoval = true;
  };

  const cancelDeleteFocusTracking = (id: number) => {
    const pending = pendingDeleteFocusRef.current;
    if (pending?.id !== id) return;
    document.removeEventListener("focusin", pending.onFocusIn);
    pendingDeleteFocusRef.current = null;
  };

  useLayoutEffect(() => {
    const pending = pendingDeleteFocusRef.current;
    if (!pending?.awaitingRemoval || items.some((entry) => entry.id === pending.id)) return;
    document.removeEventListener("focusin", pending.onFocusIn);
    pendingDeleteFocusRef.current = null;
    if (pending.moved) return;
    const removedIndex = pending.order.indexOf(pending.id);
    const neighborIds = [...pending.order.slice(removedIndex + 1), ...pending.order.slice(0, removedIndex).reverse()];
    for (const neighborId of neighborIds) {
      if (!visible.some((entry) => entry.id === neighborId)) continue;
      const row = document.querySelector<HTMLElement>(`[data-history-id="${neighborId}"]`);
      const menu = row?.querySelector<HTMLButtonElement>("[aria-haspopup='menu']");
      if (menu) { menu.focus({ preventScroll: true }); return; }
    }
    searchRef.current?.focus({ preventScroll: true });
  }, [items, visible]);

  useEffect(() => {
    if (!onQueryChange) return;
    if (!queryInitializedRef.current) {
      queryInitializedRef.current = true;
      return;
    }
    const timer = window.setTimeout(() => onQueryChange(query.trim()), 200);
    return () => window.clearTimeout(timer);
  }, [onQueryChange, query]);

  const exportData = async () => {
    if (pendingExport) return;
    setPendingExport("records");
    setDataError(null);
    setDataMessage(null);
    try {
      const path = await invoke<string>("export_history");
      setDataMessage(t("已导出到：") + path);
    } catch (reason) {
      setDataError(t("导出失败：") + String(reason));
    } finally {
      setPendingExport(null);
    }
  };

  const exportGold = async () => {
    if (pendingExport) return;
    setPendingExport("audio");
    setDataError(null);
    setDataMessage(null);
    try {
      const path = await invoke<string>("export_gold_corpus");
      setDataMessage(t("已导出音频到：") + path);
    } catch (reason) {
      setDataError(t("导出音频失败：") + String(reason));
    } finally {
      setPendingExport(null);
    }
  };

  const clearAll = () => {
    setClearError(null);
    setConfirmClear(true);
  };

  const confirmClearAll = async () => {
    if (clearingRef.current) return;
    clearingRef.current = true;
    setClearing(true);
    setClearError(null);
    setDataError(null);
    setDataMessage(null);
    try {
      await invoke("clear_all_data");
      setConfirmClear(false);
      reload();
    } catch (reason) {
      setClearError(t("清空失败：") + String(reason));
    } finally {
      clearingRef.current = false;
      setClearing(false);
    }
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("历史记录")} description={t("查看、复制和恢复这台 Mac 上的听写记录。")} actions={
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" aria-label={t("导出记录")} aria-busy={pendingExport === "records"} disabled={pendingExport !== null} className={`${secondaryButtonClass} min-w-28 whitespace-nowrap`} onClick={() => void exportData()}><Download size={15} className="shrink-0" aria-hidden="true" />{t(pendingExport === "records" ? "导出中…" : "导出记录")}</button>
          <button type="button" aria-label={t("导出音频")} aria-busy={pendingExport === "audio"} disabled={pendingExport !== null} className={`${secondaryButtonClass} min-w-28 whitespace-nowrap`} onClick={() => void exportGold()}><Download size={15} className="shrink-0" aria-hidden="true" />{t(pendingExport === "audio" ? "导出中…" : "导出音频")}</button>
          <IconButton label={t("清空")} aria-label={t("清空全部数据")} tone="danger" icon={<Eraser size={16} aria-hidden="true" />} disabled={loading || pendingExport !== null} onClick={() => void clearAll()} />
        </div>
      } />
      {dataMessage && <p role="status" className="mt-3 break-words text-xs leading-5 text-success-ink">{dataMessage}</p>}
      {dataError && <p role="alert" className="mt-3 break-words text-xs leading-5 text-error-ink">{dataError}</p>}
      <div className="mt-6 flex flex-wrap items-center justify-between gap-3 border-y border-border py-3">
        <input ref={searchRef} aria-label={t("搜索历史记录")} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("搜索历史…")} className={`${inputClass} w-64 max-w-full`} />
        <p className="text-xs text-tertiary">{items.length ? `${items.length}${hasMore ? "+" : ""} ${t("条")} · ${hasMore ? t("还有更早记录") : t("已显示全部")}` : ""}</p>
      </div>
      {error && <div className="mt-4"><SettingsAlert onRetry={onRetry ?? reload} retryLabel={t("重新加载")}>{error}</SettingsAlert></div>}
      {loading && items.length === 0 ? (
        <HistoryLoading />
      ) : visible.length ? (
        <>
          <div className="mt-6">
            {visible.map((item, index) => {
              const day = historyDay(item.created_at, t, language);
              const previous = index > 0 ? historyDay(visible[index - 1].created_at, t, language) : null;
              return <Fragment key={item.id}>
                {day.key !== previous?.key && <h2 className="vf-history-date px-5 pb-2 pt-6 text-sm font-semibold leading-5 text-primary first:pt-0">{day.label}</h2>}
                <HistoryRow item={item} reload={reload} windowed={visible.length > 50} onDeleteStart={beginDeleteFocusTracking} onDeleteSuccess={restoreDeleteFocusAfterRemoval} onDeleteFailure={cancelDeleteFocusTracking} />
              </Fragment>;
            })}
          </div>
          {hasMore && <LoadMoreButton loading={loading} searching={Boolean(query.trim())} onClick={onLoadMore} />}
        </>
      ) : items.length ? (
        <>
          <p role="status" className="mt-8 text-sm text-secondary">{t("当前已加载的记录中没有匹配项。")}</p>
          {hasMore && <LoadMoreButton loading={loading} searching={Boolean(query.trim())} onClick={onLoadMore} />}
        </>
      ) : query.trim() ? (
        <p role="status" className="mt-8 text-sm text-secondary">{t("全库没有匹配的记录。")}</p>
      ) : (
        <Empty />
      )}
      <ConfirmDialog
        open={confirmClear}
        title={t("清空全部数据")}
        description={t("这会删除全部历史文字及迁移备份、恢复音频、训练音频和本地用量。Keychain 中的 API Key 会保留。删除后无法恢复，确定继续吗？")}
        confirmLabel={t(clearing ? "正在清空…" : "清空全部数据")}
        cancelLabel={t("取消")}
        busy={clearing}
        error={clearError}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void confirmClearAll()}
      />
    </SettingsShell>
  );
}

function HistoryRow({ item, reload, windowed = false, onDeleteStart, onDeleteSuccess, onDeleteFailure }: { item: HistoryItem; reload: () => void; windowed?: boolean; onDeleteStart?: (id: number) => void; onDeleteSuccess?: (id: number) => void; onDeleteFailure?: (id: number) => void }) {
  const { t, language } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [showRaw, setShowRaw] = useState(false);
  const [showRevisions, setShowRevisions] = useState(false);
  const [revisions, setRevisions] = useState<HistoryRevision[] | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(item.final_text || item.raw_text);
  const [showReclean, setShowReclean] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (copyTimer.current) clearTimeout(copyTimer.current); }, []);
  const [operation, setOperation] = useState("cleanup");
  const [dictionaryCandidates, setDictionaryCandidates] = useState<DictionarySuggestion[]>([]);
  const actionsRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const cancelDeleteRef = useRef<HTMLButtonElement>(null);
  const editFocusPending = useRef(false);
  const deleteTriggerRef = useRef<HTMLButtonElement | null>(null);
  const status = statusPresentation(item.status, finalDeliveryMethod(item), t);
  const StatusIcon = status.icon;
  const reason = displayReason(item, t);
  const cleanupLabel = cleanupStatusLabel(item.cleanup_status, t);
  const canRetry = Boolean(item.retryable && (item.status === "failed" || item.status === "degraded"));
  useLayoutEffect(() => {
    if (editing) {
      editFocusPending.current = true;
      editorRef.current?.focus({ preventScroll: true });
    } else if (editFocusPending.current) {
      editFocusPending.current = false;
      actionsRef.current?.querySelector<HTMLButtonElement>("[data-history-edit]")?.focus({ preventScroll: true });
    }
  }, [editing]);
  useLayoutEffect(() => {
    if (confirmingDelete) cancelDeleteRef.current?.focus({ preventScroll: true });
  }, [confirmingDelete]);
  const cancelEditing = () => {
    setDraft(item.final_text || item.raw_text);
    setEditing(false);
    setError(null);
    setSaveNotice(null);
  };
  const cancelDelete = () => {
    setConfirmingDelete(false);
    deleteTriggerRef.current?.focus({ preventScroll: true });
  };

  const run = async (command: string, args: Record<string, unknown> = {}, reloadAfter = false): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      await invoke(command, { id: item.id, ...args });
      if (command === "delete_history") onDeleteSuccess?.(item.id);
      if (reloadAfter) reload();
      return true;
    } catch (reason) {
      if (command === "delete_history") onDeleteFailure?.(item.id);
      setError(historyCommandErrorMessage(reason, t));
      if (reloadAfter) reload();
      return false;
    } finally {
      setBusy(false);
    }
  };

  const saveManualRevision = async () => {
    setBusy(true);
    setError(null);
    setSaveNotice(null);
    try {
      const saved = await invoke<ManualHistorySaveResponse>("save_history_revision", {
        id: item.id,
        final_text: draft,
        revision_reason: "manual_edit",
      });
      reload();
      if (
        saved?.learning?.status === "partial_settings_failure"
        || saved?.learning?.status === "partial_dictionary_sync_failure"
      ) {
        setSaveNotice(t("历史版本已保存；个人词典同步未完成，请再次保存重试。"));
      } else {
        setEditing(false);
      }
      if (saved?.learning?.status === "pending") {
        const hits = saved.learning.hits ?? 0;
        const required = saved.learning.required ?? 0;
        setSaveNotice(t("历史版本已保存；词典候选仍在收集中") + " (" + hits + "/" + required + ")");
      }
      await suggestDictionaryCandidates(item.final_text || item.raw_text, draft);
    } catch (reason) {
      setError(historyCommandErrorMessage(reason, t));
    } finally {
      setBusy(false);
    }
  };

  const toggleRevisions = async () => {
    if (showRevisions) {
      setShowRevisions(false);
      return;
    }
    if (revisions) {
      setShowRevisions(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<HistoryRevision[]>("get_history_revisions", { id: item.id });
      setRevisions(next);
      setShowRevisions(true);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const suggestDictionaryCandidates = async (before: string, after: string) => {
    try {
      const candidates = await invoke<DictionarySuggestion[]>("suggest_dictionary_entries", { before, after });
      setDictionaryCandidates(Array.isArray(candidates) ? candidates.slice(0, 3) : []);
    } catch {
      setDictionaryCandidates([]);
    }
  };

  const confirmDictionaryCandidate = async (candidate: DictionarySuggestion) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("promote_learn_pair", {
        pairKey: candidate.pair_key,
        beforeSurface: candidate.before_span,
        afterSurface: candidate.after,
        historyId: item.id,
      });
      setDictionaryCandidates((current) => current.filter((value) => value.pair_key !== candidate.pair_key));
    } catch (reason) {
      setError(historyCommandErrorMessage(reason, t));
    } finally {
      setBusy(false);
    }
  };

  const hasCleanupFacts = item.cleanup_attempt_kind != null || item.cleanup_failure_reason != null || item.cleanup_final_guard_reason != null || (item.cleanup_transformations != null && item.cleanup_transformations.length > 0);
  const hasOriginalDetails = Boolean(item.asr_text || item.provider_cleaned_candidate || item.cleanup_rejected_candidate || (item.raw_text && item.final_text));

  return (
    <div data-history-id={item.id} className={`vf-history-row flex flex-wrap items-start gap-3 border-t border-border px-5 py-4 first:border-t-0${windowed ? " history-list--windowed" : ""}`} onKeyDown={(event) => {
      if (event.key !== "Escape" || event.nativeEvent.isComposing || busy) return;
      if (!editing && !confirmingDelete) return;
      event.preventDefault();
      event.stopPropagation();
      if (editing) cancelEditing();
      else cancelDelete();
    }}>
      <span role="img" aria-label={busy ? t("处理中…") : status.label} className="flex h-7 w-7 shrink-0 items-center justify-center">
        {busy ? <LoaderCircle size={16} className="animate-spin text-secondary motion-reduce:animate-none" aria-hidden="true" /> : <StatusIcon size={16} className={status.iconClass} aria-hidden="true" />}
      </span>
      <div className="min-w-0 flex-1">
        {editing ? (
          <div>
            <textarea ref={editorRef} aria-label={t("编辑整理结果")} value={draft} disabled={busy} onChange={(event) => setDraft(event.target.value)} rows={3} className={`w-full rounded-lg border border-border bg-elevated px-3 py-2 text-sm text-primary outline-none focus:border-accent ${focusRingClass}`} />
            <p className="mt-1 text-xs text-secondary">{t("编辑中，保存后生效；Esc 可取消。")}</p>
          </div>
        ) : <p className="line-clamp-2 whitespace-pre-wrap break-words text-sm leading-5 text-primary">{item.final_text || item.raw_text || t("识别失败")}</p>}
        <p className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-tertiary"><span>{relativeTime(item.created_at, t, language)} · {formatDuration(item.duration, language)} {t("秒")}</span>{item.engine && <><span aria-hidden="true">·</span><span>{item.engine}</span></>}{cleanupLabel && <><span aria-hidden="true">·</span><span>{cleanupLabel}</span></>}{(item.revision_count ?? 0) > 0 && <><span aria-hidden="true">·</span><span>{item.revision_count} {t("个版本")}</span></>}</p>
        {showRaw && (item.asr_text || item.provider_cleaned_candidate || item.cleanup_rejected_candidate || item.raw_text) && <div className="mt-2 space-y-1 rounded-lg bg-elevated px-3 py-2 text-xs">{item.asr_text != null && <><p className="text-tertiary">{t("ASR 识别原文")}</p><p className="whitespace-pre-wrap text-secondary">{item.asr_text}</p></>}{item.provider_cleaned_candidate != null && <><p className="pt-1 text-tertiary">{t("服务商整理候选")}</p><p className="whitespace-pre-wrap text-secondary">{item.provider_cleaned_candidate}</p></>}{item.cleanup_rejected_candidate != null && <details className="pt-1 text-secondary"><summary className="cursor-pointer text-tertiary">{t(cleanupCandidateLabel(item.cleanup_guard_reason))}</summary><p className="mt-1 text-warning-ink">{t("保真检查原因：")}{t(cleanupGuardReasonLabel(item.cleanup_guard_reason))}</p><p className="mt-1 whitespace-pre-wrap break-words">{item.cleanup_rejected_candidate}</p></details>}{item.raw_text && <><p className="pt-1 text-tertiary">{t("清理前")}</p><p className="whitespace-pre-wrap text-secondary">{item.raw_text}</p></>}{item.final_text || item.raw_text ? <><p className="pt-1 text-tertiary">{t("清理后")}</p><p className="text-tertiary">{t("当前版本")}</p><p className="whitespace-pre-wrap text-primary">{item.final_text || item.raw_text}</p></> : null}</div>}
        {showRaw && (item.cleanup_attempt_kind != null || item.cleanup_failure_reason != null || item.cleanup_final_guard_reason != null || (item.cleanup_transformations != null && item.cleanup_transformations.length > 0)) && <div className="mt-2 space-y-1 rounded-lg bg-elevated px-3 py-2 text-xs break-words"><p className="font-medium text-secondary">{t("整理过程")}</p>{item.cleanup_attempt_kind != null && <p className="text-secondary">{t("请求状态：")}{cleanupFactLabel(item.cleanup_attempt_kind, cleanupAttemptLabels, t, "整理状态未确认")}</p>}{item.cleanup_failure_reason != null && <p className="text-secondary">{t("整理原因：")}{cleanupFactLabel(item.cleanup_failure_reason, cleanupFailureLabels, t, "整理失败原因未确认")}</p>}{item.cleanup_final_guard_reason != null && <p className="text-secondary">{t("最终保真检查：")}{t(cleanupGuardReasonLabel(item.cleanup_final_guard_reason))}</p>}{item.cleanup_transformations != null && item.cleanup_transformations.length > 0 && <p className="text-secondary">{t("响应调整：")}{item.cleanup_transformations.map((code) => cleanupFactLabel(code, cleanupTransformationLabels, t, "其他响应调整")).join(language === "en" ? ", " : "、")}</p>}</div>}
        {showRevisions && revisions && <RevisionList rawText={item.raw_text} revisions={revisions} t={t} />}
        {reason && <p className={`mt-1 text-xs ${status.detailClass}`}>{reason}</p>}
        {dictionaryCandidates.length > 0 && !editing && <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs"><span className="text-tertiary">{t("可能的词典建议")}</span>{dictionaryCandidates.map((candidate) => <button key={candidate.pair_key} type="button" className={compactButtonClass} disabled={busy} onClick={() => void confirmDictionaryCandidate(candidate)}>{t("确认")} “{candidate.before_span} → {candidate.after}”</button>)}</div>}
      </div>
      <div ref={actionsRef} className="vf-history-actions ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1">
        {editing ? (
          <>
            <button type="button" aria-label={t("保存历史版本")} className={compactButtonClass} disabled={busy || !draft.trim()} onClick={() => void saveManualRevision()}><Save size={15} aria-hidden="true" />{t(busy ? "保存中…" : "保存版本")}</button>
            <button type="button" aria-label={t("取消编辑")} disabled={busy} className={ghostButtonClass} onClick={cancelEditing}>{t("取消")}</button>
          </>
        ) : <IconButton data-history-edit label={t("编辑")} aria-label={t("编辑这条历史记录")} icon={<Pencil size={15} aria-hidden="true" />} disabled={busy || !(item.final_text || item.raw_text)} onClick={() => { setConfirmingDelete(false); setSaveNotice(null); setDraft(item.final_text || item.raw_text); setEditing(true); }} />}
        <button type="button" aria-label={t("复制这条历史记录到剪贴板")} className={`${ghostButtonClass} w-24 whitespace-nowrap`} disabled={busy || editing} onClick={() => void (async () => {
          setCopied(false);
          if (copyTimer.current) clearTimeout(copyTimer.current);
          if (await run("repaste_history")) {
            setCopied(true);
            if (copyTimer.current) clearTimeout(copyTimer.current);
            copyTimer.current = setTimeout(() => setCopied(false), 2000);
          }
        })()}>{copied ? <Check size={16} aria-hidden="true" /> : <Copy size={16} aria-hidden="true" />}{t(copied ? "已复制" : "复制")}</button>
        <span className="inline-flex h-9 w-9 shrink-0" aria-hidden={!canRetry || undefined}>{canRetry && <IconButton label={t("重试")} aria-label={t("重试这条历史记录")} icon={<RotateCcw size={15} aria-hidden="true" />} disabled={busy || editing} onClick={() => void run("retry_dictation", {}, true)} />}</span>
        <ActionMenu label={t("更多操作")} disabled={busy || editing} items={[
          ...((hasOriginalDetails || hasCleanupFacts) ? [{ label: t(showRaw ? (hasOriginalDetails ? "隐藏原文" : "隐藏详情") : (hasOriginalDetails ? "查看原文" : "查看详情")), onSelect: () => setShowRaw((value) => !value) }] : []),
          ...(item.revision_count ? [{ label: t(showRevisions ? "隐藏版本" : "查看版本"), onSelect: () => void toggleRevisions() }] : []),
          { label: t("重新整理"), icon: <Wand2 size={16} aria-hidden="true" />, disabled: !item.raw_text, onSelect: () => setShowReclean(true) },
          { label: t("删除记录"), icon: <Trash2 size={16} aria-hidden="true" />, danger: true, onSelect: () => {
            deleteTriggerRef.current = actionsRef.current?.querySelector<HTMLButtonElement>("[aria-haspopup='menu']") ?? null;
            setConfirmingDelete(true);
          } },
        ]} />
      </div>
      {confirmingDelete && <div className="flex basis-full flex-wrap items-center justify-end gap-2">
        <button type="button" aria-label={t("确认删除这条历史记录")} className={dangerActionButtonClass} disabled={busy || editing} onClick={() => { onDeleteStart?.(item.id); setConfirmingDelete(false); void run("delete_history", {}, true); }}><Trash2 size={15} aria-hidden="true" />{t("确认删除")}</button>
        <button ref={cancelDeleteRef} type="button" aria-label={t("取消删除")} className={ghostButtonClass} disabled={busy} onClick={cancelDelete}>{t("取消")}</button>
      </div>}
      {showReclean && <div className="flex basis-full flex-wrap items-center gap-2 rounded-lg bg-elevated p-3">
        <Select aria-label={t("重新整理模式")} value={operation} disabled={busy || editing} onValueChange={(value) => setOperation(value)} className={`${inputClass} w-40`}>
          <option value="cleanup">{t("忠实整理")}</option><option value="rewrite">{t("改写")}</option><option value="shorten">{t("缩短")}</option><option value="formalize">{t("正式一点")}</option><option value="casualize">{t("口语一点")}</option>
        </Select>
        <button type="button" aria-label={t("从原文重新整理并复制")} aria-busy={busy} disabled={busy || editing || !item.raw_text} onClick={() => void run("reclean_history", { operation }, true)} className={secondaryButtonClass}>{t(busy ? "处理中…" : "重新整理并复制")}</button>
        <button type="button" disabled={busy} onClick={() => setShowReclean(false)} className={ghostButtonClass}>{t("收起")}</button>
      </div>}
      {saveNotice && <p role="status" className="basis-full text-xs text-secondary">{saveNotice}</p>}
      {error && <p role="alert" className="basis-full text-xs text-error-ink">{error}</p>}
      <span role="status" className="sr-only">{copied ? t("已复制") : ""}</span>
    </div>
  );
}

function RevisionList({ rawText, revisions, t }: { rawText: string; revisions: HistoryRevision[]; t: (source: string) => string }) {
  return (
    <div className="mt-2 space-y-2 rounded-lg bg-elevated px-3 py-2 text-xs">
      <p className="text-tertiary">{t("版本差异")}</p>
      {revisions.map((revision, index) => {
        const previous = index === 0 ? rawText : revisions[index - 1].final_text;
        return (
          <div key={revision.revision_id} className="space-y-1 border-t border-border pt-2 first:border-t-0 first:pt-0">
            <p className="text-tertiary">{t("版本")} {index + 1} · {revisionReasonLabel(revision.revision_reason, t)}</p>
            <p className="text-tertiary">{t("修改前")}</p>
            <p className="whitespace-pre-wrap text-secondary">{previous}</p>
            <p className="pt-1 text-tertiary">{t("修改后")}</p>
            <p className="whitespace-pre-wrap text-primary">{revision.final_text}</p>
          </div>
        );
      })}
    </div>
  );
}

const degradedReasonLabels: Record<string, string> = {
  critical_memory_pressure: "系统内存压力过高，未运行本地整理；已保留 ASR 原稿",
  local_memory_unavailable: "本机内存预算不足，未运行本地整理；已保留 ASR 原稿",
  asr_failed: "语音识别失败，没有生成可用文字，可重试",
  crash_recovery: "上次录音未完成，已保留音频，可重试",
  interrupted_recording: "录音中断，未完成识别",
  llm_cleanup_failed: "文字整理失败，原始转录已保留，可重试",
  native_input_too_large: "本机整理输入超出支持上限，原始转录已保留，可缩短后重试",
  llm_cleanup_empty: "文字整理没有返回结果，原始转录已保留，可重试",
  partial_asr_failure: "部分语音未识别，已保留已识别内容，可重试",
  partial_llm_cleanup_failure: "部分内容未完成整理，原始转录已保留，可重试",
};

function statusPresentation(
  status: string,
  deliveryMethod: string | null | undefined,
  t: (source: string) => string,
) {
  if (status === "failed") {
    return { label: t("识别失败"), icon: CircleX, surfaceClass: "bg-error/10", iconClass: "text-error-ink", detailClass: "text-error-ink" };
  }
  if (status !== "degraded") {
    if (deliveryMethod === "history") status = "history";
    else if (deliveryMethod === "clipboard") status = "copied";
    else if (deliveryMethod === "paste_unverified") status = "unverified";
  }
  if (status === "degraded") {
    return { label: t("已保留原文"), icon: TriangleAlert, surfaceClass: "bg-warning/10", iconClass: "text-warning-ink", detailClass: "text-warning-ink" };
  }
  if (status === "unverified") {
    return { label: t("交付未确认"), icon: TriangleAlert, surfaceClass: "bg-warning/10", iconClass: "text-warning-ink", detailClass: "text-warning-ink" };
  }
  if (status === "copied") {
    return { label: t("已复制"), icon: CheckCircle2, surfaceClass: "bg-success/10", iconClass: "text-success-ink", detailClass: "text-secondary" };
  }
  if (status === "history") {
    return { label: t("已保存到历史记录"), icon: HistoryIcon, surfaceClass: "bg-elevated", iconClass: "text-secondary", detailClass: "text-secondary" };
  }
  if (status === "ok") {
    return { label: t("已完成"), icon: CheckCircle2, surfaceClass: "bg-success/10", iconClass: "text-success-ink", detailClass: "text-secondary" };
  }
  return { label: t("状态未知"), icon: CircleHelp, surfaceClass: "bg-elevated", iconClass: "text-tertiary", detailClass: "text-secondary" };
}

function finalDeliveryMethod(item: HistoryItem): string | null | undefined {
  if (item.delivery_method) return item.delivery_method;
  if (item.status === "history") return "history";
  if (item.status === "copied") return "clipboard";
  if (item.status === "unverified") return "paste_unverified";
  return item.delivery_method;
}

const producerPasteUncertaintyReasonCodes = new Set<string>([
  "paste_unverified",
  "paste_shortcut_outcome_unknown",
  "paste_expected_splice_unavailable",
  "paste_target_unconfirmed_after_attempt",
  "paste_focus_unconfirmed_after_attempt",
  "paste_target_changed_after_attempt",
  "paste_focus_changed_after_attempt",
  "paste_readback_unavailable",
  "paste_readback_mismatch",
  "paste_mutation_uncertain",
  "keyboard_paste_failed",
  "clipboard_ownership_unverified",
]);

const causeOnlyDeliveryReasonCodes = new Set<string>([
  "browser_permission_required",
  "secure_input",
  "input_changed",
  "target_changed",
  "target_unavailable",
  "paste_failed",
  "clipboard_unavailable",
  "clipboard_write_failed",
]);

function displayReason(item: HistoryItem, t: (source: string) => string): string | null {
  // An uncertain paste attempt is the immediate delivery fact. Do not bury it
  // under a cleanup warning from an earlier processing stage.
  if (
    item.fallback_reason &&
    producerPasteUncertaintyReasonCodes.has(item.fallback_reason) &&
    deliveryReasonLabels[item.fallback_reason]
  ) {
    return t(deliveryReasonLabels[item.fallback_reason]);
  }
  if (
    item.fallback_reason &&
    causeOnlyDeliveryReasonCodes.has(item.fallback_reason) &&
    finalDeliveryMethod(item) === "clipboard" &&
    item.status !== "failed" &&
    item.status !== "unverified"
  ) {
    return `${t(deliveryReasonLabels[item.fallback_reason])}；${t("已复制到剪贴板，请手动粘贴")}`;
  }
  // Prefer the bounded reason code over legacy prose, which may have promised
  // a clipboard copy before the delivery owner proved where the text went.
  if (item.fallback_reason && deliveryReasonLabels[item.fallback_reason]) {
    const messages = [
      item.degraded_reason ? degradedReasonLabels[item.degraded_reason] : undefined,
      deliveryReasonLabels[item.fallback_reason],
    ].filter(
      (message, index, all): message is string => Boolean(message) && all.indexOf(message) === index,
    );
    return messages.map((message) => t(message)).join(" · ");
  }
  const method = finalDeliveryMethod(item);
  if (method && method !== "clipboard") {
    if (item.degraded_reason && degradedReasonLabels[item.degraded_reason]) {
      return t(degradedReasonLabels[item.degraded_reason]);
    }
    if (item.fallback_reason) return t("处理失败，请检查结果或重试");
    return null;
  }
  if (item.delivery_user_reason) return t(item.delivery_user_reason);
  const messages = [
    item.degraded_reason ? degradedReasonLabels[item.degraded_reason] : undefined,
    item.fallback_reason ? deliveryReasonLabels[item.fallback_reason] : undefined,
  ].filter((message, index, all): message is string => Boolean(message) && all.indexOf(message) === index);
  if (messages.length) return messages.map((message) => t(message)).join(" · ");
  if (item.fallback_reason && !deliveryReasonLabels[item.fallback_reason]) {
    return t("处理失败，请检查结果或重试");
  }
  if (item.status === "failed") return t("处理失败，没有生成可用文字，可重试");
  if (item.status === "unverified") {
    return t("自动粘贴未确认，请先检查输入框；文字仍在剪贴板或历史记录中");
  }
  if (item.status === "degraded" || item.degraded) return t("处理未完成，原始转录已保留，可重试");
  return null;
}

function cleanupStatusLabel(status: string | null | undefined, t: (source: string) => string): string | null {
  if (status === "ai_success") return t("AI 已整理");
  if (status === "ai_failed_local") return t("AI 失败，已本地整理");
  if (status === "ai_failed_raw") return t("AI 失败，已保留原文");
  if (status === "critical_memory_pressure") return t("内存压力过高，已保留 ASR 原稿");
  if (status === "local_memory_unavailable") return t("本机内存预算不足，已保留 ASR 原稿");
  if (status === "preservation_guard") return t("已阻止可能改变原意的整理");
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
    <div role="status" aria-label={t("正在加载历史记录…")} aria-busy="true" className="mt-6 space-y-3">
      <span className="sr-only">{t("正在加载历史记录…")}</span>
      {[0, 1, 2].map((index) => (
        <div key={index} aria-hidden="true" className="flex items-center gap-3 border-t border-border py-4 first:border-t-0">
          <div className="h-7 w-7 shrink-0 animate-pulse rounded-full bg-elevated motion-reduce:animate-none" />
          <div className="min-w-0 flex-1 space-y-2">
            <div className="h-4 w-3/5 animate-pulse rounded bg-elevated motion-reduce:animate-none" />
            <div className="h-3 w-2/5 animate-pulse rounded bg-elevated motion-reduce:animate-none" />
          </div>
        </div>
      ))}
    </div>
  );
}

function parseUtcTimestamp(value: string): Date | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  const hasZone = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(trimmed);
  const normalized = trimmed.includes("T") ? trimmed : trimmed.replace(" ", "T");
  const dated = new Date(hasZone ? normalized : `${normalized}Z`);
  return Number.isNaN(dated.getTime()) ? null : dated;
}

function historyDay(value: string, t: (source: string) => string, language: string) {
  const date = parseUtcTimestamp(value);
  if (!date) return { key: value, label: value || t("日期未知") };
  const key = `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  const dayKey = (day: Date) => `${day.getFullYear()}-${day.getMonth()}-${day.getDate()}`;
  const label = key === dayKey(today) ? t("今天") : key === dayKey(yesterday) ? t("昨天") : new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en", { year: "numeric", month: "long", day: "numeric" }).format(date);
  return { key, label };
}

function relativeTime(value: string, t: (source: string) => string, language: string) {
  const dated = parseUtcTimestamp(value);
  if (!dated) return value;
  const seconds = Math.max(0, (Date.now() - dated.getTime()) / 1000);
  if (seconds < 60) return t("刚刚");
  const locale = language === "zh" ? "zh-CN" : "en";
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "always" });
  const unit: Intl.RelativeTimeFormatUnit = seconds < 3600 ? "minute" : seconds < 86400 ? "hour" : "day";
  const amount = -Math.floor(seconds / (unit === "minute" ? 60 : unit === "hour" ? 3600 : 86400));
  if (language !== "zh") return rtf.format(amount, unit);
  return rtf.formatToParts(amount, unit).map((part) => (part.type === "integer" ? `${part.value} ` : part.value)).join("");
}

function formatDuration(duration: number, language: string) {
  return new Intl.NumberFormat(language === "zh" ? "zh-CN" : "en", {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(duration);
}

function revisionReasonLabel(reason: string, t: (source: string) => string) {
  if (reason === "manual_edit") return t("手动编辑");
  if (reason === "ai_reclean") return t("AI 重新整理");
  return t(reason);
}

function Empty() {
  const { t } = useI18n();
  return (
    <div className="flex min-h-[260px] flex-col items-center justify-center text-center">
      <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-xl bg-elevated text-tertiary"><HistoryIcon size={19} aria-hidden="true" /></div>
      <p className="text-base font-medium text-primary">{t("还没有记录，按热键说一句吧")}</p>
      <p className="mt-2 text-sm text-tertiary">{t("你的语音记录会出现在这里。")}</p>
    </div>
  );
}
