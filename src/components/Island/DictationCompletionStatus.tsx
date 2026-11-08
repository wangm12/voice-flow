import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useI18n } from "../../lib/i18n";

export type DeliveryOutcomeKind =
  | "inserted"
  | "copy_only"
  | "paste_unverified"
  | "history_only"
  | "preview_only"
  | "failed"
  | "cancelled"
  | "timed_out"
  | "completed_without_clipboard";

export type DeliverySettlementSnapshot = {
  transaction_id: string | null;
  terminal_outcome: {
    kind: DeliveryOutcomeKind;
    phase: string;
    delivery_method: string;
    fallback_reason: string | null;
  } | null;
  writer_admission_sealed: boolean;
  admitted_writers: number;
  writers_closed: boolean;
  clipboard_restoration: "unverified";
  next_capture_readiness: "unverified";
};

export type DeliverySettlementRegistrySnapshot = {
  schema: "voiceflow_delivery_settlement_v1";
  process_identity: string | null;
  revision: string;
  revision_available: boolean;
  current: DeliverySettlementSnapshot | null;
  last_terminal: DeliverySettlementSnapshot | null;
};

const OUTCOMES = new Set<DeliveryOutcomeKind>([
  "inserted",
  "copy_only",
  "paste_unverified",
  "history_only",
  "preview_only",
  "failed",
  "cancelled",
  "timed_out",
  "completed_without_clipboard",
]);
const OPAQUE_HEX_ID = /^[0-9a-f]{32}$/;
const DECIMAL_REVISION = /^(?:0|[1-9][0-9]*)$/;

function isOwnerSnapshot(value: unknown): value is DeliverySettlementSnapshot {
  if (typeof value !== "object" || value === null) return false;
  const owner = value as Partial<DeliverySettlementSnapshot>;
  const outcome = owner.terminal_outcome;
  return (owner.transaction_id === null || typeof owner.transaction_id === "string")
    && (owner.transaction_id === null || OPAQUE_HEX_ID.test(owner.transaction_id))
    && (outcome === null || (typeof outcome === "object" && outcome !== null
      && OUTCOMES.has(outcome.kind)
      && typeof outcome.phase === "string"
      && typeof outcome.delivery_method === "string"
      && (outcome.fallback_reason === null || typeof outcome.fallback_reason === "string")))
    && typeof owner.writer_admission_sealed === "boolean"
    && Number.isSafeInteger(owner.admitted_writers)
    && Number(owner.admitted_writers) >= 0
    && typeof owner.writers_closed === "boolean"
    && owner.clipboard_restoration === "unverified"
    && owner.next_capture_readiness === "unverified";
}

export function isDeliverySettlementSnapshot(value: unknown): value is DeliverySettlementRegistrySnapshot {
  if (typeof value !== "object" || value === null) return false;
  const snapshot = value as Partial<DeliverySettlementRegistrySnapshot>;
  return snapshot.schema === "voiceflow_delivery_settlement_v1"
    && (snapshot.process_identity === null || (typeof snapshot.process_identity === "string"
      && OPAQUE_HEX_ID.test(snapshot.process_identity)))
    && typeof snapshot.revision === "string"
    && DECIMAL_REVISION.test(snapshot.revision)
    && typeof snapshot.revision_available === "boolean"
    && (snapshot.current === null || isOwnerSnapshot(snapshot.current))
    && (snapshot.last_terminal === null || isOwnerSnapshot(snapshot.last_terminal));
}

/** Compare decimal revision strings without converting them to imprecise JS numbers. */
export function compareSettlementRevisions(left: string, right: string): number {
  if (!DECIMAL_REVISION.test(left) || !DECIMAL_REVISION.test(right)) return 0;
  if (left.length !== right.length) return left.length < right.length ? -1 : 1;
  return left === right ? 0 : left < right ? -1 : 1;
}

/** Ignore delayed/duplicate publications; an exhausted revision freezes proof. */
export function acceptsSettlementSnapshot(
  current: DeliverySettlementRegistrySnapshot | null,
  incoming: DeliverySettlementRegistrySnapshot,
): boolean {
  if (!isDeliverySettlementSnapshot(incoming)) return false;
  if (!current) return true;
  if (incoming.process_identity !== current.process_identity) return false;
  if (!incoming.revision_available) return current.revision_available;
  if (!current.revision_available) return false;
  return compareSettlementRevisions(incoming.revision, current.revision) > 0;
}

