import { useEffect, useRef, useState } from "react";
import { useI18n } from "../lib/i18n";
import { buttonClass, compactButtonClass, secondaryButtonClass } from "../lib/theme";
import { SettingsAlert, SettingsDisclosure } from "./SettingsLayout";
import type { WritingPreferenceDimension, WritingPreferenceValue, WritingTaskId } from "./ContextSettings";

export type WritingPreferenceCandidate = {
  candidate_id: string;
  candidate_generation: number;
  mapping_id: string;
  mapping_revision: number;
  app_label: string;
  task_id: WritingTaskId;
  dimension: WritingPreferenceDimension;
  value: WritingPreferenceValue;
  has_text_sample: false;
  /** Added by the settings owner after joining the current mapping snapshot. */
  display_scope?: string | null;
  distinct_origins: number;
  distinct_natural_days: number;
  distinct_sessions: number;
  qualified_at_ms: number;
};

export type WritingPreferenceDecision = "approve" | "dismiss";

export type WritingPreferenceDecisionItem = {
  candidate_id: string;
  candidate_generation: number;
  expected_mapping_revision: number;
  decision: WritingPreferenceDecision;
};

export type WritingPreferenceDecisionOutcome = {
  candidate_id: string;
  mapping_id: string | null;
  decision: WritingPreferenceDecision;
  status: "approved" | "dismissed" | "stale_candidate" | "stale_mapping" | "source_recheck_failed" | "mapping_unavailable" | "permission_revoked" | "preference_conflict" | "already_configured" | "already_decided" | "not_found" | "duplicate_request" | "settings_save_failed" | "decision_save_failed";
  new_mapping_revision: number | null;
};

export type WritingPreferenceReviewProps = {
  onLoadCandidates: () => Promise<WritingPreferenceCandidate[]>;
  onDecideCandidates: (
    items: WritingPreferenceDecisionItem[],
    candidates: WritingPreferenceCandidate[],
  ) => Promise<WritingPreferenceDecisionOutcome[]>;
  onRefreshApprovedMappings?: (
    candidates: WritingPreferenceCandidate[],
    outcomes: WritingPreferenceDecisionOutcome[],
  ) => Promise<void>;
};

const MAX_REVIEW_SELECTION = 64;

export const taskLabels: Record<WritingTaskId, string> = {
  project_update: "项目更新",
  email: "邮件正文",
  work_chat: "工作聊天",
  personal_chat: "私人聊天",
  document_editing: "文档撰写",
};

export const dimensionLabels: Record<WritingPreferenceDimension, string> = {
  list_structure: "列表结构",
  greeting_tone: "短称呼形式",
  terminal_punctuation: "陈述句句末标点",
};

export const valueLabels: Record<WritingPreferenceValue, string> = {
  bullets: "符合来源证据时使用要点",
  conversational_greeting: "你好（仅替换原有称呼）",
  respectful_greeting: "您好（仅替换原有称呼）",
  terminal_period: "保留一个句号",
  no_terminal_period: "聊天句末省略句号",
};

const dimensionValueIsCompatible: Record<WritingPreferenceDimension, readonly WritingPreferenceValue[]> = {
  list_structure: ["bullets"],
  greeting_tone: ["conversational_greeting", "respectful_greeting"],
  terminal_punctuation: ["terminal_period", "no_terminal_period"],
};

const terminalOutcomes = new Set<WritingPreferenceDecisionOutcome["status"]>([
  "approved",
  "dismissed",
  "mapping_unavailable",
  "permission_revoked",
  "stale_candidate",
  "stale_mapping",
  "source_recheck_failed",
  "already_configured",
  "already_decided",
  "not_found",
  "duplicate_request",
  "settings_save_failed",
  "decision_save_failed",
]);

const approvalBlockedOutcomes = new Set<WritingPreferenceDecisionOutcome["status"]>([
  "preference_conflict",
]);

const genericOutcomeReason = "刷新候选后即可确认此项的当前状态。";

function candidateLabel(candidate: WritingPreferenceCandidate, t: (source: string) => string): string {
  const task = taskLabels[candidate.task_id];
  const dimension = dimensionLabels[candidate.dimension];
  const value = valueLabels[candidate.value];
  return `${candidate.app_label} · ${t(task)} · ${t(dimension)} · ${t(value)}`;
}

