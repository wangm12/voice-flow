import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { SaveSettings, Settings } from "../types/settings";

type SettingsField = keyof Settings;
type FieldVersions = Map<SettingsField, number>;

export function useSettingsPersistence({
  setSettings,
  formatError,
}: {
  setSettings: Dispatch<SetStateAction<Settings | null>>;
  formatError: (reason: unknown) => string;
}) {
  const [saveError, setSaveError] = useState<string | null>(null);
  const [unsavedFields, setUnsavedFields] = useState<string[]>([]);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const settingsRevision = useRef(0);
  const fieldRevision = useRef<FieldVersions>(new Map());
  const pendingRevision = useRef<FieldVersions>(new Map());
  const pendingPatch = useRef<Partial<Settings>>({});

  const publishUnsavedFields = useCallback(() => {
    setUnsavedFields([...pendingRevision.current.keys()].map(String));
  }, []);

  const versionsFor = useCallback((patch: Partial<Settings>) => {
    const versions: FieldVersions = new Map();
    for (const field of Object.keys(patch) as SettingsField[]) {
      const version = pendingRevision.current.get(field);
      if (version !== undefined) versions.set(field, version);
    }
    return versions;
  }, []);

  const commitPersistedPatch = useCallback((patch: Partial<Settings>, versions: FieldVersions) => {
    const confirmed: Partial<Settings> = {};
    for (const [field, version] of versions) {
      if (fieldRevision.current.get(field) !== version || !(field in patch)) continue;
      (confirmed as Record<string, unknown>)[field] = patch[field];
    }
    if (Object.keys(confirmed).length === 0) return;

    const revision = ++settingsRevision.current;
    for (const field of Object.keys(confirmed) as SettingsField[]) {
      fieldRevision.current.set(field, revision);
    }
    setSettings((current) => current ? { ...current, ...confirmed } : current);
  }, [setSettings]);

  const persistPatch = useCallback((patch: Partial<Settings>, versions: FieldVersions) => {
    if (Object.keys(patch).length === 0) return;
    saveQueue.current = saveQueue.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await invoke("update_settings_patch", { patch });
          commitPersistedPatch(patch, versions);
          for (const [field, version] of versions) {
            if (pendingRevision.current.get(field) === version) {
              pendingRevision.current.delete(field);
            }
          }
          publishUnsavedFields();
          if (pendingRevision.current.size === 0) setSaveError(null);
        } catch (reason) {
          // A rejected older write must never be retried over a newer value
          // that is already queued or in flight for the same field.
          for (const [field, version] of versions) {
            if (pendingRevision.current.get(field) === version && field in patch) {
              (pendingPatch.current as Record<string, unknown>)[field] = patch[field];
            }
          }
          publishUnsavedFields();
          setSaveError(formatError(reason));
        }
      });
  }, [commitPersistedPatch, formatError, publishUnsavedFields]);

  const flushPendingSave = useCallback(() => {
    if (saveTimer.current) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const patch = pendingPatch.current;
    pendingPatch.current = {};
    persistPatch(patch, versionsFor(patch));
    return saveQueue.current;
  }, [persistPatch, versionsFor]);

  const retryPendingSave = useCallback(() => {
    if (saveTimer.current) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const patch = pendingPatch.current;
    if (Object.keys(patch).length === 0) return;
    pendingPatch.current = {};
    persistPatch(patch, versionsFor(patch));
  }, [persistPatch, versionsFor]);

  const save = useCallback<SaveSettings>((patch, options) => {
    const revision = ++settingsRevision.current;
    for (const field of Object.keys(patch) as SettingsField[]) {
      fieldRevision.current.set(field, revision);
    }
    setSettings((current) => current ? { ...current, ...patch } : current);
    if (options?.persist === false) return;

    if (pendingRevision.current.size === 0) setSaveError(null);
    for (const field of Object.keys(patch) as SettingsField[]) {
      pendingRevision.current.set(field, revision);
    }
    publishUnsavedFields();
    pendingPatch.current = { ...pendingPatch.current, ...patch };
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      const patchToPersist = pendingPatch.current;
      pendingPatch.current = {};
      persistPatch(patchToPersist, versionsFor(patchToPersist));
    }, 300);
  }, [persistPatch, publishUnsavedFields, setSettings, versionsFor]);

  const captureSettingsRevision = useCallback(() => settingsRevision.current, []);
  const hasPendingSettings = useCallback(() => pendingRevision.current.size > 0, []);

  const applySettingsSnapshot = useCallback((snapshot: Settings, startedAtRevision: number) => {
    const accepted: Partial<Settings> = {};
    for (const field of Object.keys(snapshot) as SettingsField[]) {
      if (pendingRevision.current.has(field)) continue;
      if ((fieldRevision.current.get(field) ?? 0) > startedAtRevision) continue;
      (accepted as Record<string, unknown>)[field] = snapshot[field];
    }
    if (Object.keys(accepted).length === 0) return;
    const revision = ++settingsRevision.current;
    for (const field of Object.keys(accepted) as SettingsField[]) {
      fieldRevision.current.set(field, revision);
    }
    setSettings((current) => current ? { ...current, ...accepted } : snapshot);
  }, [setSettings]);

  const applySettingsEvent = useCallback((eventSettings: Settings) => {
    const accepted: Partial<Settings> = {};
    for (const field of Object.keys(eventSettings) as SettingsField[]) {
      if (pendingRevision.current.has(field)) continue;
      (accepted as Record<string, unknown>)[field] = eventSettings[field];
    }
    if (Object.keys(accepted).length === 0) return;
    const revision = ++settingsRevision.current;
    for (const field of Object.keys(accepted) as SettingsField[]) {
      fieldRevision.current.set(field, revision);
    }
    setSettings((current) => current ? { ...current, ...accepted } : eventSettings);
  }, [setSettings]);

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
    unsavedFields,
    captureSettingsRevision,
    hasPendingSettings,
    applySettingsSnapshot,
    applySettingsEvent,
  };
}