export function dictationCompletionMessage(
  snapshot: DeliverySettlementRegistrySnapshot | null,
  translate: (source: string) => string,
): string {
  if (!snapshot) return "";
  if (!snapshot.revision_available || !snapshot.process_identity) {
    return translate("听写状态身份无法确认，请检查 VoiceFlow。");
  }
  const current = snapshot.current;
  if (current && !current.transaction_id) {
    return translate("听写状态身份无法确认，请检查 VoiceFlow。");
  }
  if (current && (!current.writer_admission_sealed || !current.writers_closed || !current.terminal_outcome)) {
    return translate("VoiceFlow 正在进行本次听写。");
  }
  const terminal = current ?? snapshot.last_terminal;
  if (!terminal) {
    return translate("尚无已完成的听写；下一次录音是否可用尚未确认。");
  }
  if (!terminal.transaction_id || !terminal.writer_admission_sealed || !terminal.writers_closed
    || !terminal.terminal_outcome) {
    return translate("听写状态身份无法确认，请检查 VoiceFlow。");
  }
  switch (terminal.terminal_outcome.kind) {
    case "inserted":
      return translate("听写已完成，文字已输入。剪贴板恢复情况和下一次录音是否可用均未确认。");
    case "copy_only":
      return translate("听写已完成，文字已复制，请手动粘贴。剪贴板恢复情况和下一次录音是否可用均未确认。");
    case "paste_unverified":
      return translate("听写已结束，但无法确认文字是否已输入。请检查目标输入框。剪贴板恢复情况和下一次录音是否可用均未确认。");
    case "history_only":
      return translate("听写已保存到历史记录，文字未确认输入。下一次录音是否可用未确认。");
    case "preview_only":
      return translate("听写预览已完成。下一次录音是否可用未确认。");
    case "failed":
      return translate("听写未完成，请检查 VoiceFlow。下一次录音是否可用未确认。");
    case "cancelled":
      return translate("听写已取消。剪贴板写入已结束，下一次录音是否可用未确认。");
    case "timed_out":
      return translate("听写已超时。文字交付未确认，下一次录音是否可用未确认。");
    case "completed_without_clipboard":
      return translate("听写流程已结束，但未进行剪贴板交付。下一次录音是否可用未确认。");
  }
}

export function DictationCompletionStatus() {
  const { language, t } = useI18n();
  const [snapshot, setSnapshot] = useState<DeliverySettlementRegistrySnapshot | null>(null);
  const latestSnapshot = useRef<DeliverySettlementRegistrySnapshot | null>(null);

  useEffect(() => {
    // Native status uses the exact resolved language used by this Island UI;
    // language changes update only its human label and value, not its evidence.
    void emit("dictation://settlement-language", language).catch(() => undefined);
  }, [language]);

  useEffect(() => {
    let active = true;
    let unlisten: UnlistenFn | undefined;

    function applySnapshot(value: unknown) {
      if (!isDeliverySettlementSnapshot(value) || !active) return;
      const current = latestSnapshot.current;
      if (!acceptsSettlementSnapshot(current, value)) return;
      latestSnapshot.current = value;
      setSnapshot(value);
    }

    void listen<unknown>("dictation://settlement", (event) => applySnapshot(event.payload))
      .then((stop) => {
        if (!active) {
          stop();
          return;
        }
        unlisten = stop;
        // Subscribe before reading so a terminal publication cannot land in a
        // gap. Revision comparison keeps a slower getter from replacing an event.
        return invoke<unknown>("get_delivery_settlement_snapshot").then(applySnapshot);
      })
      .catch(() => undefined);

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const message = dictationCompletionMessage(snapshot, t);
  return (
    <span
      id="voiceflow-dictation-settlement-status"
      className="voice-pill__sr-only"
      role="status"
      aria-label={t("VoiceFlow 听写状态")}
      aria-live="polite"
      aria-atomic="true"
      aria-relevant="text"
    >
      {message}
    </span>
  );
}
