import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type NativePermissionGuide = {
  guide_id: string; permission: "accessibility" | "screen"; bundle_path: string | null;
  bundle_identifier: string | null; signature_status: "valid" | "unsigned" | "unknown";
  drag_available: boolean; permission_allowed: boolean; window_open: boolean; detail: string | null;
  placement: "adjacent_settings_window" | "independent_movable";
};
const unreleasedGuides = new Set<string>();

export function usePermissionGuide(open: boolean, permission: "accessibility" | "screen" = "accessibility") {
  const [status, setStatus] = useState<NativePermissionGuide | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    let live = true;
    const id = open ? crypto.randomUUID() : null;
    setStatus(null);
    const close = async (guideId: string) => {
      await invoke("close_permission_guide", { guideId });
      unreleasedGuides.delete(guideId);
    };
    const work = queue.current.catch(() => undefined).then(async () => {
      for (const oldId of [...unreleasedGuides]) await close(oldId);
      if (!live || !id) return;
      unreleasedGuides.add(id);
      const response = await invoke<NativePermissionGuide>("show_permission_guide", { permission, guideId: id });
      if (!response || response.guide_id !== id || response.permission !== permission || typeof response.permission_allowed !== "boolean" || typeof response.drag_available !== "boolean" || typeof response.window_open !== "boolean") throw new Error("guide_unconfirmed");
      if (!live) { await close(id); return; }
      setStatus(response); setFailed(false);
    });
    queue.current = work.catch(() => { if (live) setFailed(true); });
    return () => {
      live = false;
      if (!id) return;
      const release = queue.current.catch(() => undefined).then(() => close(id));
      queue.current = release.catch(() => { unreleasedGuides.add(id); if (mounted.current) setFailed(true); });
    };
  }, [open, permission, revision]);
  return { status, failed, reopen: () => setRevision((value) => value + 1) };
}
