import { useEffect, useRef, type UIEvent } from "react";

export function useSettingsScrollbar() {
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(hideTimer.current), []);

  return (event: UIEvent<HTMLElement>) => {
    const pane = event.currentTarget;
    pane.style.setProperty("--settings-scrollbar-thumb", "var(--color-tertiary)");
    clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => pane.style.removeProperty("--settings-scrollbar-thumb"), 700);
  };
}
