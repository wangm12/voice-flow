import { useCallback, useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import {
  I18nProvider,
  resolveUiLanguage,
  type UiLanguagePreference,
  useI18n,
} from "./lib/i18n";
import {
  isSelectedActionPreview,
  SelectedPreviewDialog,
  type SelectedActionPreview,
} from "./components/SelectedPreviewDialog";
import "./App.css";

type PreviewWindowBootstrap = {
  ui_language: UiLanguagePreference;
  theme: "system" | "light" | "dark";
  preview: SelectedActionPreview;
};

function isPreviewWindowBootstrap(value: unknown): value is PreviewWindowBootstrap {
  if (!value || typeof value !== "object") return false;
  const bootstrap = value as Partial<PreviewWindowBootstrap>;
  return (bootstrap.ui_language === "system" || bootstrap.ui_language === "zh" || bootstrap.ui_language === "en")
    && (bootstrap.theme === "system" || bootstrap.theme === "light" || bootstrap.theme === "dark")
    && isSelectedActionPreview(bootstrap.preview);
}

export function PreviewApp() {
  const [bootstrap, setBootstrap] = useState<PreviewWindowBootstrap | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const activeRef = useRef(false);
  const bootstrapRef = useRef<PreviewWindowBootstrap | null>(null);
  const draftRef = useRef("");
  const loadSequenceRef = useRef(0);
  const commandTransactionRef = useRef<string | null>(null);
  const cancelTransactionRef = useRef<string | null>(null);

  const loadBootstrap = useCallback(async () => {
    const requestSequence = ++loadSequenceRef.current;
    try {
      const result: unknown = await invoke("get_action_preview_bootstrap");
      if (!activeRef.current || requestSequence !== loadSequenceRef.current) return;
      if (result === null) {
        bootstrapRef.current = null;
        draftRef.current = "";
        commandTransactionRef.current = null;
        cancelTransactionRef.current = null;
        setBootstrap(null);
        setDraft("");
        setBusy(false);
        setError(null);
        return;
      }
      if (!isPreviewWindowBootstrap(result)) {
        throw new Error("invalid preview bootstrap");
      }
      const previous = bootstrapRef.current;
      if (previous?.preview.transaction_id !== result.preview.transaction_id) {
        draftRef.current = result.preview.final_text;
        commandTransactionRef.current = null;
        cancelTransactionRef.current = null;
        setDraft(result.preview.final_text);
        setBusy(false);
      }
      bootstrapRef.current = result;
      setBootstrap(result);
      setError(null);
    } catch {
      if (!activeRef.current || requestSequence !== loadSequenceRef.current) return;
      bootstrapRef.current = null;
      draftRef.current = "";
      setBootstrap(null);
      setDraft("");
      setBusy(false);
      setError("无法读取预览内容，请重试。");
    }
  }, []);

  useEffect(() => {
    activeRef.current = true;
    void loadBootstrap();
    window.addEventListener("voiceflow-preview-ready", loadBootstrap);
    return () => {
      activeRef.current = false;
      window.removeEventListener("voiceflow-preview-ready", loadBootstrap);
      // Drop the retained thumbnail and draft when this webview is destroyed.
      bootstrapRef.current = null;
      draftRef.current = "";
    };
  }, [loadBootstrap]);

  useEffect(() => {
    const root = document.documentElement;
    if (!bootstrap || bootstrap.theme === "system") root.removeAttribute("data-theme");
    else root.dataset.theme = bootstrap.theme;
    if (bootstrap) root.lang = resolveUiLanguage(bootstrap.ui_language) === "en" ? "en" : "zh-CN";
    return () => root.removeAttribute("data-theme");
  }, [bootstrap]);

  const changeDraft = (value: string) => {
    draftRef.current = value;
    setDraft(value);
  };

  const runAction = async (action: "confirm" | "copy" | "cancel") => {
    const current = bootstrapRef.current?.preview;
    if (!current) return;
    const transactionId = current.transaction_id;
    if (action !== "cancel" && (!draftRef.current.trim() || cancelTransactionRef.current === transactionId)) return;
    if (action === "cancel") {
      if (cancelTransactionRef.current === transactionId) return;
      cancelTransactionRef.current = transactionId;
    } else {
      if (commandTransactionRef.current) return;
      commandTransactionRef.current = transactionId;
    }
    setBusy(commandTransactionRef.current === transactionId || cancelTransactionRef.current === transactionId);
    setError(null);
    const command = action === "cancel"
      ? current.kind === "screen" ? "cancel_screen_action_preview" : "cancel_selected_action_preview"
      : action === "copy"
        ? current.kind === "screen" ? "copy_screen_action_preview" : "copy_selected_action_preview"
        : current.kind === "screen" ? "confirm_screen_action_preview" : "confirm_selected_action_preview";
    const args = action === "cancel"
      ? { transaction_id: transactionId }
      : { transaction_id: transactionId, final_text: draftRef.current };
    try {
      const result: unknown = await invoke(command, args);
      const validResult = action === "cancel"
        || (action === "copy"
          ? result === "copied" || result === "copied_target_changed"
          : result === "replaced" || result === "unverified" || result === "copied" || result === "copied_target_changed");
      if (!validResult) throw new Error("unexpected preview action result");
      if (bootstrapRef.current?.preview.transaction_id === transactionId) {
        bootstrapRef.current = null;
        draftRef.current = "";
        setBootstrap(null);
        setDraft("");
      }
    } catch {
      if (bootstrapRef.current?.preview.transaction_id === transactionId) {
        setError("无法完成这个文字操作。请检查后重试或取消。");
      }
    } finally {
      if (commandTransactionRef.current === transactionId) {
        commandTransactionRef.current = null;
        setBusy(false);
      }
      if (cancelTransactionRef.current === transactionId) {
        cancelTransactionRef.current = null;
      }
      if (bootstrapRef.current?.preview.transaction_id === transactionId) {
        setBusy(commandTransactionRef.current === transactionId || cancelTransactionRef.current === transactionId);
      }
    }
  };

  const language = bootstrap ? resolveUiLanguage(bootstrap.ui_language) : undefined;
  return (
    <I18nProvider key={language ?? "system"} initialLanguage={language}>
      <PreviewWindowContent
        preview={bootstrap?.preview ?? null}
        draft={draft}
        busy={busy}
        error={error}
        onDraftChange={changeDraft}
        onCancel={() => void runAction("cancel")}
        onCopy={() => void runAction("copy")}
        onConfirm={() => void runAction("confirm")}
      />
    </I18nProvider>
  );
}

function PreviewWindowContent({
  preview,
  draft,
  busy,
  error,
  onDraftChange,
  onCancel,
  onCopy,
  onConfirm,
}: {
  preview: SelectedActionPreview | null;
  draft: string;
  busy: boolean;
  error: string | null;
  onDraftChange: (value: string) => void;
  onCancel: () => void;
  onCopy: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  useEffect(() => {
    document.documentElement.style.height = "100%";
    document.body.style.height = "100%";
    document.body.style.margin = "0";
    document.body.style.overflow = "hidden";
    return () => {
      document.documentElement.style.height = "";
      document.body.style.height = "";
      document.body.style.margin = "";
      document.body.style.overflow = "";
    };
  }, []);

  return (
    <>
      {!preview && <main className="flex h-full items-center justify-center bg-base px-4 text-center text-xs text-secondary" role={error ? "alert" : "status"}>
        {error ? t(error) : t("正在等待预览内容…")}
      </main>}
      {preview && <SelectedPreviewDialog
        standalone
        preview={preview}
        draft={draft}
        onDraftChange={onDraftChange}
        onCancel={onCancel}
        onCopy={onCopy}
        onConfirm={onConfirm}
        busy={busy}
        error={error ? t(error) : null}
      />}
    </>
  );
}

if (typeof document !== "undefined" && document.getElementById("root")) {
  document.documentElement.dataset.platform = /Mac/i.test(navigator.userAgent) ? "macos" : "other";
  ReactDOM.createRoot(document.getElementById("root")!).render(<PreviewApp />);
}
