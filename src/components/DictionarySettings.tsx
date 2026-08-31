import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FileText, Upload } from "lucide-react";
import { ConfirmDialog } from "./ConfirmDialog";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "./SettingsLayout";
import { MAX_DICTIONARY_FILE_BYTES, mergeDictionary, parseDictionaryText } from "../lib/dictionaryImport";
import { useI18n } from "../lib/i18n";
import { buttonClass, colors, focusRingClass, radius } from "../lib/theme";
import { Toggle } from "./Toggle";
import type { SaveSettings, Settings } from "../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;

type LearnPair = {
  pair_key: string;
  before_surface: string;
  after_surface: string;
  hits: number;
  promoted: boolean;
  ignored?: boolean;
  promote_hits?: number;
  pinned?: boolean;
};

type StyleDraft = {
  draft_key: string;
  mapping_id: string;
  style_key: string;
  excerpt: string;
  before_excerpt: string;
  after_excerpt: string;
};

export function DictionarySettings({ settings, save }: { settings: Settings; save: SaveSettings }) {
  const { t } = useI18n();
  const [newWord, setNewWord] = useState("");
  const [dictionaryMessage, setDictionaryMessage] = useState<string | null>(null);
  const [dictionaryDragging, setDictionaryDragging] = useState(false);
  const [dictionaryImporting, setDictionaryImporting] = useState(false);
  const [pendingDeleteWord, setPendingDeleteWord] = useState<string | null>(null);
  const [pendingPairs, setPendingPairs] = useState<LearnPair[]>([]);
  const [promotedPairs, setPromotedPairs] = useState<LearnPair[]>([]);
  const [styleDrafts, setStyleDrafts] = useState<StyleDraft[]>([]);
  const [pinnedTerms, setPinnedTerms] = useState<string[]>([]);
  const [pendingStyleDraft, setPendingStyleDraft] = useState<StyleDraft | null>(null);
  const dictionaryFileInput = useRef<HTMLInputElement | null>(null);
  const dictionaryDropZone = useRef<HTMLDivElement | null>(null);
  const importingRef = useRef(false);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const loadPendingPairs = useCallback(async () => {
    try {
      const [rows, drafts, pinned] = await Promise.all([
        invoke<LearnPair[]>("list_learn_pairs"),
        invoke<StyleDraft[]>("list_style_drafts"),
        invoke<string[]>("list_pinned_terms"),
      ]);
      const live = Array.isArray(rows) ? rows.filter((row) => !row.ignored) : [];
      setPendingPairs(live.filter((row) => !row.promoted));
      setPromotedPairs(live.filter((row) => row.promoted && row.before_surface));
      setStyleDrafts(Array.isArray(drafts) ? drafts : []);
      setPinnedTerms(Array.isArray(pinned) ? pinned : []);
    } catch {
      setPendingPairs([]);
      setStyleDrafts([]);
      setPinnedTerms([]);
    }
  }, []);

  useEffect(() => {
    void loadPendingPairs();
    let active = true;
    const subscription = listen("learn_pairs://changed", () => {
      if (active) void loadPendingPairs();
    });
    const drafts = listen("style_drafts://changed", () => {
      if (active) void loadPendingPairs();
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
      void drafts.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [loadPendingPairs]);

  const importText = useCallback(async (text: string, fileName: string) => {
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (importingRef.current) return;

    importingRef.current = true;
    setDictionaryImporting(true);
    setDictionaryMessage(null);
    try {
      const result = mergeDictionary(settingsRef.current.dictionary, parseDictionaryText(text, fileName));
      if (result.added > 0) {
        await invoke("add_dictionary_entries", { words: result.words });
      }
      const details = [
        `${t("已导入")} ${result.added} ${t("条")}`,
        result.duplicates > 0 ? `${t("重复")} ${result.duplicates} ${t("条")}` : null,
        result.invalid > 0 ? `${t("无效")} ${result.invalid} ${t("条")}` : null,
        result.limited > 0 ? `${t("超出上限")} ${result.limited} ${t("条")}` : null,
      ].filter(Boolean).join(" · ");
      setDictionaryMessage(details || t("没有找到可导入的新词条。"));
    } catch (reason) {
      setDictionaryMessage(t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    } finally {
      importingRef.current = false;
      setDictionaryImporting(false);
    }
  }, [t]);

  const importFile = useCallback(async (file: File) => {
    const extension = file.name.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (file.size > MAX_DICTIONARY_FILE_BYTES) {
      setDictionaryMessage(t("词典文件不能超过 1 MB。"));
      return;
    }
    try {
      const decoder = new TextDecoder("utf-8", { fatal: true });
      await importText(decoder.decode(await file.arrayBuffer()), file.name);
    } catch (reason) {
      setDictionaryMessage(reason instanceof TypeError ? t("导入失败：词典文件必须使用 UTF-8 编码。") : t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    }
  }, [importText, t]);

  const importNativeFile = useCallback(async (path: string) => {
    const fileName = path.split(/[\\/]/).pop() || "dictionary.csv";
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    try {
      const text = await invoke<string>("read_dictionary_file", { path });
      await importText(text, fileName);
    } catch (reason) {
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
    const word = newWord.trim();
    if (!word) return;
    const result = mergeDictionary(settings.dictionary, [word]);
    if (result.added === 0) {
      setDictionaryMessage(result.duplicates > 0 ? t("这个词条已经存在。") : t("词条无效或已达到上限。"));
      return;
    }
    void invoke("add_dictionary_entries", { words: result.words })
      .then(() => {
        setNewWord("");
        setDictionaryMessage(null);
      })
      .catch(() => {
        setDictionaryMessage(t("词条无效或已达到上限。"));
      });
  };

  const learnedAfters = new Set(promotedPairs.map((pair) => pair.after_surface));
  const manualWords = settings.dictionary.filter((word) => !learnedAfters.has(word));

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
      <SettingsGroup title={t("词典学习")}>
        <SettingsRow title={t("学习词条")} description={t("默认第 3 次静默纠正，人名第 2 次；也可在历史或本页确认。继续打字不会学习。")}>
          <Toggle checked={settings.dictionary_learn_enabled !== false} onChange={(checked) => save({ dictionary_learn_enabled: checked })} label={t("学习词条")} />
        </SettingsRow>
      </SettingsGroup>
      {pendingPairs.length > 0 && (
        <SettingsGroup title={t("待晋升")}>
          {pendingPairs.map((pair) => (
            <div key={pair.pair_key} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
              <span className="min-w-0 flex-1 text-primary">{pair.before_surface} → {pair.after_surface} · {pair.hits}/{pair.promote_hits || 3}</span>
              <button type="button" onClick={() => void invoke("promote_learn_pair", { pairKey: pair.pair_key, beforeSurface: pair.before_surface, afterSurface: pair.after_surface }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-secondary transition-colors hover:bg-elevated hover:text-primary">{t("确认")}</button>
              <button type="button" onClick={() => void invoke("ignore_learn_pair", { pairKey: pair.pair_key }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("忽略")}</button>
            </div>
          ))}
        </SettingsGroup>
      )}
      {styleDrafts.length > 0 && (
        <SettingsGroup title={t("待确认口癖")} description={t("确认后会覆盖该 App 现有的风格样例。")}>
          {styleDrafts.map((draft) => (
            <div key={draft.draft_key} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
              <span className="min-w-0 flex-1 text-primary">{draft.mapping_id} · {t(draft.style_key === "fewer_periods" ? "少用句号" : draft.style_key === "more_questions" ? "爱用问号" : "标点密度")} · {draft.excerpt}</span>
              <button type="button" onClick={() => setPendingStyleDraft(draft)} className="rounded-lg px-2 py-1 text-xs text-secondary transition-colors hover:bg-elevated hover:text-primary">{t("确认")}</button>
              <button type="button" onClick={() => void invoke("dismiss_style_draft", { draftKey: draft.draft_key }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("忽略")}</button>
            </div>
          ))}
        </SettingsGroup>
      )}
      <SettingsGroup title={t("添加词条")}>
        <SettingsRow title={t("添加个人词典词条")} description={t("输入一个词条，或从 CSV、TXT、TSV 文件导入。")}>
          <div className="flex w-full min-w-0 gap-2 sm:w-auto">
            <input aria-label={t("添加个人词典词条")} value={newWord} onChange={(event) => setNewWord(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") addWord(); }} placeholder={t("添加人名或术语…")} className={"min-w-0 flex-1 sm:w-48 " + controlClass} />
            <button type="button" onClick={addWord} disabled={!newWord.trim()} className={buttonClass}>{t("添加")}</button>
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
          className={`mx-4 my-3 flex min-h-16 cursor-pointer items-center gap-3 rounded-lg border border-dashed px-3 py-2.5 transition-colors sm:mx-5 ${dictionaryDragging ? "border-accent bg-elevated" : "border-border bg-elevated/40 hover:border-accent/50 hover:bg-elevated/70"} ${dictionaryImporting ? "pointer-events-none opacity-60" : ""}`}
        >
          <Upload size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-primary">{t("拖入词典文件，或点击选择")}</span>
            <span className="mt-0.5 block text-xs text-tertiary">{t("支持 CSV、TXT、TSV；CSV 默认读取第一列，每行一个词条。")} </span>
          </span>
          <FileText size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <input ref={dictionaryFileInput} type="file" accept=".csv,.txt,.tsv,text/csv,text/plain,text/tab-separated-values" className="sr-only" onChange={(event) => { const file = event.target.files?.[0]; event.target.value = ""; if (file) void importFile(file); }} />
        </div>
        {dictionaryMessage && <p role="status" className="px-4 pb-4 text-xs text-secondary sm:px-5">{dictionaryMessage}</p>}
      </SettingsGroup>
      <SettingsGroup title={t("已生效替换")} description={`${promotedPairs.length} ${t("条")}`}>
        {promotedPairs.length > 0 ? promotedPairs.map((pair) => {
          const pinned = pinnedTerms.includes(pair.after_surface);
          return (
            <div key={pair.pair_key} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
              <span className="min-w-0 flex-1 text-primary">
                {pair.before_surface} → {pair.after_surface}
                <span className="ml-2 text-xs text-tertiary">{pair.hits} {t("次")}</span>
                {pair.promote_hits === 2 ? <span className="ml-2 text-xs text-tertiary">{t("人名")}</span> : null}
              </span>
              <button type="button" aria-label={`${pinned ? t("取消置顶") : t("置顶")} ${pair.after_surface}`} onClick={() => void invoke("pin_dictionary_term", { word: pair.after_surface, pinned: !pinned }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-secondary transition-colors hover:bg-elevated hover:text-primary">{pinned ? t("已置顶") : t("置顶")}</button>
              <button type="button" onClick={() => void invoke("undo_learn_pair", { pairKey: pair.pair_key }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("忘记")}</button>
            </div>
          );
        }) : (
          <p className="px-4 py-5 text-sm text-tertiary sm:px-5">{t("还没有学到替换。听写后把错字改对，第 2 或 3 次会进入待晋升；确认后出现在已生效替换。")}</p>
        )}
      </SettingsGroup>
      <SettingsGroup title={t("词条列表")} description={`${manualWords.length} ${t("条")}`}>
        {manualWords.length > 0 ? manualWords.map((word) => (
          <div key={word} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
            <span className="flex-1 text-primary">{word}</span>
            <button type="button" aria-label={`${pinnedTerms.includes(word) ? t("取消置顶") : t("置顶")} ${word}`} onClick={() => void invoke("pin_dictionary_term", { word, pinned: !pinnedTerms.includes(word) }).then(() => loadPendingPairs())} className="rounded-lg px-2 py-1 text-xs text-secondary transition-colors hover:bg-elevated hover:text-primary">{pinnedTerms.includes(word) ? t("已置顶") : t("置顶")}</button>
            <button type="button" aria-label={`${t("删除")} ${word}`} onClick={() => setPendingDeleteWord(word)} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("删除")}</button>
          </div>
        )) : <p className="px-4 py-5 text-sm text-tertiary sm:px-5">{t("没有手加或导入的词条。")}</p>}
      </SettingsGroup>
      <ConfirmDialog
        open={pendingStyleDraft != null}
        title={t("确认口癖样例")}
        description={t("会覆盖该 App 现有的风格样例。")}
        confirmLabel={t("确认")}
        cancelLabel={t("取消")}
        onCancel={() => setPendingStyleDraft(null)}
        onConfirm={() => {
          if (pendingStyleDraft) {
            void invoke("confirm_style_draft", { draftKey: pendingStyleDraft.draft_key }).then(() => loadPendingPairs());
          }
          setPendingStyleDraft(null);
        }}
      />
      <ConfirmDialog
        open={pendingDeleteWord != null}
        title={t("删除词条")}
        description={t("确定删除“{name}”吗？").replace("{name}", pendingDeleteWord ?? "")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setPendingDeleteWord(null)}
        onConfirm={() => {
          if (pendingDeleteWord) {
            void invoke("remove_dictionary_word", { word: pendingDeleteWord });
          }
          setPendingDeleteWord(null);
        }}
      />
    </SettingsShell>
  );
}
