import type { ButtonHTMLAttributes } from "react";
import { focusRingClass, radius } from "../lib/theme";

type ToggleProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "onChange" | "type"> & {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
};

/** A consistent binary setting control with native switch semantics. */
export function Toggle({ checked, onChange, label, className = "", ...props }: ToggleProps) {
  return (
    <button
      {...props}
      type="button"
      role="switch"
      aria-label={label}
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={`group relative inline-flex h-7 w-12 shrink-0 items-center ${radius.pill} outline-none transition-colors duration-150 motion-reduce:transition-none ${focusRingClass} disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
    >
      <span className={`absolute inset-x-0 inset-y-0.5 ${radius.pill} transition-colors duration-150 motion-reduce:transition-none ${checked ? "bg-toggle-checked group-[:enabled:hover]:bg-toggle-checked-hover group-[:enabled:active]:bg-toggle-checked-pressed" : "bg-elevated group-[:enabled:hover]:bg-accent-soft group-[:enabled:active]:bg-border"}`} aria-hidden="true" />
      <span className={`absolute left-1 h-5 w-5 rounded-full transition-transform duration-150 motion-reduce:transition-none ${checked ? "translate-x-5 bg-toggle-thumb shadow-sm" : "bg-toggle-idle-thumb"}`} aria-hidden="true" />
    </button>
  );
}
