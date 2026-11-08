import { Select } from "./Select";
import { useSessionDraftState } from "../lib/sessionDraft";
import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FileText, Upload } from "lucide-react";
import { ConfirmDialog } from "./ConfirmDialog";
import { DictionaryLesson } from "./Onboarding/ContextualLessons";
import { DictionaryLearningAudit } from "./DictionaryLearningAudit";
import { SettingsAlert, SettingsDisclosure, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "./SettingsLayout";
import { MAX_DICTIONARY_FILE_BYTES, mergeDictionary, parseDictionaryText } from "../lib/dictionaryImport";
import { useI18n } from "../lib/i18n";
import { friendlyDictionaryLearningError } from "../lib/settingsError";
import { buttonClass, compactButtonClass, inputClass } from "../lib/theme";
import { Toggle } from "./Toggle";
import type { SaveSettings, Settings } from "../types/settings";

const controlClass = inputClass;
const DICTIONARY_PARTIAL_FAILURE_PREFIX = "learning_dictionary_partial:";

function isDictionaryPartialFailure(reason: unknown): boolean {
  let message = "";
  if (typeof reason === "string") message = reason;
  else if (reason instanceof Error) message = reason.message;
  else if (reason && typeof reason === "object" && "message" in reason && typeof reason.message === "string") {
    message = reason.message;
  }
  return message.startsWith(DICTIONARY_PARTIAL_FAILURE_PREFIX);
}

type LearnPair = {
  pair_key: string;
  before_surface: string;
  after_surface: string;
  hits: number;
  promoted: boolean;
  ignored?: boolean;
  promote_hits?: number;
  pinned?: boolean;
  last_at?: string;
  manual_scoped?: boolean;
  native_bundle?: string | null;
};

type ApplicationOption = { bundle_id: string; label: string };

type StyleDraft = {
  draft_key: string;
  mapping_id: string;
  style_key: string;
  excerpt: string;
  before_excerpt: string;
  after_excerpt: string;
};

type LearnedTermUsage = { word: string; replacement_runs: number; last_replaced_at: string };

export function DictionarySettings({ settings, save, onOpenHistory, onOpenPreferences }: { settings: Settings; save: SaveSettings; onOpenHistory?: () => void; onOpenPreferences?: () => void }) {
  const { t } = useI18n();
  const [newWord, setNewWord] = useSessionDraftState("dictionary:newWord", "");
  const newWordInput = useRef<HTMLInputElement>(null);
  const [addingWord, setAddingWord] = useState(false);
  const addingWordRef = useRef(false);
  const [dictionaryFailed, setDictionaryFailed] = useState(false);
  const [dictionaryMessage, setDictionaryMessage] = useState<string | null>(null);
  const [dictionaryDragging, setDictionaryDragging] = useState(false);
  const [dictionaryImporting, setDictionaryImporting] = useState(false);
  const [pendingDeleteWord, setPendingDeleteWord] = useState<string | null>(null);
  const [pendingPairs, setPendingPairs] = useState<LearnPair[]>([]);
  const [promotedPairs, setPromotedPairs] = useState<LearnPair[]>([]);
  const [manualApplications, setManualApplications] = useState<ApplicationOption[] | null>(null);
  const [manualApplicationsFailed, setManualApplicationsFailed] = useState(false);
  const [manualBefore, setManualBefore] = useSessionDraftState("dictionary:manualBefore", "");
  const [manualAfter, setManualAfter] = useSessionDraftState("dictionary:manualAfter", "");
  const [manualBundle, setManualBundle] = useSessionDraftState("dictionary:manualBundle", "");
  const [termUsage, setTermUsage] = useState<LearnedTermUsage[] | null>(null);
  const [styleDrafts, setStyleDrafts] = useState<StyleDraft[]>([]);
  const [pinnedTerms, setPinnedTerms] = useState<string[]>([]);
  const [pendingStyleDraft, setPendingStyleDraft] = useState<StyleDraft | null>(null);
  const [learningError, setLearningError] = useState<string | null>(null);
  const [learningActionError, setLearningActionError] = useState<string | null>(null);
  const [learningLoaded, setLearningLoaded] = useState(false);
  const [learningLoading, setLearningLoading] = useState(false);
  const [learningBusy, setLearningBusy] = useState(false);
  const learningRequestRef = useRef(0);
  const learningBusyRef = useRef(false);
  const failedLearningActionRef = useRef<{ command: string; args: Record<string, unknown> } | null>(null);
  const dictionaryFileInput = useRef<HTMLInputElement | null>(null);
  const dictionaryDropZone = useRef<HTMLDivElement | null>(null);
  const importingRef = useRef(false);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const loadPendingPairs = useCallback(async () => {
    const request = ++learningRequestRef.current;
    setLearningLoading(true);
    const [pairs, drafts, pinned, usage] = await Promise.allSettled([
      invoke<LearnPair[]>("list_learn_pairs"),
      invoke<StyleDraft[]>("list_style_drafts"),
      invoke<string[]>("list_pinned_terms"),
      invoke<LearnedTermUsage[]>("list_learned_term_usage"),
    ]);
    if (request !== learningRequestRef.current) return;
    if (pairs.status === "fulfilled") {
      const rows = pairs.value;
      const live = Array.isArray(rows) ? rows.filter((row) => !row.ignored) : [];
      setPendingPairs(live.filter((row) => !row.promoted));
      setPromotedPairs(live.filter((row) => row.promoted && row.before_surface));
      setLearningLoaded(true);
    }
    if (drafts.status === "fulfilled") setStyleDrafts(Array.isArray(drafts.value) ? drafts.value : []);
    if (pinned.status === "fulfilled") setPinnedTerms(Array.isArray(pinned.value) ? pinned.value : []);
    if (usage.status === "fulfilled") setTermUsage(Array.isArray(usage.value) ? usage.value : []);
    setLearningError([pairs, drafts, pinned, usage].some((result) => result.status === "rejected") ? t("部分词典学习数据未能读取，已有内容已保留。请重试。") : null);
    setLearningLoading(false);
  }, [t]);

  const runLearningAction = async (command: string, args: Record<string, unknown>): Promise<boolean> => {
    if (learningBusyRef.current) return false;
    learningBusyRef.current = true;
    setLearningBusy(true);
    setLearningActionError(null);
    try {
      await invoke(command, args);
      await loadPendingPairs();
      failedLearningActionRef.current = null;
      return true;
    } catch (reason) {
      failedLearningActionRef.current = { command, args };
      const dictionaryError = friendlyDictionaryLearningError(reason, t);
      if (dictionaryError) {
        setLearningActionError(dictionaryError);
      } else {
        const message = command === "add_manual_scoped_learn_pair"
          ? reason === "manual_pair_scope_conflict"
            ? "同一纠正已在其他范围登记；请先在已生效替换中忘记旧规则，再添加。"
            : reason === "manual_pair_scope_unknown"
              ? "所选 App 已不在运行，或当前范围无法确认；请重新选择后重试。"
              : "专用纠正未能保存；现有规则没有被覆盖，请重试。"
          : "词典操作未完成，请重试。";
        setLearningActionError(t(message));
      }
      return false;
    } finally {
      learningBusyRef.current = false;
      setLearningBusy(false);
    }
  };

  useEffect(() => {
    void loadPendingPairs();
    let active = true;
    const subscription = listen("learn_pairs://changed", () => {
      if (active) void loadPendingPairs();
    });
    const drafts = listen("style_drafts://changed", () => {
      if (active) void loadPendingPairs();
    });
    const completed = listen<{ state: string }>("dictation://state", (event) => {
      if (active && ["done", "copied", "history", "unverified", "degraded"].includes(event.payload.state)) void loadPendingPairs();
    });
    return () => {
      active = false;
      learningRequestRef.current += 1;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
      void drafts.then((unlisten) => unlisten()).catch(() => undefined);
      void completed.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [loadPendingPairs]);

  const [manualApplicationsLoading, setManualApplicationsLoading] = useState(false);
  const manualApplicationsRequest = useRef(0);
  const refreshManualApplications = useCallback(async () => {
    const request = ++manualApplicationsRequest.current;
    setManualApplicationsLoading(true);
    try {
      const applications = await invoke<ApplicationOption[]>("get_manual_learn_applications");
      if (request !== manualApplicationsRequest.current) return;
      const next = Array.isArray(applications) ? applications : [];
      setManualApplications(next); setManualApplicationsFailed(false);
      setManualBundle((current) => next.some((application) => application.bundle_id === current) ? current : "");
    } catch {
      if (request !== manualApplicationsRequest.current) return;
      setManualApplicationsFailed(true);
    } finally {
      if (request === manualApplicationsRequest.current) setManualApplicationsLoading(false);
    }
  }, [setManualBundle]);
  useEffect(() => {
    void refreshManualApplications();
    const onFocus = () => void refreshManualApplications();
    window.addEventListener("focus", onFocus);
    return () => { manualApplicationsRequest.current++; window.removeEventListener("focus", onFocus); };
  }, [refreshManualApplications]);

  const addManualPair = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!manualBundle || !manualBefore.trim() || !manualAfter.trim() || learningBusyRef.current) return;
    const added = await runLearningAction("add_manual_scoped_learn_pair", {
      beforeSurface: manualBefore.trim(),
      afterSurface: manualAfter.trim(),
      bundleId: manualBundle,
    });
    if (added) {
      setManualBefore("");
      setManualAfter("");
    }
  };

  const importText = useCallback(async (text: string, fileName: string) => {
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryFailed(true);
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (importingRef.current) return;

    importingRef.current = true;
    setDictionaryImporting(true);
    setDictionaryFailed(false);
    setDictionaryMessage(null);
    try {
      const result = mergeDictionary(settingsRef.current.dictionary, parseDictionaryText(text, fileName));
      if (result.requested.length > 0) {
        await invoke("add_dictionary_entries", { words: result.requested });
      }
      const details = [
        result.requested.length > 0
          ? t("已登记个人词条：{count} 条").replace("{count}", String(result.requested.length))
          : t("没有找到可导入的新词条。"),
        result.added > 0 ? t("新增词条：{count} 条").replace("{count}", String(result.added)) : null,
        result.duplicates > 0 ? `${t("重复")} ${result.duplicates} ${t("条")}` : null,
        result.invalid > 0 ? `${t("无效")} ${result.invalid} ${t("条")}` : null,
        result.limited > 0 ? `${t("超出上限")} ${result.limited} ${t("条")}` : null,
      ].filter(Boolean).join(" · ");
      setDictionaryMessage(details);
    } catch (reason) {
      setDictionaryFailed(true);
      setDictionaryMessage(isDictionaryPartialFailure(reason)
        ? t("词条来源已记录，但个人词典未能同步。请重新导入以重试。")
        : t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    } finally {
      importingRef.current = false;
      setDictionaryImporting(false);
    }
  }, [t]);

  const importFile = useCallback(async (file: File) => {
    const extension = file.name.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryFailed(true);
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (file.size > MAX_DICTIONARY_FILE_BYTES) {
      setDictionaryFailed(true);
      setDictionaryMessage(t("词典文件不能超过 1 MB。"));
      return;
    }
    try {
      const decoder = new TextDecoder("utf-8", { fatal: true });
      await importText(decoder.decode(await file.arrayBuffer()), file.name);
    } catch (reason) {
      setDictionaryFailed(true);
      setDictionaryMessage(reason instanceof TypeError ? t("导入失败：词典文件必须使用 UTF-8 编码。") : t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    }
  }, [importText, t]);

  const importNativeFile = useCallback(async (path: string) => {
    const fileName = path.split(/[\\/]/).pop() || "dictionary.csv";
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryFailed(true);
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    try {
      const text = await invoke<string>("read_dictionary_file", { path });
      await importText(text, fileName);
    } catch (reason) {
      setDictionaryFailed(true);
      setDictionaryMessage(t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    }
  }, [importText, t]);

  const isInsideDropZone = useCallback((position: { x: number; y: number }) => {
    const element = dictionaryDropZone.current;
    if (!element) return false;
    const rect = element.getBoundingClientRect();
    const scale = window.devicePixelRatio || 1;
    const x = position.x / scale;
    const y = position.y / scale;
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (disposed) return;
      const { payload } = event;
      if (payload.type === "leave") {
        setDictionaryDragging(false);
        return;
      }
      const overDropZone = isInsideDropZone(payload.position);
      if (payload.type === "enter" || payload.type === "over") {
        setDictionaryDragging(overDropZone);
        return;
      }
      setDictionaryDragging(false);
      if (overDropZone && payload.paths[0]) void importNativeFile(payload.paths[0]);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [importNativeFile, isInsideDropZone]);

  const addWord = () => {
    if (addingWordRef.current) return;
    const word = newWord.trim();
    if (!word) return;
    const result = mergeDictionary(settings.dictionary, [word]);
    if (result.requested.length === 0) {
      setDictionaryFailed(true);
      setDictionaryMessage(result.duplicates > 0 ? t("这个词条已经存在。") : t("词条无效或已达到上限。"));
      return;
    }
    addingWordRef.current = true;
    setAddingWord(true);
    void invoke("add_dictionary_entries", { words: result.requested })
      .then(() => {
        setNewWord((current) => current.trim() === word ? "" : current);
        setDictionaryFailed(false);
        setDictionaryMessage(result.added === 0 ? t("已有词条已登记为手动词条。") : null);
      })
      .catch((reason) => {
        setDictionaryFailed(true);
        setDictionaryMessage(isDictionaryPartialFailure(reason)
          ? t("词条来源已记录，但个人词典未能同步。再次添加此词条可重试。")
          : t("词条添加失败：") + (reason instanceof Error ? reason.message : String(reason)));
      })
      .finally(() => { addingWordRef.current = false; setAddingWord(false); });
  };

  const learnedAfters = new Set(promotedPairs.map((pair) => pair.after_surface));
  const dictionaryWords = settings.dictionary;
  const liveUsage = termUsage?.filter((item) => learnedAfters.has(item.word));
  const recentPairs = [...promotedPairs].sort((a, b) => (b.last_at ?? "").localeCompare(a.last_at ?? "")).slice(0, 3);

  const openDictionaryFilePicker = () => dictionaryFileInput.current?.click();
  const handleDictionaryDrop = (event: React.DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDictionaryDragging(false);
    const file = event.dataTransfer.files[0];
    if (file) void importFile(file);
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("个人词典")} description={t("把人名、产品名和专业术语添加到这里，识别时会优先保留正确拼写。")} />
      {learningError && <div className="mt-4"><SettingsAlert onRetry={() => void loadPendingPairs()} retryLabel={t("重试")}>{learningError}</SettingsAlert></div>}
      {learningActionError && !pendingStyleDraft && !pendingDeleteWord && <div className="mt-4"><SettingsAlert onRetry={() => {
        const failed = failedLearningActionRef.current;
        if (failed) void runLearningAction(failed.command, failed.args);
      }} retryLabel={t("重试")}>{learningActionError}</SettingsAlert></div>}
      <SettingsGroup title={t("添加词条")}>
        <SettingsRow title={t("添加个人词典词条")} description={t("输入一个词条，或从 CSV、TXT、TSV 文件导入。")}>
          <div className="flex w-full min-w-0 gap-2 sm:w-auto">
            <input ref={newWordInput} data-delete-focus-fallback aria-label={t("添加个人词典词条")} value={newWord} onChange={(event) => setNewWord(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.nativeEvent.isComposing) { event.preventDefault(); addWord(); } }} placeholder={t("添加人名或术语…")} className={"min-w-0 flex-1 sm:w-48 " + controlClass} />
            <button type="button" onClick={addWord} aria-busy={addingWord} disabled={!newWord.trim() || addingWord} className={`${buttonClass} min-w-28`}>{t(addingWord ? "添加中…" : "添加")}</button>
          </div>
        </SettingsRow>
        <div
          ref={dictionaryDropZone}
          role="button"
          tabIndex={0}
          aria-label={t("导入个人词典文件")}
          onClick={openDictionaryFilePicker}
          onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); openDictionaryFilePicker(); } }}
          onDragEnter={(event) => { event.preventDefault(); setDictionaryDragging(true); }}
          onDragOver={(event) => event.preventDefault()}
          onDragLeave={(event) => { if (event.currentTarget === event.target) setDictionaryDragging(false); }}
          onDrop={handleDictionaryDrop}
          className={`mx-5 my-4 flex min-h-16 cursor-pointer items-center gap-3 rounded-lg border border-dashed px-3 py-2.5 transition-colors ${dictionaryDragging ? "border-accent bg-elevated" : "border-border bg-elevated/40 hover:border-accent/50 hover:bg-elevated/70"} ${dictionaryImporting ? "pointer-events-none opacity-60" : ""}`}
        >
          <Upload size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-primary">{t("拖入词典文件，或点击选择")}</span>
            <span className="mt-0.5 block text-xs text-tertiary">{t("支持 CSV、TXT、TSV；CSV 默认读取第一列，每行一个词条。")} </span>
          </span>
          <FileText size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <input ref={dictionaryFileInput} type="file" accept=".csv,.txt,.tsv,text/csv,text/plain,text/tab-separated-values" className="sr-only" onChange={(event) => { const file = event.target.files?.[0]; event.target.value = ""; if (file) void importFile(file); }} />
        </div>
        {dictionaryMessage && <p role={dictionaryFailed ? "alert" : "status"} className={`px-5 pb-4 text-xs leading-5 ${dictionaryFailed ? "text-error-ink" : "text-secondary"}`}>{dictionaryMessage}</p>}
      </SettingsGroup>
      <DictionaryLesson onAddTerm={() => newWordInput.current?.focus()} onOpenHistory={onOpenHistory} onOpenPreferences={onOpenPreferences} />
      <SettingsGroup title={t("词条列表")} description={`${dictionaryWords.length} ${t("条")}`}>
        {dictionaryWords.length > 0 ? dictionaryWords.map((word) => (
          <div key={word} data-focus-row className="flex flex-wrap items-center gap-3 px-5 py-4 text-sm">
            <span className="min-w-0 flex-1 break-words text-primary">{word}</span>
            <button type="button" disabled={learningBusy} aria-label={`${pinnedTerms.includes(word) ? t("取消置顶") : t("置顶")} ${word}`} onClick={() => void runLearningAction("pin_dictionary_term", { word, pinned: !pinnedTerms.includes(word) })} className={compactButtonClass}>{pinnedTerms.includes(word) ? t("已置顶") : t("置顶")}</button>
            <button type="button" disabled={learningBusy} aria-label={`${t("删除")} ${word}`} onClick={() => { setLearningActionError(null); setPendingDeleteWord(word); }} className={`${compactButtonClass} enabled:hover:bg-error/10 enabled:hover:text-error-ink`}>{t("删除")}</button>
          </div>
        )) : <p className="px-5 py-6 text-sm text-secondary">{t("当前没有个人词典词条。")}</p>}
      </SettingsGroup>
      <SettingsGroup title={t("词典学习")}>
        <SettingsRow title={t("学习词条")} description={t("普通词需 3 次、人名需 2 次不同听写中的纠正；同一听写重复保存只计一次。也可在历史或本页确认，继续打字不会学习。")}>
          <Toggle checked={settings.dictionary_learn_enabled !== false} onChange={(checked) => save({ dictionary_learn_enabled: checked })} label={t("学习词条")} />
        </SettingsRow>
        <SettingsRow title={t("本机观察插入后的改动")} description={t("仅在 VoiceFlow 已确认插入后，短暂读取同一输入框，并只学习唯一插入片段内的改动；按当前 App 范围学习，不读取其他屏幕文字，也不会发送这些改动。关闭后撤销这项独立授权；若该 App 已单独授权读取辅助功能文字，该权限仍可能允许观察。") }>
          <Toggle checked={settings.local_edit_learning_enabled === true} onChange={(checked) => save({ local_edit_learning_enabled: checked })} label={t("本机观察插入后的改动")} />
        </SettingsRow>
      </SettingsGroup>
      {pendingPairs.length > 0 && (
        <SettingsGroup title={t("待确认的纠正")}>
          {pendingPairs.map((pair) => (
            <div key={pair.pair_key} role="group" aria-label={`${pair.before_surface} → ${pair.after_surface}`} className="flex flex-wrap items-center gap-3 px-5 py-4 text-sm">
              <span className="min-w-0 flex-1 break-words text-primary">{pair.before_surface} <span className="text-secondary">→</span> <strong className="font-medium">{pair.after_surface}</strong><span className="mt-1 block text-xs tabular-nums text-secondary">{pair.hits} {t("次纠正记录")}</span></span>
              <button type="button" disabled={learningBusy} onClick={() => void runLearningAction("promote_learn_pair", { pairKey: pair.pair_key, beforeSurface: pair.before_surface, afterSurface: pair.after_surface })} className={compactButtonClass}>{t("确认")}</button>
              <button type="button" disabled={learningBusy} onClick={() => void runLearningAction("ignore_learn_pair", { pairKey: pair.pair_key })} className={`${compactButtonClass} enabled:hover:bg-error/10 enabled:hover:text-error-ink`}>{t("忽略")}</button>
            </div>
          ))}
        </SettingsGroup>
      )}
      <SettingsGroup title={t("手动添加 App 专用纠正")} description={t("明确选择一个正在运行的原生 App；规则只在所选 App 生效，不改全局个人词典，也不改历史记录。可随时忘记撤销。") }>
        <form onSubmit={(event) => void addManualPair(event)} className="grid gap-3 px-5 py-4 sm:grid-cols-2">
          <label className="grid gap-1.5 text-sm text-secondary">
            <span>{t("听写识别为")}</span>
            <input aria-label={t("听写识别为")} className={controlClass} value={manualBefore} onChange={(event) => setManualBefore(event.target.value)} maxLength={256} />
          </label>
          <label className="grid gap-1.5 text-sm text-secondary">
            <span>{t("应替换为")}</span>
            <input aria-label={t("应替换为")} className={controlClass} value={manualAfter} onChange={(event) => setManualAfter(event.target.value)} maxLength={256} />
          </label>
          <label className="grid gap-1.5 text-sm text-secondary sm:col-span-2">
            <span>{t("仅在此 App 生效")}</span>
            <Select aria-label={t("仅在此 App 生效")} className={controlClass} value={manualBundle} onValueChange={setManualBundle} disabled={manualApplicationsLoading || manualApplicationsFailed || manualApplications == null || manualApplications.length === 0}>
              <option value="">{t("选择一个正在运行的原生 App")}</option>
              {(manualApplications ?? []).map((application) => <option key={application.bundle_id} value={application.bundle_id}>{application.label}</option>)}
            </Select>
          </label>
          {manualApplicationsFailed ? <p role="alert" className="text-xs text-error-ink sm:col-span-2">{t("无法读取当前运行的 App；请重试后再添加。")}</p> : manualApplications?.length === 0 ? <p role="status" className="text-xs text-secondary sm:col-span-2">{t("目前没有可选的运行中原生 App。")}</p> : null}
          <div className="flex flex-wrap justify-between gap-2 sm:col-span-2">
            <button type="button" disabled={manualApplicationsLoading} aria-busy={manualApplicationsLoading} onClick={() => void refreshManualApplications()} className={compactButtonClass}>{t(manualApplicationsLoading ? "正在刷新…" : "刷新 App 列表")}</button>
            <button type="submit" disabled={learningBusy || manualApplicationsLoading || manualApplicationsFailed || !manualBundle || !manualBefore.trim() || !manualAfter.trim()} className={buttonClass}>{learningBusy ? t("保存中…") : t("添加专用纠正")}</button>
          </div>
        </form>
      </SettingsGroup>
      {styleDrafts.length > 0 && (
        <SettingsGroup title={t("待确认口癖")} description={t("确认会保存这组样例，并批准用于该 App 后续的整理请求。可在语气的 App 规则中撤销批准。")}>
          {styleDrafts.map((draft) => (
            <div key={draft.draft_key} className="flex flex-wrap items-center gap-3 px-5 py-4 text-sm">
              <span className="min-w-0 flex-1 text-primary">{draft.mapping_id} · {t(draft.style_key === "fewer_periods" ? "少用句号" : draft.style_key === "more_questions" ? "爱用问号" : "标点密度")} · {draft.excerpt}</span>
              <button type="button" disabled={learningBusy} onClick={() => { setLearningActionError(null); setPendingStyleDraft(draft); }} className={compactButtonClass}>{t("确认")}</button>
              <button type="button" disabled={learningBusy} onClick={() => void runLearningAction("dismiss_style_draft", { draftKey: draft.draft_key })} className={`${compactButtonClass} enabled:hover:bg-error/10 enabled:hover:text-error-ink`}>{t("忽略")}</button>
            </div>
          ))}
        </SettingsGroup>
      )}
      <SettingsGroup title={t("已生效替换")} description={`${promotedPairs.length} ${t("条")}`}>
        {promotedPairs.length > 0 ? promotedPairs.map((pair) => {
          const pinned = pinnedTerms.includes(pair.after_surface);
          return (
            <div key={pair.pair_key} className="flex flex-wrap items-center gap-3 px-5 py-4 text-sm">
              <span className="min-w-0 flex-1 text-primary">
                {pair.before_surface} → {pair.after_surface}
                <span className="ml-2 text-xs text-tertiary">{pair.hits} {t("次纠正记录")}</span>
                {pair.promote_hits === 2 ? <span className="ml-2 text-xs text-tertiary">{t("人名")}</span> : null}
                {pair.native_bundle ? <span className="mt-1 block text-xs text-secondary">{t("适用 App：")} {manualApplications?.find((application) => application.bundle_id === pair.native_bundle)?.label ?? pair.native_bundle}</span> : null}
                {pair.manual_scoped ? <span className="mt-1 block text-xs text-secondary">{t("仅本机词典规则；不会写入全局词条")}</span> : null}
              </span>
              {!pair.manual_scoped && <button type="button" disabled={learningBusy} aria-label={`${pinned ? t("取消置顶") : t("置顶")} ${pair.after_surface}`} onClick={() => void runLearningAction("pin_dictionary_term", { word: pair.after_surface, pinned: !pinned })} className={compactButtonClass}>{pinned ? t("已置顶") : t("置顶")}</button>}
              <button type="button" disabled={learningBusy} onClick={() => void runLearningAction("undo_learn_pair", { pairKey: pair.pair_key })} className={`${compactButtonClass} enabled:hover:bg-error/10 enabled:hover:text-error-ink`}>{t("忘记")}</button>
            </div>
          );
        }) : (
          <p className="px-5 py-6 text-sm text-secondary">{t(learningLoaded ? "还没有已生效替换。听写后纠正错字，可在待确认的纠正中确认。" : learningLoading ? "正在读取词典学习数据…" : "词典学习数据尚未读取。")}</p>
        )}
      </SettingsGroup>
      <SettingsDisclosure title={t("学习统计")} description={t("查看本机纠正记录和最近使用的词条。")}>
        <div className="pt-4" aria-label={t("学习反馈")}>
          {learningLoaded ? <>
            <dl className="flex flex-wrap gap-x-6 gap-y-2 text-sm">
              <div className="flex gap-2"><dt className="text-secondary">{t("待确认")}</dt><dd className="font-medium tabular-nums text-primary">{pendingPairs.length}</dd></div>
              <div className="flex gap-2"><dt className="text-secondary">{t("已生效替换")}</dt><dd className="font-medium tabular-nums text-primary">{promotedPairs.length}</dd></div>
              <div className="flex gap-2"><dt className="text-secondary">{t("已用于本机纠正")}</dt><dd className="font-medium tabular-nums text-primary">{liveUsage ? liveUsage.length : "—"}</dd></div>
            </dl>
            <p className="mt-2 max-w-prose text-xs leading-5 text-tertiary">{t("只统计实际发生的本机词典替换，每段处理按词条计一次。历史重新整理也会计入；不代表识别准确率或成功插入次数。")}</p>
            {settings.dictionary_learn_enabled === false && <p className="mt-2 text-xs text-secondary">{t("新纠正的自动观察已关闭；已生效替换仍可使用，也可在“已生效替换”中忘记。")}</p>}
            {recentPairs.length > 0 ? <div className="mt-4">
              <p className="text-xs font-medium text-secondary">{t("最近更新的纠正")}</p>
              <ul className="mt-2 space-y-2 text-xs">
                {recentPairs.map((pair) => <li key={pair.pair_key} className="flex flex-wrap justify-between gap-2">
                  <span className="text-primary">{pair.after_surface}</span>
                  <span className="text-tertiary">{pair.last_at ? `${pair.last_at} UTC` : ""}</span>
                </li>)}
              </ul>
            </div> : <p className="mt-3 text-xs leading-5 text-secondary">{t("完成听写后纠正一个人名或术语，可在“待确认的纠正”中确认或忽略。")}</p>}
          </> : <p role="status" className="text-xs text-secondary">{t(learningLoading ? "正在汇总学习反馈…" : "学习反馈尚未读取。")}</p>}
        </div>
      {liveUsage && liveUsage.length > 0 && <SettingsGroup title={t("最近用于纠正的词条")}>
        {liveUsage.slice(0, 5).map((item) => <div key={item.word} className="flex flex-wrap items-center justify-between gap-2 px-5 py-4 text-sm">
          <span className="text-primary">{item.word}</span>
          <span className="text-xs text-secondary">{item.replacement_runs} {t("次本机处理")} · {item.last_replaced_at} UTC</span>
        </div>)}
      </SettingsGroup>}
      </SettingsDisclosure>
      <DictionaryLearningAudit />
      <ConfirmDialog
        open={pendingStyleDraft != null}
        title={t("确认口癖样例")}
        danger={false}
        description={`${t("将这组纠错前后文字保存为该 App 的风格样例，并允许用于后续整理请求。现在不会发送；可在语气的 App 规则中撤销批准。")}${pendingStyleDraft ? ` ${t("示例输入")}：${pendingStyleDraft.before_excerpt} → ${t("期望输出")}：${pendingStyleDraft.after_excerpt}` : ""}`}
        confirmLabel={t("确认")}
        cancelLabel={t("取消")}
        busy={learningBusy}
        error={learningActionError}
        onCancel={() => setPendingStyleDraft(null)}
        onConfirm={() => {
          if (pendingStyleDraft) {
            void runLearningAction("confirm_style_draft", { draftKey: pendingStyleDraft.draft_key }).then((success) => { if (success) setPendingStyleDraft(null); });
          }
        }}
      />
      <ConfirmDialog
        open={pendingDeleteWord != null}
        title={t("删除词条")}
        description={t("确定删除“{name}”吗？").replace("{name}", pendingDeleteWord ?? "")}
        confirmLabel={t("删除词条")}
        cancelLabel={t("取消")}
        busy={learningBusy}
        error={learningActionError}
        onCancel={() => setPendingDeleteWord(null)}
        onConfirm={() => {
          if (pendingDeleteWord) {
            void runLearningAction("remove_dictionary_word", { word: pendingDeleteWord }).then((success) => { if (success) setPendingDeleteWord(null); });
          }
        }}
      />
    </SettingsShell>
  );
}
