import { Loader2, Mic } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { tryItHint } from "../../lib/activationCopy";
import { matchesTrialMetadata, matchesTrialReceipt, type TrialLease, type TrialReceipt } from "../../lib/onboardingReadiness";
import type { OnboardingSettingsPage } from "./DataChoices";
import { radius, focusRingClass, ghostButtonClass, secondaryButtonClass } from "../../lib/theme";
import { iconPropsLg } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";
import { SelectedLesson } from "./ContextualLessons";

export function TryItStep({
  trial,
  recording,
  processing,
  hotkeyDisplay,
  selectedActionHotkeyDisplay,
  activationMode,
  showHotkeyHint = true,
  error, errorDetail, trialReady = true, trialLease = null, configurationGeneration = "", onTrialResult, onRetry, onCancel, onSettings,
}: {
  trial: "dictation" | "selected_action";
  recording: boolean;
  processing: boolean;
  hotkeyDisplay: string;
  selectedActionHotkeyDisplay: string;
  activationMode: string;
  showHotkeyHint?: boolean;
  error?: string | null;
  errorDetail?: string | null;
  trialReady?: boolean; trialLease?: TrialLease | null; configurationGeneration?: string;
  onTrialResult?: (receipt: TrialReceipt) => void; onRetry?: () => void; onCancel?: () => Promise<void>; onSettings?: (page: OnboardingSettingsPage) => void;
}) {
  const { t } = useI18n();
  const hint = tryItHint(activationMode, hotkeyDisplay, t);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const selectedActionInputRef = useRef<HTMLTextAreaElement>(null);
  const activeTrialRef = useRef<"dictation" | "selected_action">(trial);
  const [inputText, setInputText] = useState("");
  const selectedActionPrompt = t("周五下午三点确认 API 方案。");
  const initialSelectedActionPromptRef = useRef(selectedActionPrompt);
  const [selectedActionText, setSelectedActionText] = useState(selectedActionPrompt);
  const [selectedActionState, setSelectedActionState] = useState("idle");
  const [deliveryNote, setDeliveryNote] = useState<string | null>(null);
  const receivedGeneration = useRef<string | null>(null);
  const appliedGeneration = useRef<string | null>(null);
  const selectedSnapshot = useRef<{ generation: string; source: { start: number; end: number; text: string; focused: boolean } | null } | null>(null);
  const [recoverableResult, setRecoverableResult] = useState<string | null>(null);
  const [sourceText, setSourceText] = useState<string | null>(null);
  const leaseRef = useRef(trialLease);
  leaseRef.current = trialReady ? trialLease : null;
  const resultCallback = useRef(onTrialResult);
  resultCallback.current = onTrialResult;
  const currentTrial = useRef(trial);
  currentTrial.current = trial;
  useEffect(() => {
    setInputText(""); setSourceText(null); setDeliveryNote(null); setSelectedActionState("idle"); setRecoverableResult(null);
    receivedGeneration.current = null; appliedGeneration.current = null; selectedSnapshot.current = null;
    leaseRef.current = trialReady ? trialLease : null;
    return () => { leaseRef.current = null; };
  }, [trial, configurationGeneration, trialLease?.trial_id, trialReady]);

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
    const subscription = listen<unknown>("selected-action://state", (event) => {
      if (!active || currentTrial.current !== "selected_action" || !matchesTrialMetadata(event.payload, leaseRef.current)) return;
      if (receivedGeneration.current && BigInt(event.payload.recording_generation) < BigInt(receivedGeneration.current)) return;
      receivedGeneration.current = event.payload.recording_generation;
      const next = (event.payload as TrialReceipt & { state: string }).state;
      if (next === "listening" && selectedSnapshot.current?.generation !== event.payload.recording_generation) {
        const input = selectedActionInputRef.current;
        selectedSnapshot.current = { generation: event.payload.recording_generation, source: input
          ? { start: input.selectionStart, end: input.selectionEnd, text: input.value, focused: document.activeElement === input }
          : null };
      }
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
    const subscription = listen<unknown>("selected-action://onboarding-result", (event) => {
      if (!active || currentTrial.current !== "selected_action" || !matchesTrialReceipt(event.payload, leaseRef.current)) return;
      const result = event.payload;
      if ((receivedGeneration.current && receivedGeneration.current !== result.recording_generation)
        || (appliedGeneration.current && BigInt(result.recording_generation) <= BigInt(appliedGeneration.current))) return;
      appliedGeneration.current = result.recording_generation;
      const snapshot = selectedSnapshot.current;
      const input = selectedActionInputRef.current;
      const source = snapshot?.source;
      if (!snapshot || snapshot.generation !== result.recording_generation || !source || !source.focused
        || !input || document.activeElement !== input || input.value !== source.text
        || input.selectionStart !== source.start || input.selectionEnd !== source.end) {
        setRecoverableResult(result.final_text);
        setDeliveryNote(t("试用来源已改变或无法确认；结果保留在下方，不修改原文字。"));
        return;
      }
      activeTrialRef.current = "selected_action";
      // This already-admitted DOM operation commits text and its end selection
      // together. Controlled state consumes the actual value; there is no later
      // callback with authority to overwrite a user's next range/recording.
      input.setRangeText(result.final_text, source.start, source.end, "end");
      setSelectedActionText(input.value);
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
    const lease = leaseRef.current;
    if (!lease) return;
    void invoke("set_onboarding_selected_text", { text: input.value.slice(start, end), trialId: lease.trial_id }).catch(() => {
      if (leaseRef.current?.trial_id === lease.trial_id) setDeliveryNote(t("试用选区未能保存，请重新选择后重试。"));
    });
  };

  const dictationStatus = recording
    ? activationMode === "hold_to_talk"
      ? t("正在录音… 松开快捷键结束")
      : t("正在录音… 再按一次快捷键结束")
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
    const subscription = listen<unknown>("dictation://state", (event) => {
      if (!active || currentTrial.current !== "dictation" || !matchesTrialMetadata(event.payload, leaseRef.current)) return;
      if (!receivedGeneration.current || BigInt(event.payload.recording_generation) >= BigInt(receivedGeneration.current)) receivedGeneration.current = event.payload.recording_generation;
    });
    return () => { active = false; void subscription.then((unlisten) => unlisten()).catch(() => undefined); };
  }, []);

  useEffect(() => {
    let active = true;
    const subscription = listen<unknown>("dictation://onboarding-result", (event) => {
      if (!active || currentTrial.current !== "dictation") return;
      if (!matchesTrialReceipt(event.payload, leaseRef.current)) {
        setDeliveryNote(t("试用结果缺少当前会话回执，未作为本次成功。请重新试用。"));
        return;
      }
      if (!event.payload.final_text) return;
      if ((receivedGeneration.current && event.payload.recording_generation !== receivedGeneration.current)
        || (appliedGeneration.current && BigInt(event.payload.recording_generation) <= BigInt(appliedGeneration.current))) return;
      appliedGeneration.current = event.payload.recording_generation;
      const insertedText = event.payload.final_text;
      setSourceText(typeof event.payload.raw_text === "string" ? event.payload.raw_text : null);
      setDeliveryNote(t("本次应用内结果已收到；这不是外部插入确认。"));
      resultCallback.current?.(event.payload);
      setInputText((current) => {
        const input = inputRef.current;
        const start = input?.selectionStart ?? current.length;
        const end = input?.selectionEnd ?? start;
        const next = `${current.slice(0, start)}${insertedText}${current.slice(end)}`;
        window.requestAnimationFrame(() => {
          if (!active || !inputRef.current || !matchesTrialReceipt(event.payload, leaseRef.current)) return;
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
  }, [configurationGeneration, trial, trialLease?.trial_id, t]);

  const cancel = async () => {
    leaseRef.current = null;
    try { if (onCancel) await onCancel(); else throw new Error("trial_owner_unavailable"); setDeliveryNote(t("本次试用已取消；迟到结果不会显示。")); }
    catch { setDeliveryNote(t("无法确认取消，请等待当前操作结束。")); }
  };

  return (
    <div>
      <p className="mt-4 text-xs leading-5 text-secondary">{t("此处是真实应用内试用，结果只更新试用框，不向其他 App 粘贴。可以稍后试用。")}</p>
      {!trialReady && <p role="status" className="mt-2 text-xs text-warning-ink">{t("试用正在准备；完成后再按快捷键。")}</p>}
      {trial === "dictation" && <section className="mt-6 border-y border-border py-4" aria-labelledby="dictation-trial-heading">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p id="dictation-trial-heading" className="text-sm font-medium text-primary">{t("1. 语音输入")}</p>
            {showHotkeyHint && <p className="mt-1 text-xs text-tertiary">{hint}</p>}
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
          className={`mt-3 w-full resize-none ${radius.control} border border-border bg-base p-3 text-sm text-primary outline-none transition-colors placeholder:text-tertiary focus:border-accent ${focusRingClass}`}
        />
        <p className="mt-2 text-xs text-tertiary">{t("试说：周五下午三点确认 API 方案。 / Please review the API plan on Friday.")}</p>
        {sourceText !== null && <details className="mt-3 text-xs text-secondary"><summary>{t("查看试用来源文字（未认证纯 ASR）")}</summary><p className="mt-2 whitespace-pre-wrap">{sourceText}</p></details>}
        <TrialStatus active={recording && activeTrialRef.current === "dictation"} processing={processing && activeTrialRef.current === "dictation"} text={dictationStatus} />
        {deliveryNote && trial === "dictation" && (
          <p role="status" className="mt-3 text-xs text-warning-ink">{deliveryNote}</p>
        )}
      </section>}

      {trial === "selected_action" && <SelectedLesson />}
      {trial === "selected_action" && <section className="mt-6 border-y border-border py-4" aria-labelledby="selected-action-trial-heading">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p id="selected-action-trial-heading" className="text-sm font-medium text-primary">{t("2. 选中文本操作")}</p>
            <p className="mt-1 text-xs leading-5 text-tertiary">{t("固定练习原稿，可自行编辑。选中下面一句话后，用独立快捷键说出任务，例如“翻译成英文”、“缩短”或“改写得更专业”。")}</p>
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
          className={`mt-3 w-full resize-none ${radius.control} border border-border bg-base p-3 text-sm text-primary outline-none transition-colors focus:border-accent ${focusRingClass}`}
        />
        <TrialStatus active={selectedActionState === "listening" || (recording && activeTrialRef.current === "selected_action")} processing={selectedActionState === "preparing_rewrite" || (processing && activeTrialRef.current === "selected_action")} text={selectedActionStatus} />
        {deliveryNote && trial === "selected_action" && (
          <p role="status" className="mt-3 text-xs text-warning-ink">{deliveryNote}</p>
        )}
      </section>}
      {recoverableResult !== null && <textarea readOnly aria-label={t("可恢复的试用结果")} value={recoverableResult} rows={3} className="mt-3 w-full rounded-lg border border-border bg-base p-3 text-sm" />}
      {error && <p role="alert" className="mt-3 text-xs text-error-ink">{error}</p>}
      {errorDetail && <details className="mt-2 text-xs text-tertiary"><summary className="cursor-pointer">{t("试用错误详情：")}</summary><pre className="mt-1 whitespace-pre-wrap break-words">{errorDetail}</pre></details>}
      <div className="mt-4 flex flex-wrap gap-2">
        {(recording || processing) && <button type="button" className={secondaryButtonClass} onClick={() => void cancel()}>{t("取消本次试用")}</button>}
        {!recording && !processing && onRetry && <button type="button" className={secondaryButtonClass} onClick={onRetry}>{t("重新准备试用")}</button>}
        {!recording && !processing && onSettings && <><button type="button" className={ghostButtonClass} onClick={() => onSettings("system")}>{t("检查输入设备")}</button><button type="button" className={ghostButtonClass} onClick={() => onSettings("engine")}>{t("检查语音服务")}</button></>}
      </div>
    </div>
  );
}

function TrialStatus({ active, processing, text }: { active: boolean; processing: boolean; text: string }) {
  return (
    <div className="mt-3 flex items-center gap-2.5 text-xs" aria-live="polite">
      {processing ? <Loader2 {...iconPropsLg} className="animate-spin text-secondary motion-reduce:animate-none" aria-hidden="true" /> : <Mic {...iconPropsLg} className={active ? "text-error-ink" : "text-secondary"} aria-hidden="true" />}
      <span className={active || processing ? "text-primary" : "text-secondary"}>{text}</span>
    </div>
  );
}
