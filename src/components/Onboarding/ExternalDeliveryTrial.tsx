import { Select } from "../Select";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../../lib/i18n";
import { isNativeGeneration, type OnboardingPermissions, type TrialLease } from "../../lib/onboardingReadiness";
import { ghostButtonClass, secondaryButtonClass } from "../../lib/theme";
import { useOnboardingTrialLease } from "./useOnboardingTrialLease";

export type ExternalTargetApp = "text_edit" | "chrome";
type ExternalHistorySelection = { dictation_id: number; revision_id: number };
type ExternalSourceIdentity = ExternalHistorySelection & { kind: "history_revision"; text_sha256: string };
type Fixture = { lease: TrialLease; target_app: ExternalTargetApp; before: string; selected: string; inserted: string; after: string; selection_start_utf16: number; preparation_ms: number; switch_ms: number; source_identity?: ExternalSourceIdentity };
type HistoryChoice = { id: number; created_at: string; final_text: string; raw_text: string };
type HistoryRevisionChoice = { dictation_id: number; revision_id: number; created_at: string; final_text: string; cleanup_status: string | null; revision_reason: string; intent: { operation?: string; source?: string } | null };
type HistoryPage = { items: HistoryChoice[]; has_more: boolean };
type NativeCaptureStage = "not_started" | "application_identity" | "focused_element" | "window_identity" | "frontmost_recheck" | "complete" | "unsupported_platform";
type NativeCaptureReason = "application_unavailable" | "application_identity_unknown" | "selected_application_mismatch" | "accessibility_observation_unknown" | "focused_element_unavailable" | "focused_role_unknown" | "focused_input_not_editable" | "secure_input" | "window_identity_unknown" | "window_identity_ambiguous" | "identity_drift" | "unsupported_platform";
type DeliveryStage = "not_entered" | "entered";
export type ExternalReceipt = { lease: TrialLease; recording_generation: string; trigger_source: "app_command"; trial_current: boolean;
  permissions: [boolean, boolean, boolean]; target: null | { pid: number; bundle_id: "com.apple.TextEdit" | "com.google.Chrome"; window_id: string };
  readback: "exact_once" | "unknown"; clipboard_restoration: "not_needed" | "restored" | "ownership_changed" | "unknown";
  capture_stage: NativeCaptureStage; capture_reason: NativeCaptureReason | null; delivery_stage: DeliveryStage;
  reason: string | null; source: "product_same_field_ax_readback_not_independent_oracle"; source_identity?: ExternalSourceIdentity };
