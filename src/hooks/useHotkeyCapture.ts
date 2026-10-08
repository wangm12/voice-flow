import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { beginHotkeyCapture, type CaptureTarget } from "../lib/hotkeyCaptureSession";
import { hotkeyFromKeyboardEvent, isModifierKeyCode, isSafeCapturedHotkey, previewFromKeyboardEvent, sanitizeTauriHotkey } from "../lib/hotkeyFormat";

const identity = (source: string) => source;
export type CapturedHotkey = { hotkey: string };
type Phase = "idle" | "preparing" | "capturing" | "saving" | "cancelling";
type Session = ReturnType<typeof beginHotkeyCapture>;

function message(reason: unknown, translate: (source: string) => string): string {
  const text = reason instanceof Error ? reason.message : String(reason);
  if (text.includes("原快捷键恢复失败")) return translate("原快捷键未能恢复，请重新录制或重启 VoiceFlow。");
  if (text.includes("hotkey conflicts with another shortcut") || text.includes("快捷键冲突")) return translate("这个快捷键已用于其他功能，请换一个组合键。");
  if (text.includes("failed to register") || text.includes("register global")) return translate("这个快捷键无法使用，可能已被系统或其他 App 占用。请换一个组合键。");
  if (text.includes("录音或处理期间")) return translate("录音或处理期间不能修改快捷键和录音方式。");
  if (text.includes("已有快捷键正在编辑")) return translate("已有快捷键正在编辑，请先取消。");
  return translate("快捷键未能保存，原设置已保留。请重试。");
}

