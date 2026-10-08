import { useState } from "react";
import { HotkeyRecorder } from "../HotkeyRecorder";
import { ActivationModeSelector } from "../ActivationModeSelector";
import type { ActivationMode } from "../../lib/activationCopy";
import { FnHotkeyNote } from "../FnHotkeyNote";
import { formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { useI18n } from "../../lib/i18n";
import { TryItStep } from "./TryItStep";

export function HotkeyStep({
  trial,
  hotkey,
  activationMode,
  selectedActionHotkey,
  recording,
  processing,
  error,
  onHotkeyChange,
  onSelectedActionHotkeyChange,
  onActivationModeChange,
  modeSaving = false,
}: {
  trial: "dictation" | "selected_action";
  hotkey: string;
  activationMode: ActivationMode | string;
  selectedActionHotkey: string;
  recording: boolean;
  processing: boolean;
  error: string | null;
  onHotkeyChange: (hotkey: string, options?: { persist?: boolean }) => void;
  onActivationModeChange: (mode: ActivationMode) => void;
  modeSaving?: boolean;
  onSelectedActionHotkeyChange: (hotkey: string, options?: { persist?: boolean }) => void;
}) {
  const { t } = useI18n();
  const [captureBusy, setCaptureBusy] = useState(false);
  const isDictation = trial === "dictation";
  const currentHotkey = isDictation ? hotkey : selectedActionHotkey;
  const currentHotkeyDisplay = formatHotkeyDisplay(currentHotkey);

  return (
    <div>
      <h1 className="text-[28px] font-semibold leading-tight tracking-tight">
        {isDictation ? t("设置语音输入快捷键") : t("设置选中文本操作快捷键")}
      </h1>
      <p className="mt-2 text-[13px] leading-6 text-secondary">
        {isDictation
          ? t("用它在任何 App 中开始或结束录音。设置好后，马上在下面试用。")
          : t("选中文字后，用它让 VoiceFlow 自动翻译、缩短或改写。设置好后，马上在下面试用。")}
      </p>

      <div className="mt-6 border-y border-border py-4">
        <p className="text-sm font-medium text-primary">
          {isDictation ? t("语音输入快捷键") : t("选中文本操作快捷键")}
        </p>
        <p className="mt-1 text-xs text-tertiary">
          {isDictation ? t("在 Cursor、浏览器、邮件等 App 中都能使用。") : t("这是独立快捷键，不会替换语音输入快捷键。")}
        </p>
        <div className="mt-3">
          <HotkeyRecorder
            disabled={recording || processing || modeSaving}
            onCaptureBusyChange={setCaptureBusy}
            value={currentHotkey}
            captureTarget={trial}
            onChange={(value, options) => {
              if (isDictation) {
                onHotkeyChange(value, options);
              } else {
                onSelectedActionHotkeyChange(value, options);
              }
            }}
          />
        </div>
        {isDictation && (
          <div className="mt-4">
            <ActivationModeSelector
              value={activationMode}
              hotkey={currentHotkey}
              disabled={recording || processing || modeSaving || captureBusy}
              onChange={onActivationModeChange}
            />
            <FnHotkeyNote hotkey={currentHotkey} />
          </div>
        )}
      </div>

      <TryItStep
        trial={trial}
        recording={recording}
        processing={processing}
        hotkeyDisplay={isDictation ? currentHotkeyDisplay : formatHotkeyDisplay(hotkey)}
        selectedActionHotkeyDisplay={isDictation ? formatHotkeyDisplay(selectedActionHotkey) : currentHotkeyDisplay}
        activationMode={activationMode}
        error={error}
      />
    </div>
  );
}
