import { Select } from "../Select";
import { invoke } from "@tauri-apps/api/core";
import { Check, CheckCircle2, CircleHelp, CircleX, Copy, Download, Eraser, History as HistoryIcon, LoaderCircle, Pencil, RotateCcw, Save, Trash2, TriangleAlert, Wand2 } from "lucide-react";
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { mergeDictionary } from "../../lib/dictionaryImport";
import { secondaryButtonClass, compactButtonClass, dangerActionButtonClass, ghostButtonClass, inputClass, colors, radius, focusRingClass } from "../../lib/theme";
import { ActionMenu } from "../ActionMenu";
import { IconButton } from "../IconButton";
import { useI18n } from "../../lib/i18n";
import { deliveryReasonLabels } from "../../lib/deliveryCopy";
import { ConfirmDialog } from "../ConfirmDialog";
import { SettingsAlert, SettingsPageHeader, SettingsShell } from "../SettingsLayout";

export type HistoryItem = {
  id: number;
  created_at: string;
  raw_text: string;
  asr_text?: string | null;
  engine?: string | null;
  provider_cleaned_candidate?: string | null;
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

export function History({ items, reload, hasMore, loading, onLoadMore, error, onRetry, onQueryChange }: { items: HistoryItem[]; reload: () => void; hasMore: boolean; loading: boolean; onLoadMore: () => void; error?: string | null; onRetry?: () => void; onQueryChange?: (query: string) => void }) {
  const { t, language } = useI18n();
  const [query, setQuery] = useState("");
  const [dataError, setDataError] = useState<string | null>(null);
  const [dataMessage, setDataMessage] = useState<string | null>(null);
  const [pendingExport, setPendingExport] = useState<"records" | "audio" | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const queryInitializedRef = useRef(false);
  const normalizedQuery = query.trim().toLowerCase();
  const visible = useMemo(
    () => items.filter((item) => !normalizedQuery || `${item.asr_text ?? ""} ${item.provider_cleaned_candidate ?? ""} ${item.raw_text} ${item.final_text}`.toLowerCase().includes(normalizedQuery)),
    [items, normalizedQuery],
  );

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
    setConfirmClear(true);
  };

  const confirmClearAll = async () => {
    setConfirmClear(false);
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
      <SettingsPageHeader title={t("历史记录")} description={t("每一次表达，都留在这里。")} actions={
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" aria-label={t("导出记录")} aria-busy={pendingExport === "records"} disabled={pendingExport !== null} className={`${secondaryButtonClass} w-40 whitespace-nowrap`} onClick={() => void exportData()}><Download size={15} className="shrink-0" aria-hidden="true" />{t(pendingExport === "records" ? "导出中…" : "导出记录")}</button>
          <button type="button" aria-label={t("导出音频")} aria-busy={pendingExport === "audio"} disabled={pendingExport !== null} className={`${secondaryButtonClass} w-40 whitespace-nowrap`} onClick={() => void exportGold()}><Download size={15} className="shrink-0" aria-hidden="true" />{t(pendingExport === "audio" ? "导出中…" : "导出音频")}</button>
          <IconButton label={t("清空")} aria-label={t("清空全部数据")} tone="danger" icon={<Eraser size={16} aria-hidden="true" />} disabled={loading || pendingExport !== null} onClick={() => void clearAll()} />
        </div>
      } />
      {dataMessage && <p role="status" className="mt-3 break-words text-xs leading-5 text-success-ink">{dataMessage}</p>}
      {dataError && <p role="alert" className="mt-3 break-words text-xs leading-5 text-error-ink">{dataError}</p>}
      <div className="mt-6 flex flex-wrap items-center justify-between gap-3 border-y border-border py-3">
        <input aria-label={t("搜索历史记录")} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("搜索历史…")} className={`${inputClass} w-64 max-w-full`} />
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
                <HistoryRow item={item} reload={reload} windowed={visible.length > 50} />
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
        confirmLabel={t("清空全部数据")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void confirmClearAll()}
      />
    </SettingsShell>
  );
}

