import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
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

export function DictionarySettings({ settings, save }: { settings: Settings; save: SaveSettings }) {
  const { t } = useI18n();
  const [newWord, setNewWord] = useState("");
  const [dictionaryMessage, setDictionaryMessage] = useState<string | null>(null);
  const [dictionaryDragging, setDictionaryDragging] = useState(false);
  const [dictionaryImporting, setDictionaryImporting] = useState(false);
  const [pendingDeleteWord, setPendingDeleteWord] = useState<string | null>(null);
  const dictionaryFileInput = useRef<HTMLInputElement | null>(null);
  const dictionaryDropZone = useRef<HTMLDivElement | null>(null);
  const importingRef = useRef(false);
  const settingsRef = useRef(settings);
  const saveRef = useRef(save);
  settingsRef.current = settings;
  saveRef.current = save;

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
      if (result.added > 0) saveRef.current({ dictionary: result.words });
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
    save({ dictionary: result.words });
    setNewWord("");
    setDictionaryMessage(null);
  };

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
        <SettingsRow title={t("从历史编辑学习词条")} description={t("在历史里改正识别结果后，建议把新词加入个人词典，需确认后才会写入。关闭后不再显示这些建议。")}>
          <Toggle checked={settings.dictionary_learn_enabled !== false} onChange={(checked) => save({ dictionary_learn_enabled: checked })} label={t("从历史编辑学习词条")} />
        </SettingsRow>
      </SettingsGroup>
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
      <SettingsGroup title={t("词条列表")} description={`${settings.dictionary.length} ${t("条")}`}>
        {settings.dictionary.length > 0 ? settings.dictionary.map((word) => (
          <div key={word} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
            <span className="flex-1 text-primary">{word}</span>
            <button type="button" aria-label={`${t("删除")} ${word}`} onClick={() => setPendingDeleteWord(word)} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("删除")}</button>
          </div>
        )) : <p className="px-4 py-5 text-sm text-tertiary sm:px-5">{t("还没有词条。添加后，VoiceFlow 会更准确地识别人名和专业术语。")}</p>}
      </SettingsGroup>
      <ConfirmDialog
        open={pendingDeleteWord != null}
        title={t("删除词条")}
        description={t("确定删除“{name}”吗？").replace("{name}", pendingDeleteWord ?? "")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setPendingDeleteWord(null)}
        onConfirm={() => {
          if (pendingDeleteWord) save({ dictionary: settings.dictionary.filter((item) => item !== pendingDeleteWord) });
          setPendingDeleteWord(null);
        }}
      />
    </SettingsShell>
  );
}
