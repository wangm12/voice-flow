import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ActivationMode } from "../lib/activationCopy";
import {
  hotkeyFromKeyboardEvent,
  isModifierKeyCode,
  modifierOnlyFromKeyboardEvent,
  previewFromKeyboardEvent,
  isModifierOnlyHotkey,
  sanitizeTauriHotkey,
} from "../lib/hotkeyFormat";

const DOUBLE_TAP_MS = 400;
const KEY_RELEASE_GRACE_MS = 400;
const identity = (source: string) => source;

export type CapturedHotkey = {
  hotkey: string;
  activationMode?: ActivationMode;
};

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

async function setHotkeysSuspended(
  suspended: boolean,
  captured?: CapturedHotkey | null,
  captureTarget: "dictation" | "selected_action" | "screen_action" = "dictation",
) {
  await invoke("set_hotkeys_suspended", {
    suspended,
    capturedHotkey: captured?.hotkey ?? null,
    capturedActivationMode: captured?.activationMode ?? null,
    captureTarget,
  });
}

type ModifierGesture = {
  modifier: string | null;
  downAt: number | null;
  tapCount: number;
  commitTimer: number | null;
  awaitingSecondTap: boolean;
};

const EMPTY_GESTURE: ModifierGesture = {
  modifier: null,
  downAt: null,
  tapCount: 0,
  commitTimer: null,
  awaitingSecondTap: false,
};

