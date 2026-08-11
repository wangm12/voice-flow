import type { ButtonHTMLAttributes } from "react";
import { radius } from "../lib/theme";

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
      className={`relative inline-flex h-7 w-12 shrink-0 items-center ${radius.pill} outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-base disabled:cursor-not-allowed disabled:opacity-50 ${className}`}
    >
      <span className={`absolute inset-0 ${radius.pill} transition-colors duration-150 ${checked ? "bg-success" : "bg-elevated"}`} aria-hidden="true" />
      <span className={`absolute left-1 h-5 w-5 rounded-full bg-zinc-300 transition-transform duration-150 ${checked ? "translate-x-5 bg-success-foreground shadow-sm" : ""}`} aria-hidden="true" />
    </button>
  );
}
