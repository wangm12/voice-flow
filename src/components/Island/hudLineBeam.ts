import { useSyncExternalStore } from "react";

function subscribePageVisibility(onChange: () => void): () => void {
  if (typeof document === "undefined") return () => undefined;
  document.addEventListener("visibilitychange", onChange);
  return () => document.removeEventListener("visibilitychange", onChange);
}

function pageVisibleSnapshot(): boolean {
  return typeof document === "undefined" || document.visibilityState !== "hidden";
}

/** Shared visibility signal for stopping decorative motion while the page is backgrounded. */
export function usePageVisible(): boolean {
  return useSyncExternalStore(subscribePageVisibility, pageVisibleSnapshot, () => true);
}
