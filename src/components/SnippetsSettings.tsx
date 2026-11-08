import { useEffect, useRef, useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { colors, radius, buttonClass, secondaryButtonClass, compactButtonClass, focusRingClass } from "../lib/theme";
import { useI18n } from "../lib/i18n";
import { useSessionDraftState } from "../lib/sessionDraft";
import { SettingsGroup, SettingsPageHeader, SettingsShell } from "./SettingsLayout";
import { ConfirmDialog } from "./ConfirmDialog";
import { IconButton } from "./IconButton";
import { Toggle } from "./Toggle";

export type Snippet = { id: string; trigger: string; expansion: string; enabled: boolean };

function newId() {
  return `snippet.${Date.now()}.${Math.random().toString(36).slice(2, 8)}`;
}

export function SnippetsSettings({ snippets, saving = false, saveError = null, saveRevision = 0, onDismissSaveError, onChange }: { snippets: Snippet[]; saving?: boolean; saveError?: string | null; saveRevision?: number; onDismissSaveError?: () => void; onChange: (snippets: Snippet[]) => void | Promise<void> }) {
  const { t } = useI18n();
  const [trigger, setTrigger] = useSessionDraftState("snippets:trigger", "");
  const [expansion, setExpansion] = useSessionDraftState("snippets:expansion", "");
  const [editingId, setEditingId] = useSessionDraftState<string | null>("snippets:editingId", null);
  const [localError, setError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [localBusy, setBusy] = useState(false);
  const busy = localBusy || saving;
  const error = localError ?? saveError;
  const busyRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const editOrigin = useRef<HTMLButtonElement | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [processedSaveRevision, setProcessedSaveRevision] = useSessionDraftState("snippets:processedSaveRevision", saveRevision);

  useEffect(() => {
    if (processedSaveRevision === saveRevision) return;
    setProcessedSaveRevision(saveRevision);
    if (snippets.some((item) => (!editingId || item.id === editingId)
      && item.trigger === trigger.trim().replace(/\s+/g, " ") && item.expansion === expansion.trim())) {
      setTrigger(""); setExpansion(""); setEditingId(null); setError(null);
    }
  }, [saveRevision, processedSaveRevision, setProcessedSaveRevision, snippets, editingId, trigger, expansion, setTrigger, setExpansion, setEditingId]);

  const resetEditor = () => { setTrigger(""); setExpansion(""); setEditingId(null); setError(null); onDismissSaveError?.(); };
  const persist = async (next: Snippet[], onSuccess?: () => void) => {
    if (busyRef.current || saving) return;
    busyRef.current = true; setBusy(true); setError(null);
    try { await onChange(next); onSuccess?.(); }
    catch { setError(t("片段未能保存，输入已保留，请重试。")); }
    finally { busyRef.current = false; setBusy(false); }
  };
  const save = () => {
    const nextTrigger = trigger.trim().replace(/\s+/g, " ");
    const nextExpansion = expansion.trim();
    if (!nextTrigger || !nextExpansion) { setError(t("请填写触发短语和展开内容")); return; }
    if (editingId && !snippets.some((item) => item.id === editingId)) { setError(t("这条片段已被删除，请取消编辑后重新添加。")); return; }
    if (snippets.some((item) => item.id !== editingId && item.trigger.trim().replace(/\s+/g, " ").toLowerCase() === nextTrigger.toLowerCase())) {
      setError(t("这个触发短语已经存在")); return;
    }
    const next = editingId
      ? snippets.map((item) => item.id === editingId ? { ...item, trigger: nextTrigger, expansion: nextExpansion } : item)
      : [...snippets, { id: newId(), trigger: nextTrigger, expansion: nextExpansion, enabled: true }];
    void persist(next, () => { resetEditor(); editOrigin.current?.focus({ preventScroll: true }); });
  };

  return <SettingsShell>
    <SettingsPageHeader title={t("语音片段")} description={t("说出完整触发短语，插入在这台 Mac 上保存的文字。片段内容不会上传。")} />
    <SettingsGroup title={t(editingId ? "编辑片段" : "添加片段")} description={t("添加或保存后生效；未保存的输入在本窗口切页时保留。") }>
      <form onSubmit={(event) => { event.preventDefault(); save(); }} className="px-5 py-4">
        <fieldset disabled={busy} className="vf-snippet-fields grid min-w-0 gap-4">
          <label className="min-w-0">
            <span className="text-sm font-medium text-primary">{t("触发短语")}</span>
            <span className="mt-1 block text-xs leading-5 text-tertiary">{t("例如：插入我的邮箱")}</span>
            <input ref={inputRef} data-delete-focus-fallback aria-label={t("触发短语")} aria-describedby={error ? "snippet-add-error" : undefined} value={trigger} onChange={(event) => setTrigger(event.target.value)} placeholder={t("说出的完整短语…")} className={`mt-2 w-full ${radius.control} h-10 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 text-sm outline-none ${focusRingClass}`} />
          </label>
          <label className="min-w-0">
            <span className="text-sm font-medium text-primary">{t("展开内容")}</span>
            <span className="mt-1 block text-xs leading-5 text-tertiary">{t("支持纯文本或本地模板")}</span>
            <textarea aria-label={t("展开内容")} aria-describedby={error ? "snippet-add-error" : undefined} value={expansion} onChange={(event) => setExpansion(event.target.value)} rows={3} placeholder={t("要插入的文字…")} className={`mt-2 min-h-24 w-full resize-y ${radius.control} border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none ${focusRingClass}`} />
          </label>
        </fieldset>
        <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
          {error && !pendingDeleteId ? <p id="snippet-add-error" role="alert" className="text-xs text-error-ink">{error}</p> : <p className="text-xs text-tertiary">{snippets.length >= 128 && !editingId ? t("已达到片段上限") : t("添加后，说出完整短语即可展开")}</p>}
          <div className="ml-auto flex gap-2">
            {(editingId || trigger || expansion) && <button type="button" disabled={busy} className={secondaryButtonClass} onClick={() => { resetEditor(); (editOrigin.current ?? inputRef.current)?.focus({ preventScroll: true }); }}>{t("取消")}</button>}
            <button type="submit" aria-busy={busy} disabled={busy || (!editingId && snippets.length >= 128)} className={buttonClass}>{t(busy ? "保存中…" : editingId ? "保存片段" : "添加片段")}</button>
          </div>
        </div>
      </form>
    </SettingsGroup>
    <SettingsGroup title={t("已保存片段")} description={`${snippets.length} ${t("条")}`}>
      {snippets.length === 0 ? <p className="px-5 py-6 text-sm text-secondary">{t("还没有语音片段。")}</p> : snippets.map((snippet) => <div key={snippet.id} data-focus-row className="flex flex-wrap items-start gap-4 px-5 py-4">
        <div className="min-w-0 flex-1">
          <p className="break-words text-sm font-medium leading-5 text-primary">{snippet.trigger}</p>
          <p id={`snippet-content-${snippet.id}`} className={`mt-1 whitespace-pre-wrap break-words text-[13px] leading-5 text-secondary ${expanded.has(snippet.id) ? "" : "line-clamp-2"}`}>{snippet.expansion}</p>
          {(snippet.expansion.length > 80 || snippet.expansion.split("\n").length > 2) && <button type="button" className={`mt-2 ${compactButtonClass}`} aria-expanded={expanded.has(snippet.id)} aria-controls={`snippet-content-${snippet.id}`} onClick={() => setExpanded((current) => { const next = new Set(current); if (next.has(snippet.id)) next.delete(snippet.id); else next.add(snippet.id); return next; })}>{t(expanded.has(snippet.id) ? "收起全文" : "展开全文")}</button>}
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <Toggle checked={snippet.enabled} disabled={busy} onChange={(enabled) => void persist(snippets.map((item) => item.id === snippet.id ? { ...item, enabled } : item))} label={`${t("启用")} ${snippet.trigger}`} />
          <IconButton size="sm" label={t("编辑片段")} aria-label={`${t("编辑片段")} ${snippet.trigger}`} disabled={busy} icon={<Pencil size={15} aria-hidden="true" />} onClick={(event) => {
            editOrigin.current = event.currentTarget; setEditingId(snippet.id); setTrigger(snippet.trigger); setExpansion(snippet.expansion); setError(null); inputRef.current?.focus();
          }} />
          <IconButton size="sm" label={t("删除片段")} aria-label={`${t("删除片段")} ${snippet.trigger}`} disabled={busy} tone="danger" onClick={() => { setError(null); setPendingDeleteId(snippet.id); }} icon={<Trash2 size={15} aria-hidden="true" />} />
        </div>
      </div>)}
    </SettingsGroup>
    <ConfirmDialog open={pendingDeleteId != null} title={t("删除片段")} description={t("确定删除“{name}”吗？").replace("{name}", snippets.find((item) => item.id === pendingDeleteId)?.trigger ?? "")} confirmLabel={t("删除片段")} cancelLabel={t("取消")} busy={busy} error={error} onCancel={() => { setPendingDeleteId(null); setError(null); }} onConfirm={() => void persist(snippets.filter((item) => item.id !== pendingDeleteId), () => { if (editingId === pendingDeleteId) resetEditor(); setPendingDeleteId(null); })} />
  </SettingsShell>;
}
