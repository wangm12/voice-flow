import { useRef } from "react";
import { createPortal } from "react-dom";
import { useDialogBehavior } from "../lib/focusTrap";
import { buttonClass, colors, focusRingClass, secondaryButtonClass } from "../lib/theme";

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
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useDialogBehavior({
    open,
    dialogRef,
    onCancel,
    restoreFocusRef,
    isolateBackground: open,
  });

  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-5">
      <div role="presentation" className="absolute inset-0 bg-black/35" onClick={onCancel} />
      <section
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        aria-describedby="confirm-dialog-description"
        onClick={(event) => event.stopPropagation()}
        className={`relative w-full max-w-md rounded-2xl border ${colors.border} ${colors.bg.card} p-5 outline-none ${focusRingClass}`}
      >
        <h2 id="confirm-dialog-title" className="text-base font-semibold text-primary">{title}</h2>
        <p id="confirm-dialog-description" className="mt-2 text-sm leading-6 text-secondary">{description}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" className={secondaryButtonClass} onClick={onCancel}>{cancelLabel}</button>
          <button
            type="button"
            className={danger
              ? `inline-flex h-9 items-center rounded-xl border border-error/30 bg-error/10 px-4 text-sm font-medium text-error transition-colors hover:bg-error/15 ${focusRingClass}`
              : buttonClass}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
