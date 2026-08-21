import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { SaveSettings, Settings } from "../types/settings";

export function useSettingsPersistence({
  setSettings,
  formatError,
}: {
  setSettings: Dispatch<SetStateAction<Settings | null>>;
  formatError: (reason: unknown) => string;
}) {
  const [saveError, setSaveError] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const saveRevision = useRef(0);
  const pendingPatch = useRef<Partial<Settings>>({});

  const persistPatch = useCallback((patch: Partial<Settings>, revision: number) => {
    if (Object.keys(patch).length === 0) return;
    saveQueue.current = saveQueue.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await invoke("update_settings_patch", { patch });
          if (saveRevision.current === revision && Object.keys(pendingPatch.current).length === 0) {
            setSaveError(null);
          }
        } catch (reason) {
          pendingPatch.current = { ...patch, ...pendingPatch.current };
          setSaveError(formatError(reason));
        }
      });
  }, [formatError]);

  const flushPendingSave = useCallback(() => {
    if (saveTimer.current) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const patch = pendingPatch.current;
    pendingPatch.current = {};
    persistPatch(patch, saveRevision.current);
    return saveQueue.current;
  }, [persistPatch]);

  const retryPendingSave = useCallback(() => {
    const patch = pendingPatch.current;
    if (Object.keys(patch).length === 0) return;
    pendingPatch.current = {};
    saveRevision.current += 1;
    persistPatch(patch, saveRevision.current);
  }, [persistPatch]);

  const save = useCallback<SaveSettings>((patch, options) => {
    setSettings((current) => current ? { ...current, ...patch } : current);
    if (options?.persist === false) return;
    setSaveError(null);
    saveRevision.current += 1;
    const revision = saveRevision.current;
    pendingPatch.current = { ...pendingPatch.current, ...patch };
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      const patchToPersist = pendingPatch.current;
      pendingPatch.current = {};
      persistPatch(patchToPersist, revision);
    }, 300);
  }, [persistPatch, setSettings]);

  useEffect(() => {
    const flushWhenHidden = () => {
      if (document.visibilityState === "hidden") void flushPendingSave();
    };
    window.addEventListener("pagehide", flushWhenHidden);
    document.addEventListener("visibilitychange", flushWhenHidden);
    return () => {
      window.removeEventListener("pagehide", flushWhenHidden);
      document.removeEventListener("visibilitychange", flushWhenHidden);
      void flushPendingSave();
    };
  }, [flushPendingSave]);

  return {
    save,
    saveError,
    setSaveError,
    flushPendingSave,
    retryPendingSave,
  };
}
