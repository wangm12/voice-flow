import { useId } from "react";
import { Check } from "lucide-react";
import { ACTIVATION_MODES, recordingInstruction, type ActivationMode } from "../lib/activationCopy";
import { formatHotkeyDisplay } from "../lib/hotkeyFormat";
import { useI18n } from "../lib/i18n";

export function ActivationModeSelector({
  value,
  onChange,
  hotkey,
  disabled = false,
}: {
  value: ActivationMode | string;
  onChange: (mode: ActivationMode) => void;
  hotkey: string;
  disabled?: boolean;
}) {
  const { t } = useI18n();
  const descriptionId = useId();
  const name = useId();
  return (
    <div className="space-y-2" role="radiogroup" aria-label={t("录音方式")} aria-describedby={descriptionId}>
      <div className="vf-activation-options grid grid-cols-2 gap-1 rounded-lg bg-card p-1">
        {ACTIVATION_MODES.map((mode) => {
          const active = value === mode.id;
          return (
            <label
              key={mode.id}
              className={`vf-activation-option relative flex min-h-8 min-w-0 items-center justify-center gap-1.5 rounded-[6px] px-2 py-1 text-center transition-colors ${
                disabled ? "cursor-not-allowed text-disabled-foreground" : "cursor-pointer"
              } ${
                active ? "bg-elevated text-primary" : disabled ? "" : "text-secondary hover:bg-elevated active:bg-elevated"
              }`}
            >
              <input
                type="radio"
                name={name}
                value={mode.id}
                checked={active}
                disabled={disabled}
                onChange={() => onChange(mode.id)}
                className="sr-only"
              />
              <Check size={14} strokeWidth={2} aria-hidden="true" className={`shrink-0 ${active ? "" : "invisible"}`} />
              <span className="min-w-0">
                <span className="block text-sm font-medium leading-5">{t(mode.label)}</span>
              </span>
            </label>
          );
        })}
      </div>
      <p id={descriptionId} className="text-[13px] leading-5 text-secondary">
        {recordingInstruction(value, formatHotkeyDisplay(hotkey), t)}
        <span className="mt-1 block text-xs text-tertiary">{t("Esc 取消。")}</span>
      </p>
    </div>
  );
}
