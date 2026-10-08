import { useRef, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useDialogBehavior } from "../lib/focusTrap";
import { buttonClass, colors, dangerActionButtonClass, focusRingClass, secondaryButtonClass } from "../lib/theme";

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel,
  danger = true,
  busy = false,
  error,
  returnFocusRef,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  cancelLabel: string;
  danger?: boolean;
  busy?: boolean;
  error?: string | null;
  returnFocusRef?: RefObject<HTMLElement | null>;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useDialogBehavior({
    open,
    dialogRef,
    onCancel: () => { if (!busy) onCancel(); },
    restoreFocusRef: returnFocusRef ?? restoreFocusRef,
    isolateBackground: open,
  });

  if (!open) return null;

  return createPortal(
    <div className="vf-settings fixed inset-0 z-50 flex items-center justify-center p-6">
      <div role="presentation" className="absolute inset-0 bg-black/35" onClick={() => { if (!busy) onCancel(); }} />
      <section
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        aria-describedby="confirm-dialog-description"
        onClick={(event) => event.stopPropagation()}
        className={`relative flex max-h-[calc(100dvh-3rem)] min-h-0 w-full max-w-md flex-col rounded-2xl border ${colors.border} ${colors.bg.card} p-6 shadow-elevated outline-none ${focusRingClass}`}
      >
        <h2 id="confirm-dialog-title" className="shrink-0 text-lg font-semibold leading-6 text-primary">{title}</h2>
        <p id="confirm-dialog-description" className="mt-2 min-h-0 overflow-y-auto overscroll-contain break-words text-[13px] leading-6 text-secondary">{description}</p>
        {error && <p role="alert" className="mt-4 max-h-24 shrink-0 overflow-y-auto break-words text-[13px] leading-5 text-error-ink">{error}</p>}
        <div className="mt-6 flex shrink-0 flex-wrap justify-end gap-2">
          <button type="button" disabled={busy} className={secondaryButtonClass} onClick={onCancel}>{cancelLabel}</button>
          <button
            type="button"
            disabled={busy}
            aria-busy={busy}
            className={danger ? dangerActionButtonClass : buttonClass}
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