function outcomeLabel(status: WritingPreferenceDecisionOutcome["status"], t: (source: string) => string): string {
  switch (status) {
    case "approved": return t("已批准为显式偏好");
    case "dismissed": return t("已忽略此候选");
    case "stale_candidate": return t("候选已更新，请刷新后复核");
    case "stale_mapping": return t("偏好映射已更新，请刷新后复核");
    case "source_recheck_failed": return t("History 来源未能重新验证；请刷新候选列表后再决定是否重试");
    case "mapping_unavailable": return t("App 规则已停用或删除；请刷新候选列表后再复核");
    case "permission_revoked": return t("本机观察授权已撤销；请刷新候选列表后再复核");
    case "preference_conflict": return t("现有偏好与候选冲突；请检查 App 规则");
    case "already_configured": return t("此偏好已由 App 规则配置，请刷新候选");
    case "already_decided": return t("候选已处理，请刷新列表");
    case "not_found": return t("候选已不存在，请刷新列表");
    case "duplicate_request": return t("候选在批次中重复，请刷新列表");
    case "settings_save_failed": return t("偏好未能写入 App 规则，请刷新候选");
    case "decision_save_failed": return t("复核结果未能保存，请刷新候选");
  }
}

function candidateNeedsRefresh(status: WritingPreferenceDecisionOutcome["status"]): boolean {
  return terminalOutcomes.has(status);
}

function outcomeBlocksApproval(status: WritingPreferenceDecisionOutcome["status"] | undefined): boolean {
  return status !== undefined && approvalBlockedOutcomes.has(status);
}

function candidateKey(candidate: WritingPreferenceCandidate): string {
  return `${candidate.candidate_id}:${candidate.candidate_generation}`;
}