function HistoryRow({ item, reload, windowed = false }: { item: HistoryItem; reload: () => void; windowed?: boolean }) {
  const { t, language } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
  const status = statusPresentation(item.status, t);
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
      if (reloadAfter) reload();
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
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
      const settings = await invoke<{ dictionary: string[] }>("get_settings");
      const dictionary = settings.dictionary ?? [];
      const alreadyPresent = dictionary.some((word) => word === candidate.after);
      if (!alreadyPresent) {
        const result = mergeDictionary(dictionary, [candidate.after]);
        if (result.added === 0) {
          setError(t("词条无效或已达到上限。"));
          return;
        }
      }
      await invoke("promote_learn_pair", {
        pairKey: candidate.pair_key,
        beforeSurface: candidate.before_span,
        afterSurface: candidate.after,
        historyId: item.id,
      });
      setDictionaryCandidates((current) => current.filter((value) => value.pair_key !== candidate.pair_key));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`vf-history-row flex flex-wrap items-start gap-3 border-t border-border px-5 py-4 first:border-t-0${windowed ? " history-list--windowed" : ""}`} onKeyDown={(event) => {
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
        {showRaw && (item.asr_text || item.provider_cleaned_candidate || item.raw_text) && <div className="mt-2 space-y-1 rounded-lg bg-elevated px-3 py-2 text-xs">{item.asr_text != null && <><p className="text-tertiary">{t("ASR 识别原文")}</p><p className="whitespace-pre-wrap text-secondary">{item.asr_text}</p></>}{item.provider_cleaned_candidate != null && <><p className="pt-1 text-tertiary">{t("服务商整理候选")}</p><p className="whitespace-pre-wrap text-secondary">{item.provider_cleaned_candidate}</p></>}{item.raw_text && <><p className="pt-1 text-tertiary">{t("清理前")}</p><p className="whitespace-pre-wrap text-secondary">{item.raw_text}</p></>}<p className="pt-1 text-tertiary">{t("清理后")}</p><p className="text-tertiary">{t("当前版本")}</p><p className="whitespace-pre-wrap text-primary">{item.final_text || item.raw_text}</p></div>}
        {showRevisions && revisions && <RevisionList rawText={item.raw_text} revisions={revisions} t={t} />}
        {reason && <p className={`mt-1 text-xs ${status.detailClass}`}>{reason}</p>}
        {dictionaryCandidates.length > 0 && !editing && <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs"><span className="text-tertiary">{t("可能的词典建议")}</span>{dictionaryCandidates.map((candidate) => <button key={candidate.pair_key} type="button" className={compactButtonClass} disabled={busy} onClick={() => void confirmDictionaryCandidate(candidate)}>{t("确认")} “{candidate.before_span} → {candidate.after}”</button>)}</div>}
      </div>
      <div ref={actionsRef} className="vf-history-actions ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1">
        {editing ? (
          <>
            <button type="button" aria-label={t("保存历史版本")} className={compactButtonClass} disabled={busy || !draft.trim()} onClick={() => void (async () => { if (await run("save_history_revision", { final_text: draft, revision_reason: "manual_edit" }, true)) { setEditing(false); await suggestDictionaryCandidates(item.raw_text, draft); } })()}><Save size={15} aria-hidden="true" />{t(busy ? "保存中…" : "保存版本")}</button>
            <button type="button" aria-label={t("取消编辑")} disabled={busy} className={ghostButtonClass} onClick={cancelEditing}>{t("取消")}</button>
          </>
        ) : <IconButton data-history-edit label={t("编辑")} aria-label={t("编辑这条历史记录")} icon={<Pencil size={15} aria-hidden="true" />} disabled={busy || !(item.final_text || item.raw_text)} onClick={() => { setConfirmingDelete(false); setDraft(item.final_text || item.raw_text); setEditing(true); }} />}
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
          ...((item.asr_text || item.provider_cleaned_candidate || (item.raw_text && item.final_text)) ? [{ label: t(showRaw ? "隐藏原文" : "查看原文"), onSelect: () => setShowRaw((value) => !value) }] : []),
          ...(item.revision_count ? [{ label: t(showRevisions ? "隐藏版本" : "查看版本"), onSelect: () => void toggleRevisions() }] : []),
          { label: t("重新整理"), icon: <Wand2 size={16} aria-hidden="true" />, disabled: !item.raw_text, onSelect: () => setShowReclean(true) },
          { label: t("删除记录"), icon: <Trash2 size={16} aria-hidden="true" />, danger: true, onSelect: () => {
            deleteTriggerRef.current = actionsRef.current?.querySelector<HTMLButtonElement>("[aria-haspopup='menu']") ?? null;
            setConfirmingDelete(true);
          } },
        ]} />
      </div>
      {confirmingDelete && <div className="flex basis-full flex-wrap items-center justify-end gap-2">
        <button type="button" aria-label={t("确认删除这条历史记录")} className={dangerActionButtonClass} disabled={busy || editing} onClick={() => { setConfirmingDelete(false); void run("delete_history", {}, true); }}><Trash2 size={15} aria-hidden="true" />{t("确认删除")}</button>
        <button ref={cancelDeleteRef} type="button" aria-label={t("取消删除")} className={ghostButtonClass} disabled={busy} onClick={cancelDelete}>{t("取消")}</button>
      </div>}
      {showReclean && <div className="flex basis-full flex-wrap items-center gap-2 rounded-lg bg-elevated p-3">
        <Select aria-label={t("重新整理模式")} value={operation} disabled={busy || editing} onValueChange={(value) => setOperation(value)} className={`${inputClass} w-40`}>
          <option value="cleanup">{t("忠实整理")}</option><option value="rewrite">{t("改写")}</option><option value="shorten">{t("缩短")}</option><option value="formalize">{t("正式一点")}</option><option value="casualize">{t("口语一点")}</option>
        </Select>
        <button type="button" aria-label={t("从原文重新整理")} aria-busy={busy} disabled={busy || editing || !item.raw_text} onClick={() => void run("reclean_history", { operation }, true)} className={secondaryButtonClass}>{t(busy ? "处理中…" : "重新整理")}</button>
        <button type="button" disabled={busy} onClick={() => setShowReclean(false)} className={ghostButtonClass}>{t("收起")}</button>
      </div>}
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
  asr_failed: "语音识别失败，没有生成可用文字，可重试",
  crash_recovery: "上次录音未完成，已保留音频，可重试",
  interrupted_recording: "录音中断，未完成识别",
  llm_cleanup_failed: "AI 整理失败，已插入原文",
  llm_cleanup_empty: "AI 整理没有返回结果，已插入原文",
  partial_asr_failure: "部分语音未识别，已保留已识别内容，可重试",
  partial_llm_cleanup_failure: "部分内容未完成整理，原始转录已保留，可重试",
};

function statusPresentation(status: string, t: (source: string) => string) {
  if (status === "failed") {
    return { label: t("识别失败"), icon: CircleX, surfaceClass: "bg-error/10", iconClass: "text-error-ink", detailClass: "text-error-ink" };
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

function displayReason(item: HistoryItem, t: (source: string) => string): string | null {
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
