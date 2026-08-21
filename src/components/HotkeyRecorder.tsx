import { useEffect, useRef } from "react";
import { Keyboard } from "lucide-react";
import { useHotkeyCapture } from "../hooks/useHotkeyCapture";
import { ACTIVATION_MODES, type ActivationMode } from "../lib/activationCopy";
import { hotkeyDisplayParts, isModifierOnlyHotkey, toTanStackHotkey } from "../lib/hotkeyFormat";
import { radius, focusRingClass } from "../lib/theme";
import { iconPropsSm } from "../lib/icons";
import { useI18n } from "../lib/i18n";

function gestureHint(gesture: ActivationMode | "waiting_second_tap" | null, hotkey: string, t: (source: string) => string): string | null {
  if (!isModifierOnlyHotkey(hotkey) && (gesture === "double_tap" || gesture === "waiting_second_tap")) {
    return t("已识别：按一下切换");
  }
  switch (gesture) {
    case "waiting_second_tap":
      return t("请双击该键确认「双击开始」…");
    case "tap":
      return t("已识别：按一下切换");
    case "double_tap":
      return t("已识别：双击开始");
    default:
      return null;
  }
}

export type HotkeyChangeOptions = { persist?: boolean };

export function HotkeyRecorder({
  value,
  onChange,
  disabled = false,
  captureTarget = "dictation",
}: {
  value: string;
  onChange: (hotkey: string, activationMode?: ActivationMode, options?: HotkeyChangeOptions) => void;
  disabled?: boolean;
  captureTarget?: "dictation" | "selected_action";
}) {
  const { t } = useI18n();
  const surfaceRef = useRef<HTMLButtonElement>(null);
  const startingRef = useRef(false);
  const recorder = useHotkeyCapture({
    translate: t,
    captureTarget,
    onRecord: ({ hotkey, activationMode }) => {
      // Persist so the settings page reflects (and keeps) the new hotkey. The
      // backend already saved it during the unsuspend handshake, so this is an
      // idempotent re-save that also refreshes our local React state.
      if (hotkey) onChange(hotkey, activationMode, { persist: true });
    },
  });

  useEffect(() => {
    if (!recorder.isRecording) return;
    surfaceRef.current?.focus({ preventScroll: true });
  }, [recorder.isRecording]);

  const previewSource = recorder.isRecording ? recorder.previewHotkey ?? recorder.recordedHotkey ?? value : value;
  const preview = hotkeyDisplayParts(previewSource);
  const hint = gestureHint(recorder.detectedGesture, previewSource, t);

  const beginRecording = () => {
    if (disabled || recorder.isRecording || startingRef.current) return;
    startingRef.current = true;
    void recorder.startRecording().finally(() => {
      startingRef.current = false;
    });
  };

  return (
    <>
      <div role="group" aria-label={t("快捷键录制")} className="w-full">
      <button
        type="button"
        ref={surfaceRef}
        disabled={disabled}
        aria-pressed={recorder.isRecording}
        onPointerDown={(event) => {
          if (disabled || recorder.isRecording || event.button !== 0) return;
          event.preventDefault();
          surfaceRef.current?.focus({ preventScroll: true });
          beginRecording();
        }}
        onKeyDown={(event) => {
          if (disabled || recorder.isRecording) return;
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            beginRecording();
          }
        }}
        className={`flex w-full cursor-pointer items-center gap-3 ${radius.control} border px-4 py-3 text-left text-sm transition-colors focus-visible:border-accent ${focusRingClass} ${
          recorder.isRecording ? "border-accent bg-elevated" : "border-border bg-elevated hover:border-accent/60"
        } ${disabled ? "cursor-not-allowed opacity-50" : ""}`}
      >
      <Keyboard {...iconPropsSm} className="shrink-0 -translate-y-px text-tertiary" aria-hidden="true" />
      <span className="min-w-0 flex-1">
        {recorder.isRecording ? (
          <span className="text-secondary">
            {preview.length > 0 ? (
              <span className="flex flex-wrap items-center gap-1.5">
                {preview.map((part, index) => (
                  <kbd
                    key={`${part}-${index}`}
                    className={`inline-flex min-w-[1.75rem] items-center justify-center ${radius.control} border border-border bg-card px-2 py-1 text-xs font-medium text-primary`}
                  >
                    {part}
                  </kbd>
                ))}
                <span className="text-xs text-tertiary">
                  {hint ?? (preview.length > 1 ? t("再按主键完成组合") : t("组合键按一下，或双击功能键"))}
                </span>
              </span>
            ) : (
              <span className="flex flex-col gap-2">
                <span>{t("组合键按一下，或双击功能键（Esc 取消）")}</span>
              </span>
            )}
          </span>
        ) : preview.length > 0 ? (
          <span className="flex flex-wrap items-center gap-1.5">
            {preview.map((part, index) => (
              <kbd
                key={`${part}-${index}`}
                className={`inline-flex min-w-[1.75rem] items-center justify-center ${radius.control} border border-border bg-card px-2 py-1 text-xs font-medium text-primary`}
              >
                {part}
              </kbd>
            ))}
          </span>
        ) : (
          <span className="text-tertiary">{t("点击后按快捷键，会自动识别你的按法")}</span>
        )}
      </span>
      {!recorder.isRecording && (
        <span className="shrink-0 text-xs text-tertiary">
          {preview.length > 0 ? t("点击重新设置") : toTanStackHotkey(value)}
        </span>
      )}
      </button>
      {recorder.isRecording && preview.length === 0 && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs text-tertiary">
          <span>{t("Mac 的 fn 键应用内检测不到，请点选：")}</span>
          <button
            type="button"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => recorder.commitPreset("Fn", "double_tap")}
            className={`inline-flex min-w-[1.75rem] items-center justify-center ${radius.control} border border-border bg-card px-2 py-1 text-xs font-medium text-primary hover:border-accent/60`}
          >
            fn
          </button>
        </div>
      )}
      </div>
      {recorder.captureError && <p role="alert" className="mt-2 text-xs text-error">{recorder.captureError}</p>}
    </>
  );
}

export function activationModeLabel(mode: ActivationMode | string, translate: (source: string) => string = (source) => source): string {
  const label = ACTIVATION_MODES.find((item) => item.id === mode)?.label;
  return label ? translate(label) : String(mode);
}
