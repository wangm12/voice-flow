import type { ReactNode } from "react";

export type SegmentedOption<T extends string | number> = {
  value: T;
  label: ReactNode;
};

export function SegmentedControl<T extends string | number>({
  options,
  value,
  onChange,
  ariaLabel,
  className = "",
}: {
  options: readonly SegmentedOption<T>[];
  value: T | null;
  onChange: (value: T) => void;
  ariaLabel: string;
  className?: string;
}) {
  return (
    <div className={`vf-segmented-control ${className}`.trim()} role="radiogroup" aria-label={ariaLabel}>
      {options.map((option) => {
        const active = option.value === value;
        return (
          <button
            key={String(option.value)}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(option.value)}
            className={`vf-segmented-control__option ${active ? "vf-segmented-control__option--active" : ""}`.trim()}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