const captureStages: readonly NativeCaptureStage[] = ["not_started", "application_identity", "focused_element", "window_identity", "frontmost_recheck", "complete", "unsupported_platform"];
const captureReasons: readonly NativeCaptureReason[] = ["application_unavailable", "application_identity_unknown", "selected_application_mismatch", "accessibility_observation_unknown", "focused_element_unavailable", "focused_role_unknown", "focused_input_not_editable", "secure_input", "window_identity_unknown", "window_identity_ambiguous", "identity_drift", "unsupported_platform"];
const captureStageLabels: Record<NativeCaptureStage, string> = {
  not_started: "尚未开始原生焦点检查", application_identity: "检查前台应用身份", focused_element: "检查焦点控件", window_identity: "检查窗口身份",
  frontmost_recheck: "复核前台应用身份", complete: "原生焦点检查已完成", unsupported_platform: "当前平台不支持原生焦点检查",
};
const captureReasonLabels: Record<NativeCaptureReason, string> = {
  application_unavailable: "前台应用不可用", application_identity_unknown: "前台应用身份无法确认", selected_application_mismatch: "前台应用与所选目标不符",
  accessibility_observation_unknown: "辅助功能观察状态无法确认", focused_element_unavailable: "焦点控件无法读取", focused_role_unknown: "焦点控件类型无法确认",
  focused_input_not_editable: "焦点控件不可编辑", secure_input: "检测到安全输入框", window_identity_unknown: "窗口身份无法确认",
  window_identity_ambiguous: "有多个窗口符合条件", identity_drift: "复核时应用或焦点身份发生变化", unsupported_platform: "当前平台不支持此检查",
};
const sameLease = (a: TrialLease | null, b: TrialLease | null) => !!a && !!b && a.trial_id === b.trial_id && a.config_generation === b.config_generation;
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
function isMember<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && values.includes(value as T);
}
function parseSourceIdentity(value: unknown): ExternalSourceIdentity | null {
  if (!isRecord(value) || value.kind !== "history_revision"
    || !Number.isSafeInteger(value.dictation_id) || (value.dictation_id as number) <= 0
    || !Number.isSafeInteger(value.revision_id) || (value.revision_id as number) <= 0
    || typeof value.text_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(value.text_sha256)) return null;
  return {
    kind: "history_revision",
    dictation_id: value.dictation_id as number,
    revision_id: value.revision_id as number,
    text_sha256: value.text_sha256,
  };
}
function sameSourceIdentity(a: ExternalSourceIdentity | null, b: ExternalSourceIdentity | null): boolean {
  return a === null ? b === null : !!b && a.kind === b.kind && a.dictation_id === b.dictation_id
    && a.revision_id === b.revision_id && a.text_sha256 === b.text_sha256;
}
function parseFixture(value: unknown, lease: TrialLease, targetApp: ExternalTargetApp, expectedSource: ExternalHistorySelection | null): Fixture | null {
  if (!value || typeof value !== "object") return null;
  const x = value as Fixture;
  const sourceIdentity = x.source_identity === undefined ? null : parseSourceIdentity(x.source_identity);
  const sourceMatches = expectedSource === null
    ? sourceIdentity === null
    : !!sourceIdentity && sourceIdentity.dictation_id === expectedSource.dictation_id && sourceIdentity.revision_id === expectedSource.revision_id;
  if (!sourceMatches || (x.source_identity !== undefined && sourceIdentity === null)) return null;
  return sameLease(x.lease, lease) && x.target_app === targetApp
    && [x.before, x.selected, x.inserted, x.after].every((text) => typeof text === "string" && text.length > 0 && text.length <= 512)
    && Number.isSafeInteger(x.selection_start_utf16) && x.selection_start_utf16 >= 0
    && x.before.slice(x.selection_start_utf16, x.selection_start_utf16 + x.selected.length) === x.selected
    && x.before.slice(0, x.selection_start_utf16) + x.inserted + x.before.slice(x.selection_start_utf16 + x.selected.length) === x.after
    && x.preparation_ms === 180_000 && x.switch_ms === 5_000
    ? { ...x, source_identity: sourceIdentity ?? undefined }
    : null;
}
export function parseExternalReceipt(
  value: unknown,
  lease: TrialLease,
  targetApp: ExternalTargetApp,
  expectedSource: ExternalSourceIdentity | null = null,
): ExternalReceipt | null {
  if (!isRecord(value) || !isRecord(value.lease)) return null;
  const rawLease = value.lease;
  if (typeof rawLease.trial_id !== "string" || typeof rawLease.config_generation !== "string") return null;
  const parsedLease = { trial_id: rawLease.trial_id, config_generation: rawLease.config_generation };
  if (!sameLease(parsedLease, lease)) return null;

  const expectedBundle: "com.apple.TextEdit" | "com.google.Chrome" =
    targetApp === "text_edit" ? "com.apple.TextEdit" : "com.google.Chrome";
  const rawTarget = value.target;
  let target: ExternalReceipt["target"] = null;
  if (rawTarget !== null) {
    if (!isRecord(rawTarget) || !Number.isSafeInteger(rawTarget.pid) || (rawTarget.pid as number) <= 0
      || rawTarget.bundle_id !== expectedBundle || !isNativeGeneration(rawTarget.window_id)) return null;
    target = { pid: rawTarget.pid as number, bundle_id: expectedBundle, window_id: rawTarget.window_id };
  }

  const sourceIdentity = value.source_identity === undefined ? null : parseSourceIdentity(value.source_identity);
  if ((value.source_identity !== undefined && sourceIdentity === null) || !sameSourceIdentity(expectedSource, sourceIdentity)) return null;

  const rawPermissions = value.permissions;
  if (!Array.isArray(rawPermissions) || rawPermissions.length !== 3 || !rawPermissions.every((bit) => typeof bit === "boolean")) return null;
  const permissions: [boolean, boolean, boolean] = [rawPermissions[0] as boolean, rawPermissions[1] as boolean, rawPermissions[2] as boolean];
  if (!isNativeGeneration(value.recording_generation) || value.trigger_source !== "app_command"
    || typeof value.trial_current !== "boolean"
    || !isMember(captureStages, value.capture_stage)
    || !(value.capture_reason === null || isMember(captureReasons, value.capture_reason))
    || !isMember(["not_entered", "entered"] as const, value.delivery_stage)
    || !isMember(["exact_once", "unknown"] as const, value.readback)
    || !isMember(["not_needed", "restored", "ownership_changed", "unknown"] as const, value.clipboard_restoration)
    || !(value.reason === null || (typeof value.reason === "string" && value.reason.length <= 80))
    || value.source !== "product_same_field_ax_readback_not_independent_oracle") return null;

  const captureStage = value.capture_stage;
  const captureReason = value.capture_reason;
  const deliveryStage = value.delivery_stage;
  const readback = value.readback;
  const clipboardRestoration = value.clipboard_restoration;
  const reason = value.reason;
  if (deliveryStage === "not_entered" && (clipboardRestoration !== "not_needed" || readback !== "unknown")) return null;
  if (deliveryStage === "entered" && (captureStage === "not_started" || target === null)) return null;
  if (readback === "exact_once" && (deliveryStage !== "entered" || target === null
    || captureStage !== "complete" || captureReason !== null || reason !== null)) return null;
  return {
    lease: parsedLease,
    recording_generation: value.recording_generation,
    trigger_source: "app_command",
    trial_current: value.trial_current,
    permissions,
    target,
    readback,
    clipboard_restoration: clipboardRestoration,
    capture_stage: captureStage,
    capture_reason: captureReason,
    delivery_stage: deliveryStage,
    reason,
    source: "product_same_field_ax_readback_not_independent_oracle",
    source_identity: sourceIdentity ?? undefined,
  };
}
function matchesPermissions(receipt: ExternalReceipt, permissions: OnboardingPermissions | null): boolean {
  return !!permissions && permissions.accessibility && receipt.permissions[0] === permissions.microphone
    && receipt.permissions[1] === permissions.accessibility && receipt.permissions[2] === permissions.screen_recording;
}

