import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export const onDemandGuideIds = ["dictionary_learning", "context_screen", "selected_translation", "scene_preferences"] as const;
export type OnDemandGuideId = typeof onDemandGuideIds[number];
export type GuideAcknowledgement = { guide_id: OnDemandGuideId; current_version: number; acknowledged_version: number | null };
const contentVersion = 1;

const isVersion = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value > 0 && value <= 0xffffffff;

/** Normalize the native acknowledgement receipt once, without inventing a local completion store. */
export function parseGuideAcknowledgements(value: unknown): GuideAcknowledgement[] | null {
  if (!Array.isArray(value) || value.length !== onDemandGuideIds.length) return null;
  const seen = new Set<OnDemandGuideId>();
  const rows: GuideAcknowledgement[] = [];
  for (const candidate of value as unknown[]) {
    if (!candidate || typeof candidate !== "object") return null;
    const row = candidate as Record<string, unknown>;
    const guideId = onDemandGuideIds.find((id) => id === row.guide_id);
    if (!guideId || seen.has(guideId)
      || !isVersion(row.current_version)
      || !(row.acknowledged_version === null || isVersion(row.acknowledged_version))) return null;
    seen.add(guideId);
    rows.push({ guide_id: guideId, current_version: row.current_version, acknowledged_version: row.acknowledged_version });
  }
  return rows;
}

export function useGuideAcknowledgement(guideId: OnDemandGuideId) {
  const [receipt, setReceipt] = useState<GuideAcknowledgement | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const request = useRef(0);
  const busy = useRef(false);
  useEffect(() => {
    const generation = ++request.current;
    setReceipt(null);
    setError(null);
    busy.current = false;
    setPending(false);
    void (async () => {
      const value = await invoke<unknown>("get_guide_acknowledgements");
      if (request.current !== generation) return;
      const row = parseGuideAcknowledgements(value)?.find((item) => item.guide_id === guideId);
      if (!row) throw new Error("invalid_guide_receipt");
      setReceipt(row);
    })().catch(() => {
      if (request.current === generation) setError("教学阅读状态未能读取；仍可重看说明，点击“知道了”重试保存。");
    });
    return () => { request.current++; };
  }, [guideId]);

  const acknowledge = useCallback(async (): Promise<boolean> => {
    if (busy.current) return false;
    busy.current = true;
    setPending(true);
    setError(null);
    const generation = ++request.current;
    try {
      // If the initial read failed, first obtain the version from its native owner.
      const current = receipt?.guide_id === guideId ? receipt : parseGuideAcknowledgements(await invoke<unknown>("get_guide_acknowledgements"))?.find((item) => item.guide_id === guideId);
      if (request.current !== generation) return false;
      if (!current || current.current_version !== contentVersion) throw new Error("unsupported_guide_version");
      const result = parseGuideAcknowledgements(await invoke<unknown>("acknowledge_guide", { guideId, version: current.current_version }))?.find((item) => item.guide_id === guideId);
      if (!result || result.current_version !== current.current_version || result.acknowledged_version !== current.current_version) throw new Error("unconfirmed_guide_acknowledgement");
      if (request.current !== generation) return false;
      setReceipt(result);
      return true;
    } catch {
      if (request.current === generation) setError("教学阅读状态未保存，请重试；设置和功能授权没有改变。");
      return false;
    } finally {
      if (request.current === generation) { busy.current = false; setPending(false); }
    }
  }, [guideId, receipt]);

  return { acknowledge, pending, error, needsAcknowledgement: receipt?.guide_id === guideId ? receipt.acknowledged_version !== receipt.current_version : null };
}