export function WritingPreferenceReview({ onLoadCandidates, onDecideCandidates, onRefreshApprovedMappings }: WritingPreferenceReviewProps) {
  const { t } = useI18n();
  const [candidates, setCandidates] = useState<WritingPreferenceCandidate[]>([]);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [outcomes, setOutcomes] = useState<Map<string, WritingPreferenceDecisionOutcome>>(() => new Map());
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [reviewError, setReviewError] = useState(false);
  const [requiresRefresh, setRequiresRefresh] = useState(false);
  const [mappingRefreshError, setMappingRefreshError] = useState(false);
  const [selectionLimitReached, setSelectionLimitReached] = useState(false);
  const [resultSummary, setResultSummary] = useState<string | null>(null);
  const busyRef = useRef(false);
  const requestIdRef = useRef(0);
  const pendingMappingRefreshRef = useRef<{ candidates: WritingPreferenceCandidate[]; outcomes: WritingPreferenceDecisionOutcome[] } | null>(null);

  const mountedRef = useRef(false);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      requestIdRef.current += 1;
    };
  }, []);

  const isCurrentRequest = (requestId: number) => mountedRef.current && requestId === requestIdRef.current;

  const loadCandidates = async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setLoaded(false);
    setCandidates([]);
    setSelectedIds(new Set());
    setSelectionLimitReached(false);
    setOutcomes(new Map());
    setResultSummary(null);
    setLoadError(false);
    setReviewError(false);
    try {
      const rows = await onLoadCandidates();
      if (requestId !== requestIdRef.current) return;
      if (!Array.isArray(rows)) throw new Error("preference_candidates_invalid_response");
      setCandidates(rows);
      setLoaded(true);
      setRequiresRefresh(false);
    } catch {
      if (isCurrentRequest(requestId)) setLoadError(true);
    } finally {
      if (isCurrentRequest(requestId)) {
        busyRef.current = false;
        setLoading(false);
      }
    }
  };

  const refreshApprovedMappings = async (requestId: number) => {
    const pending = pendingMappingRefreshRef.current;
    if (!onRefreshApprovedMappings || !pending || !isCurrentRequest(requestId)) return;
    try {
      await onRefreshApprovedMappings(pending.candidates, pending.outcomes);
      if (!isCurrentRequest(requestId)) return;
      pendingMappingRefreshRef.current = null;
      setMappingRefreshError(false);
    } catch {
      if (isCurrentRequest(requestId)) setMappingRefreshError(true);
    }
  };

  const eligibleCandidates = candidates.filter((candidate) => {
    const status = outcomes.get(candidate.candidate_id)?.status;
    const hasLiveScope = typeof candidate.display_scope === "string" && candidate.display_scope.trim().length > 0;
    return hasLiveScope && (status === undefined || !candidateNeedsRefresh(status));
  });

  const setAllSelected = (checked: boolean) => {
    if (!checked) {
      setSelectedIds(new Set());
      setSelectionLimitReached(false);
      return;
    }
    setSelectedIds(new Set(eligibleCandidates.slice(0, MAX_REVIEW_SELECTION).map(candidateKey)));
    setSelectionLimitReached(eligibleCandidates.length > MAX_REVIEW_SELECTION);
  };

  const toggleCandidate = (candidate: WritingPreferenceCandidate, checked: boolean) => {
    const key = candidateKey(candidate);
    const next = new Set(selectedIds);
    if (checked) {
      if (next.size >= MAX_REVIEW_SELECTION && !next.has(key)) {
        setSelectionLimitReached(true);
        return;
      }
      next.add(key);
    } else {
      next.delete(key);
      if (next.size < MAX_REVIEW_SELECTION) setSelectionLimitReached(false);
    }
    setSelectedIds(next);
  };

  const decideSelected = async (decision: WritingPreferenceDecision) => {
    if (busyRef.current || requiresRefresh || mappingRefreshError || !loaded) return;
    const selected = eligibleCandidates.filter((candidate) => selectedIds.has(candidateKey(candidate)));
    if (selected.length === 0 || (decision === "approve" && selected.some((candidate) => outcomeBlocksApproval(outcomes.get(candidate.candidate_id)?.status)))) return;
    busyRef.current = true;
    const requestId = requestIdRef.current;
    setReviewing(true);
    setReviewError(false);
    setResultSummary(null);
    try {
      const decisions = selected.map((candidate) => ({
        candidate_id: candidate.candidate_id,
        candidate_generation: candidate.candidate_generation,
        expected_mapping_revision: candidate.mapping_revision,
        decision,
      }));
      const results = await onDecideCandidates(decisions, selected);
      if (!isCurrentRequest(requestId)) return;
      if (!Array.isArray(results)) throw new Error("preference_review_invalid_response");
      const resultById = new Map(results.map((result) => [result.candidate_id, result]));
      const nextOutcomes = new Map(outcomes);
      let approved = 0;
      let dismissed = 0;
      let needsAttention = 0;
      for (const candidate of selected) {
        const result = resultById.get(candidate.candidate_id) ?? {
          candidate_id: candidate.candidate_id,
          mapping_id: candidate.mapping_id,
          decision,
          status: "not_found" as const,
          new_mapping_revision: null,
        };
        nextOutcomes.set(candidate.candidate_id, result);
        if (result.status === "approved") approved += 1;
        else if (result.status === "dismissed") dismissed += 1;
        else needsAttention += 1;
      }
      setOutcomes(nextOutcomes);
      setSelectedIds(new Set());
      setSelectionLimitReached(false);
      setResultSummary(t("处理结果：批准 {approved} 项，忽略 {dismissed} 项，需复核 {attention} 项。")
        .replace("{approved}", String(approved))
        .replace("{dismissed}", String(dismissed))
        .replace("{attention}", String(needsAttention)));
      const approvedCandidates = selected.filter((candidate) => resultById.get(candidate.candidate_id)?.status === "approved");
      if (approvedCandidates.length > 0 && onRefreshApprovedMappings) {
        const approvedOutcomes = approvedCandidates.flatMap((candidate) => {
          const outcome = resultById.get(candidate.candidate_id);
          return outcome ? [outcome] : [];
        });
        pendingMappingRefreshRef.current = { candidates: approvedCandidates, outcomes: approvedOutcomes };
        await refreshApprovedMappings(requestId);
      }
    } catch {
      if (isCurrentRequest(requestId)) {
        setReviewError(true);
        setRequiresRefresh(true);
        setSelectedIds(new Set());
      }
    } finally {
      busyRef.current = false;
      if (isCurrentRequest(requestId)) setReviewing(false);
    }
  };

  const allSelected = eligibleCandidates.length > 0
    && eligibleCandidates.slice(0, MAX_REVIEW_SELECTION).every((candidate) => selectedIds.has(candidateKey(candidate)));
  const selectedCandidates = eligibleCandidates.filter((candidate) => selectedIds.has(candidateKey(candidate)));
  const approvalBlocked = selectedCandidates.some((candidate) => outcomeBlocksApproval(outcomes.get(candidate.candidate_id)?.status));

  return (
    <SettingsDisclosure
      title={t("待复核的写作偏好")}
      description={t("打开列表后逐项检查候选；批准或忽略都需要你主动选择。")}
      summary={t(loaded ? "候选 {count} 项" : "尚未读取").replace("{count}", String(candidates.length))}
      onExpandedChange={(expanded) => { if (expanded) void loadCandidates(); }}
    >
      <div className="space-y-3 pt-4">
        <p className="max-w-prose text-xs leading-5 text-secondary">
          {t("本次只复核结构化偏好；列表仅含 App、任务、维度、预测值和聚合计数，不含原文样例。批准只改变本机偏好；关闭此面板不代表批准或忽略。")}
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" onClick={() => setAllSelected(!allSelected)} disabled={loading || reviewing || requiresRefresh || mappingRefreshError || !loaded || eligibleCandidates.length === 0} className={compactButtonClass}>
            {t(allSelected && eligibleCandidates.length > MAX_REVIEW_SELECTION ? "取消本批选择" : allSelected ? "取消全选" : "全选待复核偏好")}
          </button>
          <button type="button" onClick={() => setSelectedIds(new Set())} disabled={loading || reviewing || requiresRefresh || mappingRefreshError || selectedIds.size === 0} className={compactButtonClass}>
            {t("取消选择")}
          </button>
          <button type="button" onClick={() => void decideSelected("approve")} disabled={loading || reviewing || requiresRefresh || mappingRefreshError || !loaded || selectedIds.size === 0 || approvalBlocked} aria-busy={reviewing} className={buttonClass}>
            {t("批准所选偏好")} ({selectedIds.size})
          </button>
          <button type="button" onClick={() => void decideSelected("dismiss")} disabled={loading || reviewing || requiresRefresh || mappingRefreshError || !loaded || selectedIds.size === 0} aria-busy={reviewing} className={secondaryButtonClass}>
            {t("忽略所选候选")} ({selectedIds.size})
          </button>
          <button type="button" onClick={() => void loadCandidates()} disabled={loading || reviewing} className={compactButtonClass}>
            {t(loading ? "正在读取…" : "刷新候选列表")}
          </button>
        </div>
        {selectionLimitReached && <p role="status" className="text-xs text-secondary">{t("每批最多复核 64 项；请先处理当前选择，再继续选择其余候选。")}</p>}
        {approvalBlocked && <p role="status" className="text-xs text-warning-ink">{t("所选候选中有无法批准的项目；可取消选择该项，或将其忽略。")}</p>}
        {loadError && <SettingsAlert onRetry={() => void loadCandidates()} retryLabel={t("重试")}>{t("候选列表读取失败；现有设置未更改。请重试。")}</SettingsAlert>}
        {reviewError && <SettingsAlert onRetry={() => void loadCandidates()} retryLabel={t("刷新候选列表")}>{t("批量操作结果未确认；请先刷新候选，再决定是否重试。")}</SettingsAlert>}
        {mappingRefreshError && <SettingsAlert onRetry={() => void refreshApprovedMappings(requestIdRef.current)} retryLabel={t("重试同步 App 偏好")}>{t("偏好已批准，但 App 规则未能同步。请重试同步；完成前不要保存这个 App 规则。")}</SettingsAlert>}
        {loading && <p role="status" className="text-sm text-secondary">{t("正在读取待复核偏好…")}</p>}
        {reviewing && <p role="status" className="text-sm text-secondary">{t("正在处理所选偏好…")}</p>}
        {resultSummary && <p role="status" aria-live="polite" className="text-sm text-secondary">{resultSummary}</p>}
        {!loading && loaded && candidates.length === 0 && (
          <div className="rounded-lg border border-border px-3 py-3">
            <p role="status" className="text-sm text-secondary">{t("目前没有待复核的写作偏好。")}</p>
            <p className="mt-1 text-xs leading-5 text-tertiary">{t("列表为空也可能是全局学习词条、本机改动观察或此 App 规则学习未开启；关闭这三类学习开关会清除未批准证据，已批准偏好保留。全局上下文、App 规则或浏览器权限关闭时会暂时阻止候选处理，但不会清除证据。")}</p>
          </div>
        )}
        {!loading && candidates.length > 0 && (
          <div className="divide-y divide-border rounded-lg border border-border" aria-label={t("写作偏好候选列表")}>
            {candidates.map((candidate) => {
              const rowOutcome = outcomes.get(candidate.candidate_id);
              const compatibleValues = dimensionValueIsCompatible[candidate.dimension];
              const malformed = !compatibleValues.includes(candidate.value);
              const label = candidateLabel(candidate, t);
              return (
                <div key={candidateKey(candidate)} role="group" aria-label={label} className="flex flex-wrap items-start gap-3 px-4 py-3">
                  <label className="flex min-h-10 shrink-0 items-center gap-2 text-xs text-secondary">
                    <input
                      type="checkbox"
                      checked={selectedIds.has(candidateKey(candidate))}
                      onChange={(event) => toggleCandidate(candidate, event.target.checked)}
                      disabled={loading || reviewing || requiresRefresh || mappingRefreshError || !loaded || Boolean(rowOutcome && candidateNeedsRefresh(rowOutcome.status)) || malformed || typeof candidate.display_scope !== "string" || candidate.display_scope.trim().length === 0}
                      aria-label={`${t("选择候选偏好")}: ${label} · ${t("规则范围")}: ${candidate.display_scope ?? t("无法识别当前规则范围")}`}
                      className="accent-current"
                    />
                    <span>{t("选择")}</span>
                  </label>
                  <dl className="grid min-w-0 flex-1 grid-cols-1 gap-x-5 gap-y-2 text-xs sm:grid-cols-2">
                    <div className="min-w-0"><dt className="text-tertiary">{t("App")}</dt><dd className="mt-0.5 break-words font-medium text-primary">{candidate.app_label}</dd></div>
                    <div className="min-w-0 sm:col-span-2"><dt className="text-tertiary">{t("规则范围")}</dt><dd className="mt-0.5 break-words text-secondary">{candidate.display_scope ?? t("无法识别当前规则范围")}</dd></div>
                    <div className="min-w-0"><dt className="text-tertiary">{t("写作任务")}</dt><dd className="mt-0.5 break-words text-secondary">{t(taskLabels[candidate.task_id])}</dd></div>
                    <div className="min-w-0"><dt className="text-tertiary">{t("偏好维度")}</dt><dd className="mt-0.5 break-words text-secondary">{t(dimensionLabels[candidate.dimension])}</dd></div>
                    <div className="min-w-0"><dt className="text-tertiary">{t("预测值")}</dt><dd className="mt-0.5 break-words text-secondary">{t(valueLabels[candidate.value])}</dd></div>
                    <div className="min-w-0"><dt className="text-tertiary">{t("原文样例")}</dt><dd className="mt-0.5 text-secondary">{t("不包含原文样例")}</dd></div>
                    <div className="min-w-0 sm:col-span-2">
                      <dt className="text-tertiary">{t("聚合证据")}</dt>
                      <dd className="mt-0.5 break-words text-secondary">
                        {t("{origins} 笔独立来源 · {days} 个自然日 · {sessions} 个会话")
                          .replace("{origins}", String(candidate.distinct_origins))
                          .replace("{days}", String(candidate.distinct_natural_days))
                          .replace("{sessions}", String(candidate.distinct_sessions))}
                      </dd>
                    </div>
                    {malformed && <div className="sm:col-span-2"><p role="alert" className="text-warning-ink">{t("偏好维度与预测值不匹配，无法批量处理。")}</p></div>}
                    {candidate.display_scope === null && <div className="sm:col-span-2"><p role="alert" className="text-warning-ink">{t("无法读取此候选对应的当前规则范围，请刷新候选列表。")}</p></div>}
                    {rowOutcome && (
                      <div className="sm:col-span-2">
                        <dt className="text-tertiary">{t("处理结果")}</dt>
                        <dd className={`mt-0.5 ${rowOutcome.status === "approved" || rowOutcome.status === "dismissed" ? "text-secondary" : "text-warning-ink"}`}>
                          {outcomeLabel(rowOutcome.status, t)}
                          {rowOutcome.status !== "approved" && rowOutcome.status !== "dismissed" && <span className="block">{t(genericOutcomeReason)}</span>}
                        </dd>
                      </div>
                    )}
                  </dl>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </SettingsDisclosure>
  );
}
