import { Loader2, Mic } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { tryItHint } from "../../lib/activationCopy";
import { radius } from "../../lib/theme";
import { iconPropsLg } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function TryItStep({
  trial,
  recording,
  processing,
  hotkeyDisplay,
  selectedActionHotkeyDisplay,
  activationMode,
  error,
}: {
  trial: "dictation" | "selected_action";
  recording: boolean;
  processing: boolean;
  hotkeyDisplay: string;
  selectedActionHotkeyDisplay: string;
  activationMode: string;
  error?: string | null;
}) {
  const { t } = useI18n();
  const hint = tryItHint(activationMode, hotkeyDisplay, t);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const selectedActionInputRef = useRef<HTMLTextAreaElement>(null);
  const activeTrialRef = useRef<"dictation" | "selected_action">(trial);
  const selectedTextRangeRef = useRef({ start: 0, end: 0 });
  const [inputText, setInputText] = useState("");
  const selectedActionPrompt = t("选中这段文字，然后用语音告诉 VoiceFlow 你希望怎样改写它。");
  const initialSelectedActionPromptRef = useRef(selectedActionPrompt);
  const [selectedActionText, setSelectedActionText] = useState(selectedActionPrompt);
  const [selectedActionState, setSelectedActionState] = useState("idle");

  useEffect(() => {
    setSelectedActionText((current) => current === initialSelectedActionPromptRef.current ? selectedActionPrompt : current);
    initialSelectedActionPromptRef.current = selectedActionPrompt;
  }, [selectedActionPrompt]);

  useEffect(() => {
    if (trial === "dictation") {
      inputRef.current?.focus();
    } else {
      selectedActionInputRef.current?.focus();
    }
  }, [trial]);

  useEffect(() => {
    if (trial !== "dictation" || !recording || selectedActionState !== "idle") return;
    activeTrialRef.current = "dictation";
  }, [recording, selectedActionState, trial]);

  useEffect(() => {
    let active = true;
    const subscription = listen<{ state: string }>("selected-action://state", (event) => {
      if (!active) return;
      const next = event.payload.state;
      setSelectedActionState(next);
      if (next !== "idle") {
        activeTrialRef.current = "selected_action";
      }
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let active = true;
    const subscription = listen<{ raw_text: string; final_text: string }>("selected-action://onboarding-result", (event) => {
      if (!active) return;
      const result = event.payload;
      activeTrialRef.current = "selected_action";
      setSelectedActionText((current) => {
        const { start, end } = selectedTextRangeRef.current;
        const next = `${current.slice(0, start)}${result.final_text}${current.slice(end)}`;
        window.requestAnimationFrame(() => {
          const input = selectedActionInputRef.current;
          if (!input) return;
          const caret = start + result.final_text.length;
          input.focus();
          input.setSelectionRange(caret, caret);
        });
        return next;
      });
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const captureSelectedText = (event: SyntheticEvent<HTMLTextAreaElement>) => {
    const input = event.currentTarget;
    const start = input.selectionStart;
    const end = input.selectionEnd;
    selectedTextRangeRef.current = { start, end };
    void invoke("set_onboarding_selected_text", { text: input.value.slice(start, end) }).catch(() => undefined);
  };

  const dictationStatus = recording
    ? activationMode === "double_tap"
      ? t("正在录音… 再双击功能键结束")
      : t("正在录音… 再按一次热键结束")
    : processing && activeTrialRef.current === "dictation"
      ? t("正在处理… 请稍候")
      : t("等待你按语音输入快捷键开始");
  const selectedActionStatus = selectedActionState === "waiting_for_selection"
    ? t("请选择一段文字后重试")
    : selectedActionState === "listening" || (recording && activeTrialRef.current === "selected_action")
      ? t("正在录音… 再按一次快捷键结束")
      : selectedActionState === "preparing_rewrite" || (processing && activeTrialRef.current === "selected_action")
        ? t("正在改写选中的文字…")
        : t("选中文字后，按选中文本操作快捷键开始");

  useEffect(() => {
    let active = true;
    const subscription = listen<{ final_text: string }>("dictation://onboarding-result", (event) => {
      if (!active || !event.payload.final_text) return;
      const insertedText = event.payload.final_text;
      setInputText((current) => {
        const input = inputRef.current;
        const start = input?.selectionStart ?? current.length;
        const end = input?.selectionEnd ?? start;
        const next = `${current.slice(0, start)}${insertedText}${current.slice(end)}`;
        window.requestAnimationFrame(() => {
          if (!inputRef.current) return;
          inputRef.current.focus();
          const caret = start + insertedText.length;
          inputRef.current.setSelectionRange(caret, caret);
        });
        return next;
      });
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  return (
    <div>
      {trial === "dictation" && <section className="mt-6 border-y border-border py-4" aria-labelledby="dictation-trial-heading">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p id="dictation-trial-heading" className="text-sm font-medium text-primary">{t("1. 语音输入")}</p>
            <p className="mt-1 text-xs text-tertiary">{hint}</p>
          </div>
          <kbd className="shrink-0 rounded-md border border-border bg-elevated px-2 py-1 text-xs font-medium text-primary">{hotkeyDisplay}</kbd>
        </div>
        <label htmlFor="onboarding-try-input" className="sr-only">{t("语音输入试用框")}</label>
        <textarea
          ref={inputRef}
          id="onboarding-try-input"
          value={inputText}
          onChange={(event) => setInputText(event.target.value)}
          placeholder={t("把光标放在这里，然后按语音输入快捷键开始…")}
          rows={3}
          className={`mt-3 w-full resize-none ${radius.control} border border-border bg-base p-3 text-sm text-primary outline-none transition-colors placeholder:text-tertiary focus:border-accent`}
        />
        <TrialStatus active={recording && activeTrialRef.current === "dictation"} processing={processing && activeTrialRef.current === "dictation"} text={dictationStatus} />
      </section>}

      {trial === "selected_action" && <section className="mt-6 border-y border-border py-4" aria-labelledby="selected-action-trial-heading">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p id="selected-action-trial-heading" className="text-sm font-medium text-primary">{t("2. 选中文本操作")}</p>
            <p className="mt-1 text-xs leading-5 text-tertiary">{t("选中下面一句话，按快捷键后说出要求，例如“翻译成英文”、“缩短”或“改写得更专业”。")}</p>
          </div>
          <kbd className="shrink-0 rounded-md border border-border bg-elevated px-2 py-1 text-xs font-medium text-primary">{selectedActionHotkeyDisplay}</kbd>
        </div>
        <textarea
          ref={selectedActionInputRef}
          aria-label={t("选中文本操作试用框")}
          value={selectedActionText}
          onChange={(event) => setSelectedActionText(event.target.value)}
          onSelect={captureSelectedText}
          rows={3}
          className={`mt-3 w-full resize-none ${radius.control} border border-border bg-base p-3 text-sm text-primary outline-none transition-colors focus:border-accent`}
        />
        <TrialStatus active={selectedActionState === "listening" || (recording && activeTrialRef.current === "selected_action")} processing={selectedActionState === "preparing_rewrite" || (processing && activeTrialRef.current === "selected_action")} text={selectedActionStatus} />
      </section>}
      {error && <p role="alert" className="mt-3 text-xs text-error">{error}</p>}
    </div>
  );
}

function TrialStatus({ active, processing, text }: { active: boolean; processing: boolean; text: string }) {
  return (
    <div className="mt-3 flex items-center gap-2.5 text-xs" aria-live="polite">
      {processing ? <Loader2 {...iconPropsLg} className="animate-spin text-secondary motion-reduce:animate-none" aria-hidden="true" /> : <Mic {...iconPropsLg} className={active ? "text-error" : "text-secondary"} aria-hidden="true" />}
      <span className={active || processing ? "text-primary" : "text-secondary"}>{text}</span>
    </div>
  );
}
