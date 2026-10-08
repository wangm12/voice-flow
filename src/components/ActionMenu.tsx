import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { MoreHorizontal } from "lucide-react";
import { focusRingClass } from "../lib/theme";

type MenuItem = { label: string; icon?: ReactNode; onSelect: () => void; disabled?: boolean; danger?: boolean };

/** Low-frequency row actions; the trigger stays in the tab order when the menu closes. */
export function ActionMenu({ label, items, disabled = false }: { label: string; items: MenuItem[]; disabled?: boolean }) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const firstFocus = useRef<"first" | "last">("first");
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });

  function close() {
    setOpen(false);
    trigger.current?.focus({ preventScroll: true });
  }

  useLayoutEffect(() => {
    if (!open) return;
    const anchor = trigger.current?.getBoundingClientRect();
    const panel = menu.current;
    if (!anchor || !panel) return;
    const height = panel.offsetHeight;
    setPosition({
      left: Math.max(8, Math.min(anchor.right - panel.offsetWidth, window.innerWidth - panel.offsetWidth - 8)),
      top: anchor.bottom + height + 4 > window.innerHeight - 8 ? Math.max(8, anchor.top - height - 4) : anchor.bottom + 4,
    });
    const enabled = panel.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
    enabled[firstFocus.current === "last" ? enabled.length - 1 : 0]?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !menu.current?.contains(event.target) && !trigger.current?.contains(event.target)) close();
    };
    const reposition = () => close();
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", reposition);
    // A fixed menu must not become detached from a scrolling row.
    window.addEventListener("scroll", reposition, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", reposition);
      window.removeEventListener("scroll", reposition, true);
    };
  }, [open]);

  return <>
    <button ref={trigger} type="button" disabled={disabled} aria-label={label} title={label} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
      className={`vf-icon-button inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-secondary enabled:hover:bg-elevated enabled:active:bg-border disabled:cursor-not-allowed ${focusRingClass}`}
      onClick={() => { if (open) close(); else { firstFocus.current = "first"; setOpen(true); } }}
      onKeyDown={(event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          firstFocus.current = event.key === "ArrowUp" ? "last" : "first";
          setOpen(true);
        }
      }}><MoreHorizontal size={16} aria-hidden="true" /></button>
    {open && createPortal(<div ref={menu} id={id} role="menu" aria-label={label} className="vf-settings vf-action-menu fixed z-[60] min-w-40 rounded-lg border border-border bg-card p-1 shadow-elevated" style={position}
      onKeyDown={(event) => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
        if (event.key === "Tab") { close(); return; }
        const buttons = Array.from(menu.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index - 1 + buttons.length) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
        if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      }}>
      {items.map((item) => <button key={item.label} type="button" role="menuitem" tabIndex={-1} disabled={item.disabled}
        className={`flex min-h-8 w-full items-center gap-2 rounded-md px-3 py-1 text-left text-[13px] leading-5 disabled:cursor-not-allowed disabled:text-disabled-foreground ${item.danger ? "text-error-ink enabled:hover:bg-error/5 focus:bg-error/5" : "text-primary enabled:hover:bg-elevated focus:bg-elevated"} ${focusRingClass}`}
        onClick={() => { close(); item.onSelect(); }}>{item.icon}{item.label}</button>)}
    </div>, document.body)}
  </>;
}
