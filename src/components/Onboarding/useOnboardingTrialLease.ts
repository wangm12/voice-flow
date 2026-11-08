import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTrialLease, type TrialLease } from "../../lib/onboardingReadiness";

// Failed releases remain owned across a closed/reopened guide. Native matching-ID
// release is idempotent and cannot cancel a newer lease.
const abandonedLeases = new Set<string>();

export function useOnboardingTrialLease(key: string | null, onGeneration: (generation: string) => void) {
  const [acquired, setAcquired] = useState<{ key: string; lease: TrialLease } | null>(null);
  const [error, setError] = useState(false);
  const owned = useRef(new Set<string>());
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const active = useRef(false);
  const currentKey = useRef(key);
  currentKey.current = key;
  const generationCallback = useRef(onGeneration);
  generationCallback.current = onGeneration;

  const enqueue = useCallback(<T,>(operation: () => Promise<T>): Promise<T> => {
    const request = queue.current.catch(() => undefined).then(operation);
    queue.current = request.catch(() => undefined);
    return request;
  }, []);

  const release = useCallback(async (id: string) => {
    const response = await invoke<unknown>("set_onboarding_test_mode", { enabled: false, trialId: id });
    if (!isTrialLease(response) || response.trial_id !== id || response.enabled) throw new Error("trial_release_unconfirmed");
    owned.current.delete(id);
    abandonedLeases.delete(id);
  }, []);

  const releaseAll = useCallback(() => {
    setAcquired(null);
    return enqueue(async () => {
      try {
        for (const id of new Set([...abandonedLeases, ...owned.current])) await release(id);
        if (active.current) setError(false);
      } catch (reason) {
        if (active.current) setError(true);
        throw reason;
      }
    });
  }, [enqueue, release]);

  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  useEffect(() => {
    let live = true;
    setAcquired(null);
    void enqueue(async () => {
      for (const id of new Set([...abandonedLeases, ...owned.current])) await release(id);
      if (!live || !key) return;
      const id = crypto.randomUUID();
      owned.current.add(id); // Invocation failure may still have acquired natively.
      const response = await invoke<unknown>("set_onboarding_test_mode", { enabled: true, trialId: id });
      if (!isTrialLease(response) || response.trial_id !== id || !response.enabled) throw new Error("trial_acquisition_unconfirmed");
      if (!live || currentKey.current !== key) { await release(id); return; }
      generationCallback.current(response.config_generation);
      setAcquired({ key, lease: response });
      setError(false);
    }).catch(() => { if (live && active.current) setError(true); });
    return () => {
      live = false;
      void enqueue(async () => {
        for (const id of [...owned.current]) {
          try { await release(id); }
          catch { abandonedLeases.add(id); if (active.current) setError(true); }
        }
      });
    };
  }, [key, enqueue, release]);

  return { lease: acquired?.key === key ? acquired.lease : null, releaseAll, releaseFailed: error };
}