export function useHotkeyCapture({
  onRecord,
  onCancel,
  translate = identity,
  captureTarget = "dictation",
}: {
  onRecord: (result: CapturedHotkey) => void;
  onCancel?: () => void;
  translate?: (source: string) => string;
  captureTarget?: "dictation" | "selected_action" | "screen_action";
}) {
  const [isRecording, setIsRecording] = useState(false);
  const [recordedHotkey, setRecordedHotkey] = useState<string | null>(null);
  const [previewHotkey, setPreviewHotkey] = useState<string | null>(null);
  const [detectedGesture, setDetectedGesture] = useState<ActivationMode | "waiting_second_tap" | null>(null);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const pressedCodesRef = useRef(new Set<string>());
  const usedModifiersRef = useRef(new Set<string>());
  const sawNonModifierRef = useRef(false);
  const pendingComboRef = useRef<string | null>(null);
  const gestureRef = useRef<ModifierGesture>({ ...EMPTY_GESTURE });
  const finishingRef = useRef(false);
  const capturingRef = useRef(false);
  const callbacksRef = useRef({ onRecord, onCancel });
  callbacksRef.current = { onRecord, onCancel };

  const clearGestureTimer = useCallback(() => {
    if (gestureRef.current.commitTimer !== null) {
      window.clearTimeout(gestureRef.current.commitTimer);
      gestureRef.current.commitTimer = null;
    }
  }, []);

  const resetCaptureState = useCallback(() => {
    pressedCodesRef.current.clear();
    usedModifiersRef.current.clear();
    sawNonModifierRef.current = false;
    pendingComboRef.current = null;
    clearGestureTimer();
    gestureRef.current = { ...EMPTY_GESTURE };
    setDetectedGesture(null);
  }, [clearGestureTimer]);

  const finish = useCallback(
    async (hotkey: string, activationMode?: ActivationMode) => {
      if (finishingRef.current) return;
      finishingRef.current = true;
      setCaptureError(null);
      resetCaptureState();
      const normalizedHotkey = hotkey ? sanitizeTauriHotkey(hotkey) : "";
      const normalizedActivationMode =
        activationMode === "double_tap" && !isModifierOnlyHotkey(normalizedHotkey)
          ? "tap"
          : activationMode;
      setPreviewHotkey(normalizedHotkey || null);
      if (normalizedActivationMode) setDetectedGesture(normalizedActivationMode);
      await new Promise((resolve) => window.setTimeout(resolve, KEY_RELEASE_GRACE_MS));
      const captured = normalizedHotkey
        ? { hotkey: normalizedHotkey, activationMode: normalizedActivationMode }
        : null;
      try {
        await setHotkeysSuspended(false, captured, captureTarget);
        capturingRef.current = false;
        setIsRecording(false);
        setRecordedHotkey(normalizedHotkey || null);
        setPreviewHotkey(null);
        setDetectedGesture(null);
        if (normalizedHotkey) callbacksRef.current.onRecord({ hotkey: normalizedHotkey, activationMode: normalizedActivationMode });
      } catch (reason) {
        capturingRef.current = false;
        setIsRecording(false);
        setRecordedHotkey(null);
        setPreviewHotkey(null);
        setDetectedGesture(null);
        setCaptureError(`${translate("快捷键恢复失败：")}${errorMessage(reason)}`);
      } finally {
        finishingRef.current = false;
      }
    },
    [captureTarget, resetCaptureState, translate],
  );

  const cancelRecording = useCallback(async () => {
    if (finishingRef.current) return;
    finishingRef.current = true;
    setCaptureError(null);
    resetCaptureState();
    setPreviewHotkey(null);
    await new Promise((resolve) => window.setTimeout(resolve, KEY_RELEASE_GRACE_MS));
    try {
      await setHotkeysSuspended(false, null, captureTarget);
      callbacksRef.current.onCancel?.();
    } catch (reason) {
      setCaptureError(`${translate("快捷键恢复失败：")}${errorMessage(reason)}`);
    } finally {
      capturingRef.current = false;
      setIsRecording(false);
      setRecordedHotkey(null);
      finishingRef.current = false;
    }
  }, [captureTarget, resetCaptureState, translate]);

  const startRecording = useCallback(async () => {
    if (finishingRef.current || isRecording) return;
    setCaptureError(null);
    resetCaptureState();
    setRecordedHotkey(null);
    setPreviewHotkey(null);
    try {
      await setHotkeysSuspended(true, null, captureTarget);
      capturingRef.current = true;
      setIsRecording(true);
    } catch (reason) {
      capturingRef.current = false;
      setIsRecording(false);
      setCaptureError(`${translate("无法开始快捷键录制：")}${errorMessage(reason)}`);
    }
  }, [captureTarget, isRecording, resetCaptureState, translate]);

  const commitPreset = useCallback(
    (hotkey: string, activationMode?: ActivationMode) => {
      if (!capturingRef.current || finishingRef.current) return;
      void finish(hotkey, activationMode);
    },
    [finish],
  );

  const tryCommitWhenAllReleased = useCallback(() => {
    if (finishingRef.current || pressedCodesRef.current.size > 0) return;

    if (sawNonModifierRef.current) {
      const combo = pendingComboRef.current;
       if (combo) void finish(combo, "tap");
      return;
    }

    if (usedModifiersRef.current.size !== 1) return;

    const [modifierOnly] = usedModifiersRef.current;
    if (!modifierOnly) return;

    const gesture = gestureRef.current;
    gesture.downAt = null;

    if (gesture.tapCount >= 2) {
      void finish(modifierOnly, "double_tap");
      return;
    }

    gesture.tapCount = 1;
    gesture.modifier = modifierOnly;
    gesture.awaitingSecondTap = true;
    setDetectedGesture("waiting_second_tap");
    gesture.commitTimer = window.setTimeout(() => {
      gesture.commitTimer = null;
      gesture.awaitingSecondTap = false;
      gesture.tapCount = 0;
      setDetectedGesture(null);
    }, DOUBLE_TAP_MS);
  }, [finish]);

  useEffect(() => {
    if (!isRecording) return undefined;

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.repeat || finishingRef.current) return;

      event.stopPropagation();

      if (event.key === "Escape") {
        event.preventDefault();
        void cancelRecording();
        return;
      }

      if (event.key === "Backspace" || event.key === "Delete") {
        if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
          void finish("");
          return;
        }
      }

      const preview = previewFromKeyboardEvent(event);
      if (preview) setPreviewHotkey(preview);

      pressedCodesRef.current.add(event.code);

      if (isModifierKeyCode(event.code)) {
        const modifierOnly = modifierOnlyFromKeyboardEvent(event);
        if (!modifierOnly) return;
        usedModifiersRef.current.add(modifierOnly);

        const gesture = gestureRef.current;
        const now = Date.now();

        if (
          gesture.awaitingSecondTap &&
          gesture.modifier === modifierOnly &&
          gesture.commitTimer !== null
        ) {
          clearGestureTimer();
          gesture.awaitingSecondTap = false;
          void finish(modifierOnly, "double_tap");
          return;
        } else if (!gesture.downAt) {
          gesture.modifier = modifierOnly;
          gesture.downAt = now;
        }
        return;
      }

      sawNonModifierRef.current = true;
      clearGestureTimer();
      gestureRef.current = { ...EMPTY_GESTURE };
      setDetectedGesture(null);
      const combo = hotkeyFromKeyboardEvent(event);
      if (combo) pendingComboRef.current = combo;
    };

    const onKeyUp = (event: KeyboardEvent) => {
      if (event.repeat || finishingRef.current) return;

      event.stopPropagation();

      pressedCodesRef.current.delete(event.code);
      tryCommitWhenAllReleased();
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      clearGestureTimer();
    };
  }, [cancelRecording, clearGestureTimer, finish, isRecording, tryCommitWhenAllReleased]);

  useEffect(() => {
    return () => {
      if (capturingRef.current) {
        void setHotkeysSuspended(false, null, captureTarget);
        capturingRef.current = false;
      }
    };
  }, [captureTarget]);

  return {
    isRecording,
    recordedHotkey,
    previewHotkey,
    detectedGesture,
    captureError,
    startRecording,
    cancelRecording,
    commitPreset,
  };
}
