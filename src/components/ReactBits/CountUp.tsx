import { useEffect, useState } from "react";

type CountUpProps = {
  value: number;
  duration?: number;
  decimals?: number;
  suffix?: string;
};

function formatValue(value: number, decimals: number) {
  return value.toFixed(decimals);
}

/** A reduced-motion-aware number transition for small, meaningful metric changes. */
export function CountUp({ value, duration = 420, decimals = 0, suffix = "" }: CountUpProps) {
  const [displayValue, setDisplayValue] = useState(value);

  useEffect(() => {
    const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduceMotion || duration <= 0) {
      setDisplayValue(value);
      return;
    }

    let frame = 0;
    const startedAt = performance.now();
    const initialValue = displayValue;
    const tick = (now: number) => {
      const progress = Math.min(1, (now - startedAt) / duration);
      const eased = 1 - Math.pow(1 - progress, 3);
      setDisplayValue(initialValue + (value - initialValue) * eased);
      if (progress < 1) frame = window.requestAnimationFrame(tick);
    };
    frame = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frame);
  // The current display value is intentionally captured when a new metric arrives.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, duration]);

  return <span className="tabular-nums">{formatValue(displayValue, decimals)}{suffix}</span>;
}
