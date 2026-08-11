import { ACTIVATION_MODES, type ActivationMode } from "../lib/activationCopy";
import { radius } from "../lib/theme";
import { useI18n } from "../lib/i18n";

export function ActivationModeSelector({
  value,
  onChange,
}: {
  value: ActivationMode | string;
  onChange: (mode: ActivationMode) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="space-y-2">
      {ACTIVATION_MODES.map((mode) => {
        const active = value === mode.id;
        return (
          <label
            key={mode.id}
            className={`flex cursor-pointer gap-3 ${radius.control} border px-4 py-3 transition-colors ${
              active ? "border-accent bg-elevated" : "border-border bg-card hover:border-border/80"
            }`}
          >
            <input
              type="radio"
              name="activation_mode"
              value={mode.id}
              checked={active}
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
    </div>
  );
}