/** The native TrialOwner certifies this opt-in continuation. This panel uses the
 * same matching-ID lease hook as ordinary onboarding; it owns no target process. */
export function ExternalDeliveryTrial({ configurationKey, permissions, onGeneration, onContinuation, continuationIsCurrent, refreshPermissions, onConfirmed, onClose }: {
  configurationKey: string; permissions: OnboardingPermissions | null;
  onGeneration: (generation: string) => void; onContinuation: (lease: TrialLease | null) => void; continuationIsCurrent: (lease: TrialLease) => boolean;
  refreshPermissions: () => Promise<OnboardingPermissions | null | undefined>;
  onConfirmed: (receipt: ExternalReceipt) => void; onClose: () => void;
}) {
  const { t } = useI18n();
  const [attempt, setAttempt] = useState(0);
  const [targetApp, setTargetApp] = useState<ExternalTargetApp>("text_edit");
  const [sourceMode, setSourceMode] = useState<"fixed_fixture" | "history_revision">("fixed_fixture");
  const [historyItems, setHistoryItems] = useState<HistoryChoice[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [historyItemId, setHistoryItemId] = useState<number | null>(null);
  const [historyRevisions, setHistoryRevisions] = useState<HistoryRevisionChoice[]>([]);
  const [historyRevisionId, setHistoryRevisionId] = useState<number | null>(null);
  const selectedHistorySource = useMemo<ExternalHistorySelection | null>(
    () => sourceMode === "history_revision" && historyItemId !== null && historyRevisionId !== null
      ? { dictation_id: historyItemId, revision_id: historyRevisionId }
      : null,
    [sourceMode, historyItemId, historyRevisionId],
  );
  const sourceKey = sourceMode === "fixed_fixture"
    ? "fixed_fixture"
    : selectedHistorySource
      ? "history:" + selectedHistorySource.dictation_id + ":" + selectedHistorySource.revision_id
      : null;
  const [fixture, setFixture] = useState<Fixture | null>(null);
  const [receipt, setReceipt] = useState<ExternalReceipt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = permissions?.accessibility && sourceKey
    ? configurationKey + ":external:" + targetApp + ":" + attempt
      + (sourceMode === "fixed_fixture" ? "" : ":" + sourceKey)
    : null;
  const { lease, releaseAll, releaseFailed } = useOnboardingTrialLease(key, onGeneration);
  const current = useRef(lease); current.current = lease;
  const active = useRef(true);
  const continuation = useRef(onContinuation); continuation.current = onContinuation;
  const pending = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  useEffect(() => {
    if (sourceMode !== "history_revision") return;
    let live = true;
    setHistoryItems([]);
    setHistoryError(null);
    setHistoryLoading(true);
    void invoke<HistoryPage>("get_history", { limit: 50, query: null })
      .then((page) => {
        if (live) setHistoryItems(Array.isArray(page.items) ? page.items : []);
      })
      .catch(() => { if (live) setHistoryError("历史记录加载失败，请稍后重试。"); })
      .finally(() => { if (live) setHistoryLoading(false); });
    return () => { live = false; };
  }, [sourceMode]);
  useEffect(() => {
    if (sourceMode !== "history_revision" || historyItemId === null) {
      setHistoryRevisions([]);
      return;
    }
    let live = true;
    setHistoryRevisions([]);
    void invoke<HistoryRevisionChoice[]>("get_history_revisions", { id: historyItemId })
      .then((rows) => { if (live) setHistoryRevisions(Array.isArray(rows) ? rows : []); })
      .catch(() => { if (live) setHistoryError("整理版本加载失败，请重新选择历史记录。"); });
    return () => { live = false; };
  }, [sourceMode, historyItemId]);
  const eligibleRevisions = historyRevisions.filter((revision) =>
    revision.revision_reason === "ai_reclean" && revision.cleanup_status === "ai_success"
    && revision.intent?.operation === "cleanup" && revision.intent.source === "selected_text");
  const selectedRevision = eligibleRevisions.find((revision) => revision.revision_id === historyRevisionId) ?? null;
  const preview = (value: string) => {
    const compact = value.replace(/\s+/g, " ").trim();
    const chars = Array.from(compact);
    return chars.length > 72 ? chars.slice(0, 72).join("") + "…" : compact;
  };
  useEffect(() => {
    setFixture(null); setReceipt(null); setError(null); continuation.current(null);
    if (!lease) return;
    let live = true;
    const historySource = selectedHistorySource
      ? { dictation_id: selectedHistorySource.dictation_id, revision_id: selectedHistorySource.revision_id }
      : undefined;
    void invoke<unknown>("prepare_onboarding_external", {
      trialId: lease.trial_id,
      targetApp,
      historySource: historySource ?? null,
    }).then((value) => {
      if (!live || !sameLease(current.current, lease)) return;
      const prepared = parseFixture(value, lease, targetApp, selectedHistorySource);
      if (!prepared) { setError("外部测试准备身份无法确认，请重新选择历史版本。"); return; }
      setFixture(prepared); continuation.current(lease);
    }).catch(() => {
      if (live) setError(selectedHistorySource
        ? "所选历史整理版本已变化、不适用或超出测试长度；请选择其他成功版本。"
        : "外部测试准备失败，请检查辅助功能权限后重试。");
    });
    return () => {
      live = false; continuation.current(null);
      // This request is not treated as terminal. The existing hook retains
      // release ownership until the native matching-ID terminal receipt.
      void invoke("cancel_onboarding_external", { trialId: lease.trial_id }).catch(() => undefined);
    };
  }, [lease, targetApp, sourceMode, historyItemId, historyRevisionId, selectedHistorySource]);
  const isCurrent = (owned: TrialLease) => active.current && sameLease(current.current, owned) && continuationIsCurrent(owned);
  const copy = async () => {
    if (!fixture || !lease || pending.current) return;
    const owned = lease;
    try { await invoke("copy_onboarding_external_fixture", { trialId: owned.trial_id });
      if (isCurrent(owned)) setError(null);
    } catch { if (isCurrent(owned)) setError("固定测试文字复制未确认，请重试。"); }
  };
  const run = async () => {
    if (!fixture || !lease || pending.current || receipt) return;
    const owned = lease; pending.current = true; setBusy(true); setError(null);
    try {
      const value = await invoke<unknown>("run_onboarding_external", { trialId: owned.trial_id });
      if (!isCurrent(owned)) return;
      const observed = parseExternalReceipt(value, owned, targetApp, fixture.source_identity ?? null);
      if (!observed) { setError("外部测试结果身份无法确认；请检查目标，不要盲目重试。"); return; }
      setReceipt(observed);
    } catch { if (isCurrent(owned)) setError("外部测试未确认；请检查目标，不要盲目重试。"); }
    finally { pending.current = false; if (active.current) setBusy(false); }
  };
  const cancel = useCallback(async () => {
    const owned = current.current;
    if (owned) await invoke("cancel_onboarding_external", { trialId: owned.trial_id });
    await releaseAll(); // waits for the native joined operation, not cancel IPC
    continuation.current(null);
  }, [releaseAll]);
  const close = async () => {
    try { await cancel(); onClose(); }
    catch { if (active.current) setError("试用资源释放尚未确认；请重试，旧会话不会作为当前成功。"); }
  };
  const retry = async () => {
    if (pending.current) return;
    try { await cancel(); if (active.current) setAttempt((value) => value + 1); }
    catch { if (active.current) setError("试用资源释放尚未确认；请重试，旧会话不会作为当前成功。"); }
  };
  const confirm = async () => {
    if (!lease || !receipt || pending.current) return;
    const owned = lease; pending.current = true; setBusy(true);
    try {
      // A fresh readback is mandatory even when the UI still shows granted.
      const permissions = await refreshPermissions();
      if (!isCurrent(owned) || !matchesPermissions(receipt, permissions ?? null)) throw new Error("external_permission_changed");
      const value = await invoke<unknown>("confirm_onboarding_external", { trialId: owned.trial_id });
      const confirmed = parseExternalReceipt(value, owned, targetApp, fixture?.source_identity ?? null);
      if (!isCurrent(owned) || !confirmed || !matchesPermissions(confirmed, permissions ?? null)) throw new Error("external_receipt_stale");
      onConfirmed(confirmed);
      await releaseAll(); continuation.current(null); onClose();
    } catch { if (active.current) setError("权限、配置或测试期限已变化；旧结果不能恢复为当前成功，请重新准备。"); }
    finally { pending.current = false; if (active.current) setBusy(false); }
  };
  return <section aria-label={t("受控外部插入测试")} className="mt-5 rounded-lg border border-border p-4 text-left text-sm">
    <h2 className="font-medium">{t("受控外部插入测试")}</h2>
    <p className="mt-2 text-xs text-secondary">{t("可选：只测试你准备的固定测试目标，不录音、不调用模型、不保存历史。应用内回读是产品事实，独立验收另行进行。")}</p>
    <label className="mt-3 block text-xs text-secondary">{t("插入内容来源")}
      <Select aria-label={t("插入内容来源")} className="mt-1 block w-full rounded-md border border-border bg-card px-3 py-2 text-primary" value={sourceMode} disabled={busy || !!receipt} onValueChange={(value) => {
        const next = value as "fixed_fixture" | "history_revision";
        setSourceMode(next);
        if (next === "history_revision") { setHistoryItemId(null); setHistoryRevisionId(null); setHistoryRevisions([]); }
        setHistoryError(null);
      }}>
        <option value="fixed_fixture">{t("固定测试文字（默认）")}</option>
        <option value="history_revision">{t("已整理的历史版本")}</option>
      </Select>
    </label>
    {sourceMode === "history_revision" && <>
      <label className="mt-3 block text-xs text-secondary">{t("历史记录")}
        <Select aria-label={t("历史记录")} className="mt-1 block w-full rounded-md border border-border bg-card px-3 py-2 text-primary" value={historyItemId ?? ""} disabled={historyLoading || busy || !!receipt} onValueChange={(value) => {
          const next = Number(value);
          setHistoryItemId(Number.isSafeInteger(next) && next > 0 ? next : null);
          setHistoryRevisionId(null);
          setHistoryRevisions([]);
          setHistoryError(null);
        }}>
          <option value="">{historyLoading ? t("正在加载历史记录…") : t("选择一条历史记录")}</option>
          {historyItems.map((item) => <option key={item.id} value={item.id}>
            {item.created_at} — {preview(item.final_text || item.raw_text)}
          </option>)}
        </Select>
      </label>
      <label className="mt-3 block text-xs text-secondary">{t("成功整理版本")}
        <Select aria-label={t("成功整理版本")} className="mt-1 block w-full rounded-md border border-border bg-card px-3 py-2 text-primary" value={historyRevisionId ?? ""} disabled={historyItemId === null || busy || !!receipt} onValueChange={(value) => {
          const next = Number(value);
          setHistoryRevisionId(Number.isSafeInteger(next) && next > 0 ? next : null);
        }}>
          <option value="">{historyItemId === null
            ? t("请先选择历史记录")
            : eligibleRevisions.length === 0
              ? t("这条记录没有成功整理版本")
              : t("选择一个成功的清理版本")}</option>
          {eligibleRevisions.map((revision, index) => <option key={revision.revision_id} value={revision.revision_id}>
            {t("整理版本")} {index + 1} — {revision.created_at} — {preview(revision.final_text)}
          </option>)}
        </Select>
      </label>
      {selectedRevision && <div className="mt-2 rounded bg-surface-inset p-3 text-xs">
        <p className="mb-1 text-secondary">{t("将插入所选的已保存整理版本")}</p>
        <p className="whitespace-pre-wrap text-primary">{selectedRevision.final_text}</p>
      </div>}
      <p className="mt-2 text-xs text-secondary">{t("准备时会固定所选成功版本；若记录变化、不可用或超出 512 个 UTF-16 单元的测试载荷上限，测试会拒绝，不会改用固定文字。")}</p>
      {historyError && <p role="alert" className="mt-2 text-xs text-error-ink">{t(historyError)}</p>}
    </>}
    <label className="mt-3 block text-xs text-secondary">{t("测试目标")}
      <Select aria-label={t("测试目标")} className="mt-1 block w-full rounded-md border border-border bg-card px-3 py-2 text-primary" value={targetApp} disabled={busy || !!receipt} onValueChange={(value) => setTargetApp(value as ExternalTargetApp)}>
        <option value="text_edit">TextEdit</option>
        <option value="chrome">Chrome 本地测试页</option>
      </Select>
    </label>
    <p className="mt-2 text-xs text-secondary">{t(targetApp === "text_edit"
      ? "准备期限三分钟。请新建空白纯文本文档，复制下方固定文字并粘贴，然后只选中 REPLACE ONLY THIS。不要使用个人文档。"
      : "准备期限三分钟。请只打开本次受控的 VoiceFlow 本地 Chrome 测试页，复制下方固定文字并粘贴到其唯一的纯文本框，再只选中 REPLACE ONLY THIS。不要使用个人页面或其他标签页。")}</p>
    {fixture && <><pre className="mt-3 whitespace-pre-wrap rounded bg-surface-inset p-3 text-xs">{fixture.before}</pre>
      {fixture.source_identity && <p className="mt-2 text-xs text-secondary">{t("目标仍是本次受控测试文档；插入内容来自上方所选历史版本。")}</p>}
      <p className="mt-2 text-xs text-secondary">{t("复制固定文字会主动替换当前剪贴板。测试只恢复开始插入时的剪贴板，不会恢复复制前的内容。")}</p>
      <button type="button" className={`mt-2 ${secondaryButtonClass}`} disabled={busy || !!receipt} onClick={() => void copy()}>{t("复制固定测试文字")}</button>
      <button type="button" className={`mt-2 ml-2 ${secondaryButtonClass}`} disabled={busy || !!receipt} onClick={() => void run()}>{t(targetApp === "text_edit" ? "五秒后测试：现在切换到固定 TextEdit 文档" : "五秒后测试：现在切换到受控 Chrome 测试页")}</button></>}
    {busy && <p role="status" className="mt-3 text-xs">{t("测试尚未结束；取消请求不代表资源已释放。请勿重复粘贴。")}</p>}
    {receipt && <div className="mt-3 text-xs"><p>{t(receipt.trial_current && receipt.readback === "exact_once" && receipt.target && receipt.delivery_stage === "entered" ? "固定目标中已回读到恰好一次插入。" : "插入未确认；请检查固定文档，勿盲目重试。")}</p>
      <p>{t("原生焦点检查阶段")}：{t(captureStageLabels[receipt.capture_stage])}</p>
      <p>{t("原生焦点检查结果")}：{t(receipt.capture_reason ? captureReasonLabels[receipt.capture_reason] : receipt.capture_stage === "complete" ? "未发现已知的焦点检查异常" : "焦点检查尚无结果")}</p>
      <p>{t("插入交付阶段")}：{t(receipt.delivery_stage === "entered" ? "已进入插入交付流程；不代表按键已发送" : "尚未进入插入交付流程")}</p>
      <p>{t(receipt.clipboard_restoration === "restored" ? "本次测试开始时的剪贴板已确认恢复。" : receipt.clipboard_restoration === "not_needed" ? "本次测试无需恢复剪贴板。" : receipt.clipboard_restoration === "ownership_changed" ? "剪贴板所有权已变化，未覆盖新内容。" : "剪贴板恢复无法确认。")}</p>
      {receipt.reason && <p>{t("未确认原因")}：{receipt.reason}</p>}
      <button type="button" className={`mt-2 ${secondaryButtonClass}`} disabled={busy} onClick={() => void confirm()}>{t("返回后重新检查权限并记录本次事实")}</button></div>}
    {(error || releaseFailed) && <p role="alert" className="mt-3 text-xs text-error-ink">{t(error ?? "试用资源释放尚未确认；请重试，旧会话不会作为当前成功。")}</p>}
    <div className="mt-3 flex gap-2"><button type="button" className={ghostButtonClass} disabled={busy} onClick={() => void retry()}>{t("重新准备固定测试")}</button>
      <button type="button" className={ghostButtonClass} onClick={() => void close()}>{t(busy ? "取消并等待释放" : "关闭外部测试")}</button></div>
  </section>;
}
