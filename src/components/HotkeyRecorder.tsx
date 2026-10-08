import { useEffect, useRef, useState } from "react";
import { Pencil, X } from "lucide-react";
import { useHotkeyCapture } from "../hooks/useHotkeyCapture";
import { DEFAULT_DICTATION_HOTKEY, formatHotkeyDisplay, hotkeyDisplayParts, isFnOnlyHotkey, isModifierOnlyHotkey } from "../lib/hotkeyFormat";
import { compactButtonClass, focusRingClass, ghostButtonClass, inputClass } from "../lib/theme";
import { useI18n } from "../lib/i18n";
import { IconButton } from "./IconButton";

export type HotkeyChangeOptions = { persist?: boolean };
const targetLabels = {
  dictation: "听写快捷键", selected_action: "选中文本快捷键", screen_action: "看屏幕快捷键",
  verbatim_action: "原文录音快捷键", translation_action: "翻译快捷键",
};

export function HotkeyRecorder({ value, onChange, disabled = false, captureTarget = "dictation", onCaptureBusyChange, onCaptureErrorChange, compact = false }: {
  value: string;
  onChange: (hotkey: string, options?: HotkeyChangeOptions) => void;
  disabled?: boolean;
  captureTarget?: keyof typeof targetLabels;
  onCaptureBusyChange?: (busy: boolean) => void;
  onCaptureErrorChange?: (error: string | null) => void;
  compact?: boolean;
}) {
  const { t } = useI18n();
  const group = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const capture = useRef<HTMLInputElement>(null);
  const returnFocus = useRef(false);
  const wasBusy = useRef(false);
  const errorCallback = useRef(onCaptureErrorChange);
  errorCallback.current = onCaptureErrorChange;
  const [showSaved, setShowSaved] = useState(false);
  const recorder = useHotkeyCapture({
    translate: t, captureTarget, captureRef: capture, onBusyChange: onCaptureBusyChange,
    onRecord: ({ hotkey }) => { if (hotkey || captureTarget !== "dictation") onChange(hotkey, { persist: false }); },
  });
  const recordingTarget = ["dictation", "verbatim_action", "translation_action"].includes(captureTarget);
  const label = t(targetLabels[captureTarget]);
  const displayed = value ? formatHotkeyDisplay(value) : t("未设置");
  const parts = hotkeyDisplayParts(value);
  const begin = () => {
    if (disabled || recorder.isBusy) return;
    returnFocus.current = true;
    setShowSaved(false);
    void recorder.startRecording();
  };
  const cancel = (focus = true) => { returnFocus.current = focus; void recorder.cancelRecording(); };

  useEffect(() => { errorCallback.current?.(recorder.captureError); }, [recorder.captureError]);
  useEffect(() => {
    if (["preparing", "capturing"].includes(recorder.phase) && (!wasBusy.current || recorder.phase === "preparing")) capture.current?.focus({ preventScroll: true });
    if (wasBusy.current && !recorder.isBusy && returnFocus.current && document.hasFocus()) trigger.current?.focus({ preventScroll: true });
    wasBusy.current = recorder.isBusy;
  }, [recorder.isBusy, recorder.phase]);
  useEffect(() => {
    if (recorder.lastOutcome !== "saved") return;
    setShowSaved(true);
    const timer = window.setTimeout(() => setShowSaved(false), 1600);
    return () => window.clearTimeout(timer);
  }, [recorder.lastOutcome]);
  useEffect(() => {
    if (!recorder.isBusy) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !group.current?.contains(event.target)) {
        returnFocus.current = false;
        if (["preparing", "capturing"].includes(recorder.phase)) void recorder.cancelRecording();
      }
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [recorder.cancelRecording, recorder.isBusy, recorder.phase]);

  const status = recorder.phase === "preparing" ? t("准备录入…")
    : recorder.phase === "saving" ? t("正在保存快捷键…")
    : recorder.phase === "cancelling" ? t("正在取消…")
    : recorder.captureHint ?? t("按下新的组合快捷键；松开后保存。");

  return <div ref={group} role="group" aria-label={t("快捷键录制")} data-capture-busy={recorder.isBusy || undefined} className="w-full"
    onBlur={(event) => {
      // WebKit does not focus buttons on pointer clicks, so their blur target can be null.
      // Outside pointer and window-blur listeners cover leaving the app without a target.
      if (!recorder.isBusy || !(event.relatedTarget instanceof Node) || group.current?.contains(event.relatedTarget)) return;
      if (["preparing", "capturing"].includes(recorder.phase)) cancel(false);
      else if (event.relatedTarget instanceof Node) returnFocus.current = false;
    }}>
    {!recorder.isBusy ? <div className={`flex items-center gap-1.5 ${compact ? "justify-end" : "justify-start"}`}>
      <button ref={trigger} type="button" disabled={disabled} onClick={begin} aria-label={`${t("更改")}${label}：${displayed}`}
        title={t("点击更改快捷键")}
        className={`vf-hotkey-trigger group inline-flex min-h-8 max-w-full items-center justify-center gap-2 rounded-full bg-elevated px-3 text-[13px] text-primary transition-colors enabled:hover:bg-accent-soft disabled:cursor-not-allowed disabled:text-disabled-foreground ${focusRingClass}`}>
        <span className="flex flex-wrap items-center gap-1">{parts.length ? parts.map((part, index) => <kbd key={`${part}-${index}`} className="font-sans text-[13px] font-normal">{part}</kbd>) : displayed}</span>
        <Pencil size={12} strokeWidth={1.5} aria-hidden="true" className="shrink-0 text-tertiary opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100" />
      </button>
      {captureTarget !== "dictation" && value && <IconButton size="sm" tooltip={false} label={`${t("清除")}${label}`} icon={<X size={14} />} disabled={disabled}
        onClick={() => { returnFocus.current = true; void recorder.clearBinding(); }} />}
    </div> : <div className="space-y-2">
      <p className="text-xs text-tertiary">{t("当前快捷键：")}{displayed}</p>
      <input ref={capture} type="text" readOnly aria-label={t("录入新的快捷键")} aria-busy={recorder.isFinishing || recorder.phase === "preparing"}
        value={recorder.previewHotkey ? formatHotkeyDisplay(recorder.previewHotkey) : ""}
        placeholder={recorder.phase === "capturing" ? t("按下组合快捷键") : status}
        className={`${inputClass} w-full focus:outline focus:outline-2 focus:outline-focus focus:outline-offset-0`} />
      <p role="status" aria-live="polite" className="text-xs leading-5 text-secondary">{status}</p>
      <div className="flex flex-wrap items-center gap-1">
        {recordingTarget && <button type="button" disabled={recorder.phase !== "capturing"} className={compactButtonClass} onClick={() => recorder.commitPreset("Fn")}>{t("使用 Fn")}</button>}
        {captureTarget === "dictation" && value !== DEFAULT_DICTATION_HOTKEY && <button type="button" disabled={recorder.phase !== "capturing"} className={ghostButtonClass} onClick={() => recorder.commitPreset(DEFAULT_DICTATION_HOTKEY)}>{t("恢复默认快捷键")}</button>}
        <button type="button" disabled={recorder.isFinishing} className={ghostButtonClass} onClick={() => cancel()}>{t("取消")}</button>
        {captureTarget !== "dictation" && <button type="button" disabled={recorder.phase !== "capturing"} className={ghostButtonClass} onClick={() => { returnFocus.current = true; recorder.commitPreset(""); }}>{t("清除")}</button>}
        {!recorder.isFinishing && <span className="text-xs text-tertiary">{t("Esc 取消")}</span>}
      </div>
    </div>}
    {showSaved && !recorder.isBusy && <p role="status" className="mt-1 text-xs text-secondary">{t("已保存")}</p>}
    {!recorder.isBusy && isModifierOnlyHotkey(value) && !(isFnOnlyHotkey(value) && recordingTarget) && <p role="alert" className="mt-2 text-xs text-error-ink">{t("旧的单独修饰键已暂停。请选择 Fn 或组合快捷键。")}</p>}
    {recorder.captureError && <div role="alert" className="mt-2 text-xs leading-5 text-error-ink">{recorder.captureError}<button type="button" disabled={disabled || recorder.isBusy} className={`ml-1 ${ghostButtonClass}`} onClick={begin}>{t("重试")}</button></div>}
  </div>;
}
