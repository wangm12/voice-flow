import { ACTIVATION_MODES, type ActivationMode } from "../lib/activationCopy";
import { radius } from "../lib/theme";
import { useI18n } from "../lib/i18n";

export function ActivationModeSelector({
  value,
  onChange,
  modifierOnly = false,
}: {
  value: ActivationMode | string;
  onChange: (mode: ActivationMode) => void;
  modifierOnly?: boolean;
}) {
  const { t } = useI18n();
  const allowed: ActivationMode = modifierOnly ? "double_tap" : "tap";
  return (
    <div className="space-y-2" role="radiogroup" aria-describedby={modifierOnly ? "activation-mode-modifier-hint" : undefined}>
      {ACTIVATION_MODES.map((mode) => {
        const active = (modifierOnly ? "double_tap" : value) === mode.id;
        const disabled = mode.id !== allowed;
        return (
          <label
            key={mode.id}
            className={`flex gap-3 ${radius.control} border px-4 py-3 transition-colors ${
              disabled ? "cursor-not-allowed opacity-60" : "cursor-pointer"
            } ${
              active ? "border-accent bg-elevated" : "border-border bg-card hover:border-border/80"
            }`}
          >
            <input
              type="radio"
              name="activation_mode"
              value={mode.id}
              checked={active}
              disabled={disabled}
              onChange={() => onChange(mode.id)}
              className="mt-0.5 accent-[var(--color-accent)]"
            />
            <span className="min-w-0">
              <span className="block text-sm font-medium text-primary">{t(mode.label)}</span>
              <span className="mt-0.5 block text-xs text-secondary">{t(mode.description)}</span>
            </span>
          </label>
        );
      })}
      {modifierOnly && (
        <p id="activation-mode-modifier-hint" className="px-1 text-xs leading-5 text-tertiary">
          {t("功能键只能使用双击开始和结束。")}
        </p>
      )}
    </div>
  );
}
