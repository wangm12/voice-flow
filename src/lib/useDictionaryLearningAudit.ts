import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { parseDictionaryLearningAudit, parseLearningAuditChanged, parseLearningAuditReadError, type DictionaryLearningAudit, type LearningAuditChanged, type LearningAuditReadError } from "./dictionaryLearningAudit";

export type LearningAuditProblem = LearningAuditReadError | { code: "unsupported_receipt" | "invalid_invalidation" | "subscription_unavailable" | "outdated_receipt" | "read_unavailable" };
type View = { snapshot: DictionaryLearningAudit | null; pending: boolean; stale: boolean; problem: LearningAuditProblem | null };

/** One mounted read session, one in-flight read and at most one trailing read per burst. */
export function useDictionaryLearningAudit() {
  const [view, setView] = useState<View>({ snapshot: null, pending: true, stale: true, problem: null });
  const refreshRef = useRef<() => void>(() => undefined);
  const refresh = useCallback(() => refreshRef.current(), []);
  useEffect(() => {
    let active = true;
    let listening = false;
    let reading = false;
    let trailing = false;
    let unknownInvalidation = false;
    let latestEvent: LearningAuditChanged | null = null;
    let shown: DictionaryLearningAudit | null = null;
    const retiredRuntimes: string[] = [];
    let unlisten: (() => void) | null = null;

    const load = async (allowTrailing: boolean, manual: boolean) => {
      if (!active) return;
      if (reading) { trailing = true; return; }
      if (manual) { latestEvent = null; unknownInvalidation = false; }
      reading = true;
      trailing = false;
      setView((previous) => ({ ...previous, pending: true, stale: true, problem: null }));
      try {
        const result = parseDictionaryLearningAudit(await invoke<unknown>("get_dictionary_learning_audit"));
        if (!active) return;
        if (!result) { setView((previous) => ({ ...previous, stale: true, problem: { code: "unsupported_receipt" } })); return; }
        const obsoleteEvent = latestEvent && (result.runtime_id !== latestEvent.runtime_id || BigInt(result.audit_revision) < BigInt(latestEvent.audit_revision));
        const obsoleteRead = shown && result.runtime_id === shown.runtime_id
          && (BigInt(result.audit_revision) < BigInt(shown.audit_revision) || BigInt(result.clear_generation) < BigInt(shown.clear_generation));
        if (obsoleteEvent || obsoleteRead || retiredRuntimes.includes(result.runtime_id)) {
          setView((previous) => ({ ...previous, stale: true, problem: { code: "outdated_receipt" } }));
          return;
        }
        if (shown && shown.runtime_id !== result.runtime_id) {
          retiredRuntimes.push(shown.runtime_id);
          if (retiredRuntimes.length > 8) retiredRuntimes.shift();
        }
        shown = result;
        latestEvent = null;
        if (!unknownInvalidation) trailing = false; // This read already covers received invalidations.
        setView({ snapshot: result, pending: true,
          stale: unknownInvalidation || !listening || result.coverage.projection_consistency === "changed_during_read",
          problem: unknownInvalidation ? { code: "invalid_invalidation" } : !listening ? { code: "subscription_unavailable" } : null });
      } catch (error) {
        if (active) setView((previous) => ({ ...previous, stale: true, problem: parseLearningAuditReadError(error) ?? { code: "read_unavailable" } }));
      } finally {
        reading = false;
        if (active) {
          setView((previous) => ({ ...previous, pending: false }));
          if (trailing && allowTrailing) { trailing = false; void load(false, false); }
        }
      }
    };

    refreshRef.current = () => { void load(true, true); };
    void (async () => {
      try {
        const release = await listen<unknown>("dictionary_learning_audit://changed", (event) => {
          if (!active) return;
          const changed = parseLearningAuditChanged(event.payload);
          if (!changed) {
            unknownInvalidation = true;
            setView((previous) => ({ ...previous, stale: true, problem: { code: "invalid_invalidation" } }));
            return;
          }
          if (retiredRuntimes.includes(changed.runtime_id)) return;
          if (shown?.runtime_id === changed.runtime_id && BigInt(changed.audit_revision) <= BigInt(shown.audit_revision)) return;
          if (latestEvent?.runtime_id === changed.runtime_id && BigInt(changed.audit_revision) <= BigInt(latestEvent.audit_revision)) return;
          latestEvent = changed;
          setView((previous) => ({ ...previous, stale: true }));
          if (reading) trailing = true;
          else void load(true, false);
        });
        if (!active) { release(); return; }
        unlisten = release;
        listening = true;
      } catch {
        if (!active) return;
      }
      if (active) void load(true, false);
    })();

    return () => {
      active = false;
      refreshRef.current = () => undefined;
      unlisten?.();
    };
  }, []);
  return { ...view, refresh };
}
