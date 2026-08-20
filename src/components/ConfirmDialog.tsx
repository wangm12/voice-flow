import { useEffect, useRef } from "react";
import { buttonClass, colors, secondaryButtonClass } from "../lib/theme";

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel,
  danger = true,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  cancelLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialogRef.current?.focus();
    return () => {
      previousFocusRef.current?.focus();
    };
  }, [open]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/35 p-5">
      <section
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        aria-describedby="confirm-dialog-description"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          }
        }}
        className={`w-full max-w-md rounded-2xl border ${colors.border} ${colors.bg.card} p-5 outline-none focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent`}
      >
        <h2 id="confirm-dialog-title" className="text-base font-semibold text-primary">{title}</h2>
        <p id="confirm-dialog-description" className="mt-2 text-sm leading-6 text-secondary">{description}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" className={secondaryButtonClass} onClick={onCancel}>{cancelLabel}</button>
          <button
            type="button"
            className={danger
              ? "inline-flex h-9 items-center rounded-xl border border-error/30 bg-error/10 px-4 text-sm font-medium text-error transition-colors hover:bg-error/15 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
              : buttonClass}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}
