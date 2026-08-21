import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE_SELECTOR =
  "a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex=\"-1\"])";

export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) => !element.hasAttribute("disabled") && element.getAttribute("aria-hidden") !== "true",
  );
}

function trapTabFocus(event: KeyboardEvent, container: HTMLElement) {
  if (event.key !== "Tab") return;

  const focusable = getFocusableElements(container);
  if (focusable.length === 0) return;

  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  const active = document.activeElement;
  const activeInDialog = active instanceof Node && container.contains(active);

  if (event.shiftKey) {
    if (!activeInDialog || active === first || active === container) {
      event.preventDefault();
      last.focus();
    }
    return;
  }

  if (!activeInDialog || active === last || active === container) {
    event.preventDefault();
    first.focus();
  }
}

type DialogBehaviorOptions = {
  open: boolean;
  dialogRef: RefObject<HTMLElement | null>;
  onCancel: () => void;
  restoreFocusRef?: RefObject<HTMLElement | null>;
  isolateBackground?: boolean;
};

export function useDialogBehavior({
  open,
  dialogRef,
  onCancel,
  restoreFocusRef,
  isolateBackground = false,
}: DialogBehaviorOptions) {
  const onCancelRef = useRef(onCancel);

  useEffect(() => {
    onCancelRef.current = onCancel;
  }, [onCancel]);

  useEffect(() => {
    if (!open) return;

    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (restoreFocusRef) {
      if (restoreFocusRef.current == null) {
        restoreFocusRef.current = previousFocus;
      }
    }

    const capturedRestore = restoreFocusRef?.current ?? previousFocus;

    requestAnimationFrame(() => dialogRef.current?.focus({ preventScroll: true }));

    let isolatedLayout: HTMLElement | null = null;
    let previousInert = false;
    if (isolateBackground) {
      const layout = document.querySelector("main")?.firstElementChild;
      if (layout instanceof HTMLElement) {
        isolatedLayout = layout;
        previousInert = layout.inert;
        layout.inert = true;
      }
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancelRef.current();
        return;
      }
      if (!dialogRef.current) return;
      trapTabFocus(event, dialogRef.current);
    };

    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      if (isolatedLayout) {
        isolatedLayout.inert = previousInert;
      }
      if (restoreFocusRef) {
        restoreFocusRef.current = null;
      }
      requestAnimationFrame(() => capturedRestore?.focus({ preventScroll: true }));
    };
  }, [open, dialogRef, restoreFocusRef, isolateBackground]);
}
