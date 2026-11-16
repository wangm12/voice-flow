import { useEffect, useState } from "react";
import { inputClass } from "../../lib/theme";

/** Numeric settings input that holds local draft text and clamps on commit. */
export function ClampedNumberInput({
  label,
  value,
  min,
  max,
  step = 1,
  decimals = 0,
  widthClassName = "w-[88px]",
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  decimals?: number;
  widthClassName?: string;
  onCommit: (next: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);

  const commit = () => {
    const parsed = Number(draft);
    if (!Number.isFinite(parsed)) {
      setDraft(String(value));
      return;
    }
    const factor = 10 ** decimals;
    const rounded = Math.round(parsed * factor) / factor;
    const clamped = Math.min(max, Math.max(min, rounded));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      aria-label={label}
      type="number"
      min={min}
      max={max}
      step={step}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
      className={`${inputClass} ${widthClassName} text-right`}
    />
  );
}
