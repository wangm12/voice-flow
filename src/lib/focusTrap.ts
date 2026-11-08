import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE_SELECTOR =
  "a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), summary, [tabindex]:not([tabindex=\"-1\"])";

export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) => canReceiveFocus(element),
  );
}

function canReceiveFocus(element: HTMLElement): boolean {
  if (!element.isConnected || element.hasAttribute("disabled") || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
  let ancestor = element.parentElement;
  while (ancestor) {
    if (ancestor.tagName === "DETAILS" && !ancestor.hasAttribute("open") && !ancestor.querySelector(":scope > summary")?.contains(element)) return false;
    ancestor = ancestor.parentElement;
  }
  const style = getComputedStyle(element);
  return style.display !== "none" && style.visibility !== "hidden";
}

type ModalEntry = { panel: HTMLElement; layer: HTMLElement; isolate: boolean };
const modals: ModalEntry[] = [];
const isolatedNodes = new Map<HTMLElement, { inert: boolean; ariaHidden: string | null; zIndex: string }>();

function setIsolated(node: HTMLElement, isolated: boolean) {
  if (!isolatedNodes.has(node)) isolatedNodes.set(node, { inert: node.inert === true, ariaHidden: node.getAttribute("aria-hidden"), zIndex: node.style.zIndex });
  const original = isolatedNodes.get(node)!;
  node.inert = isolated || original.inert;
  if (isolated) node.setAttribute("aria-hidden", "true");
  else if (original.ariaHidden === null) node.removeAttribute("aria-hidden");
  else node.setAttribute("aria-hidden", original.ariaHidden);
}

function syncModalIsolation() {
  if (modals.length === 0) {
    for (const [node, original] of isolatedNodes) {
      node.inert = original.inert;
      node.style.zIndex = original.zIndex;
      if (original.ariaHidden === null) node.removeAttribute("aria-hidden");
      else node.setAttribute("aria-hidden", original.ariaHidden);
    }
    isolatedNodes.clear();
    return;
  }
  const top = modals[modals.length - 1];
  const layout = document.querySelector("main")?.firstElementChild;
  if (layout instanceof HTMLElement && !layout.contains(top.panel)) setIsolated(layout, modals.some((entry) => entry.isolate));
  for (const [index, entry] of modals.entries()) {
    setIsolated(entry.layer, entry !== top);
    entry.layer.style.zIndex = String(50 + index);
  }
}

function deletionFallbacks(opener: HTMLElement | null): HTMLElement[] {
  const row = opener?.closest<HTMLElement>("[data-focus-row]");
  if (!row?.parentElement) return [];
  const rows = Array.from(row.parentElement.querySelectorAll<HTMLElement>(":scope > [data-focus-row]"));
  const index = rows.indexOf(row);
  return [...rows.slice(index + 1), ...rows.slice(0, index).reverse()].flatMap(getFocusableElements);
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
    const fallbackTargets = deletionFallbacks(capturedRestore);
    const panel = dialogRef.current;
    if (!panel) return;
    const entry = { panel, layer: panel.parentElement ?? panel, isolate: isolateBackground };
    modals.push(entry);
    syncModalIsolation();
    requestAnimationFrame(() => {
      if (modals[modals.length - 1] === entry && !panel.contains(document.activeElement)) panel.focus({ preventScroll: true });
    });

    const onKeyDown = (event: KeyboardEvent) => {
      if (modals[modals.length - 1] !== entry || event.defaultPrevented) return;
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
      const wasTop = modals[modals.length - 1] === entry;
      const index = modals.indexOf(entry);
      if (index >= 0) modals.splice(index, 1);
      syncModalIsolation();
      if (restoreFocusRef) {
        restoreFocusRef.current = null;
      }
      const nextTop = modals[modals.length - 1];
      if (wasTop) requestAnimationFrame(() => {
        const top = modals[modals.length - 1];
        if (top !== nextTop || (top && top.panel.contains(document.activeElement))) return;
        const target = [capturedRestore, ...fallbackTargets].find((candidate) => candidate && canReceiveFocus(candidate));
        if (top) {
          (target && top.panel.contains(target) ? target : top.panel).focus({ preventScroll: true });
        } else {
          (target ?? document.querySelector<HTMLElement>('[data-delete-focus-fallback]') ?? document.querySelector<HTMLElement>('main h1, main input, main button'))?.focus({ preventScroll: true });
        }
      });
    };
  }, [open, dialogRef, restoreFocusRef, isolateBackground]);
}