export function useHotkeyCapture({ onRecord, onCancel, onBusyChange, captureRef, translate = identity, captureTarget = "dictation" }: {
  onRecord: (result: CapturedHotkey) => void;
  onCancel?: () => void;
  onBusyChange?: (busy: boolean) => void;
  captureRef: RefObject<HTMLInputElement | null>;
  translate?: (source: string) => string;
  captureTarget?: CaptureTarget;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [previewHotkey, setPreviewHotkey] = useState<string | null>(null);
  const [captureHint, setCaptureHint] = useState<string | null>(null);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [lastOutcome, setLastOutcome] = useState<"saved" | "cancelled" | null>(null);
  const phaseRef = useRef<Phase>("idle");
  const fnAvailable = ["dictation", "verbatim_action", "translation_action"].includes(captureTarget);
  const sessionRef = useRef<Session | null>(null);
  const pendingCombo = useRef<string | null>(null);
  const lifecycleRef = useRef({ active: true });
  const callbacks = useRef({ onRecord, onCancel, onBusyChange });
  callbacks.current = { onRecord, onCancel, onBusyChange };

  const transition = useCallback((next: Phase) => {
    const wasBusy = phaseRef.current !== "idle";
    phaseRef.current = next;
    setPhase(next);
    if (wasBusy !== (next !== "idle")) callbacks.current.onBusyChange?.(next !== "idle");
  }, []);
  const resetKeys = useCallback(() => { pendingCombo.current = null; }, []);

  const finish = useCallback(async (hotkey: string | null) => {
    const session = sessionRef.current;
    if (!session || session.finishing) return;
    const lifecycle = lifecycleRef.current;
    transition(hotkey === null ? "cancelling" : "saving");
    resetKeys();
    try {
      const result = await session.finish(hotkey === null ? null : sanitizeTauriHotkey(hotkey));
      if (!lifecycle.active) return;
      setLastOutcome(result === null ? "cancelled" : "saved");
      if (result === null) callbacks.current.onCancel?.();
      else callbacks.current.onRecord({ hotkey: result });
    } catch (reason) {
      if (lifecycle.active) setCaptureError(message(reason, translate));
    } finally {
      if (sessionRef.current === session) sessionRef.current = null;
      if (lifecycle.active) { setPreviewHotkey(null); setCaptureHint(null); transition("idle"); }
    }
  }, [resetKeys, transition, translate]);
  const cancelRecording = useCallback(() => finish(null), [finish]);

  const startRecording = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    if (!lifecycle.active || phaseRef.current !== "idle") return;
    resetKeys(); setPreviewHotkey(null); setCaptureHint(null); setCaptureError(null); setLastOutcome(null);
    transition("preparing");
    try {
      const session = beginHotkeyCapture(captureTarget);
      sessionRef.current = session;
      const ready = await session.ready;
      if (!ready || !lifecycle.active || session.finishing) return;
      transition("capturing");
    } catch (reason) {
      if (lifecycle.active && !sessionRef.current?.finishing) {
        sessionRef.current = null;
        setCaptureError(message(reason, translate)); transition("idle");
      }
    }
  }, [captureTarget, resetKeys, transition, translate]);

  const commitPreset = useCallback((hotkey: string) => {
    if (phaseRef.current === "capturing") void finish(hotkey);
  }, [finish]);
  const clearBinding = useCallback(async () => {
    if (captureTarget === "dictation") return;
    if (phaseRef.current === "idle") await startRecording();
    if (phaseRef.current === "capturing") await finish("");
  }, [captureTarget, finish, startRecording]);

  useEffect(() => {
    if (phase === "idle") return;
    const keyDown = (event: KeyboardEvent) => {
      if (phaseRef.current === "saving" || phaseRef.current === "cancelling") return;
      const bare = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); void cancelRecording(); return; }
      if (phaseRef.current !== "capturing" || event.target !== captureRef.current || event.repeat || event.isComposing) return;
      if (event.key === "Tab" && !event.metaKey && !event.ctrlKey && !event.altKey) return;
      event.preventDefault(); event.stopPropagation();
      if (bare && ["Backspace", "Delete"].includes(event.key)) { void finish(captureTarget === "dictation" ? null : ""); return; }
      setPreviewHotkey(previewFromKeyboardEvent(event));
      if (isModifierKeyCode(event.code)) { setCaptureHint(translate(fnAvailable ? "请再按一个键组成快捷键；单键录音可选择 Fn。" : "请再按一个键组成快捷键。")); return; }
      const hotkey = hotkeyFromKeyboardEvent(event);
      if (hotkey && isSafeCapturedHotkey(hotkey)) {
        pendingCombo.current = hotkey; setCaptureHint(translate("松开按键即可保存。"));
      } else {
        pendingCombo.current = null;
        setCaptureHint(translate(fnAvailable ? "请使用 ⌘、⌥ 或 ⌃ 组成快捷键，或选择 Fn。" : "请使用 ⌘、⌥ 或 ⌃ 加上另一个键。"));
      }
    };
    const keyUp = () => {
      if (phaseRef.current !== "capturing") return;
      // WebKit can omit keyups under Command or retain modifier flags. Hand the
      // candidate to native code, which waits for every physical key before saving.
      if (pendingCombo.current) void finish(pendingCombo.current);
    };
    const blur = () => { resetKeys(); if (["preparing", "capturing"].includes(phaseRef.current)) void cancelRecording(); };
    const visibility = () => { if (document.hidden) blur(); };
    window.addEventListener("keydown", keyDown, true); window.addEventListener("keyup", keyUp, true);
    window.addEventListener("blur", blur); document.addEventListener("visibilitychange", visibility);
    return () => {
      window.removeEventListener("keydown", keyDown, true); window.removeEventListener("keyup", keyUp, true);
      window.removeEventListener("blur", blur); document.removeEventListener("visibilitychange", visibility);
    };
  }, [cancelRecording, captureRef, captureTarget, finish, fnAvailable, phase, resetKeys, translate]);

  useEffect(() => {
    const lifecycle = { active: true }; lifecycleRef.current = lifecycle;
    return () => {
      lifecycle.active = false; resetKeys(); callbacks.current.onBusyChange?.(false);
      const session = sessionRef.current;
      if (session && !session.finishing) void session.finish(null).catch(() => undefined);
    };
  }, [captureTarget, resetKeys]);

  return { phase, isRecording: phase === "capturing", isBusy: phase !== "idle", isFinishing: phase === "saving" || phase === "cancelling", previewHotkey, captureHint, captureError, lastOutcome, startRecording, cancelRecording, commitPreset, clearBinding };
}
