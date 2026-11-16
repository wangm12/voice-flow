import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Download, LoaderCircle, Upload } from "lucide-react";
import { useI18n } from "../lib/i18n";
import { buttonClass, compactButtonClass, focusRingClass, secondaryButtonClass } from "../lib/theme";
import { SettingsAlert, SettingsGroup, SettingsRow, SettingsStatus } from "./SettingsLayout";
import { dimensionLabels, taskLabels, valueLabels } from "./WritingPreferenceReview";
import type { WritingPreferenceDimension, WritingPreferenceStatus, WritingPreferenceValue, WritingTaskId } from "./ContextSettings";

type TransferKind = "global_word" | "word_pair" | "writing_preference";
type TransferStatus = "ready" | "conflict" | "already_present" | "blocked";
type TransferChoice = "keep_local" | "replace" | "import_global";
type TransferItem = {
  itemId: string;
  kind: TransferKind;
  summary: string;
  scopeSummary: string;
  willCreateMapping?: boolean;
  status: TransferStatus;
  defaultAction: "import" | "keep_local" | "none";
  choiceOptions: TransferChoice[];
  reason?: string;
  details?: {
    word?: string;
    before?: string;
    after?: string;
    taskId?: WritingTaskId;
    dimension?: WritingPreferenceDimension;
    value?: WritingPreferenceValue;
    existingValue?: string;
  };
};
type TransferSourceSummary = { globalWordCount: number; pairCount: number; preferenceCount: number };
type TransferPreview = {
  status: "preview";
  previewId: string;
  expiresAtMs: number;
  sourceSummary?: TransferSourceSummary;
  items: TransferItem[];
};
type PreviewResponse = { status: "cancelled" } | TransferPreview;
type ExportResponse = {
  status: "exported" | "cancelled";
  globalWordCount: number;
  pairCount: number;
  preferenceCount: number;
  skippedCount: number;
};
type TransferResultStatus = "imported" | "kept_local" | "already_present" | "blocked" | "failed";
type TransferItemResult = { itemId: string; status: TransferResultStatus; reason?: string };
type ApplyResponse = { status: "applied" | "partial"; results: TransferItemResult[] };
type Operation = "export" | "preview" | "apply";
type TransferProblem = { message: string; retry?: Operation };

const kindLabels: Record<TransferKind, string> = {
  global_word: "全局个人词条",
  word_pair: "已批准纠正",
  writing_preference: "已保存的偏好",
};

const statusLabels: Record<TransferStatus, string> = {
  ready: "可导入",
  conflict: "与本机冲突",
  already_present: "本机已存在",
  blocked: "无法导入",
};

const choiceLabels: Record<TransferChoice, string> = {
  keep_local: "保留本机（默认）",
  replace: "替换本机项",
  import_global: "加入全局词典",
};

const resultLabels: Record<TransferResultStatus, string> = {
  imported: "已导入",
  kept_local: "已保留本机",
  already_present: "本机已存在",
  blocked: "已阻止",
  failed: "导入失败",
};

const preferenceValueDebugNames: Record<string, WritingPreferenceValue> = {
  Bullets: "bullets",
  ConversationalGreeting: "conversational_greeting",
  RespectfulGreeting: "respectful_greeting",
  TerminalPeriod: "terminal_period",
  NoTerminalPeriod: "no_terminal_period",
};

const preferenceStatusDebugNames: Record<string, WritingPreferenceStatus> = {
  Active: "active",
  Paused: "paused",
  Forgotten: "forgotten",
};

const preferenceStatusLabels: Record<WritingPreferenceStatus, string> = {
  active: "保存并应用",
  paused: "暂停",
  forgotten: "忘记此偏好",
};

const taskDebugNames: Record<WritingTaskId, string> = {
  project_update: "ProjectUpdate",
  email: "Email",
  work_chat: "WorkChat",
  personal_chat: "PersonalChat",
  document_editing: "DocumentEditing",
};

const reasonLabels: Record<string, string> = {
  already_global: "本机全局词典中已有此词条。",
  global_scope_upgrade: "本机学习规则中已有此词条；加入后会成为全局词条。",
  dictionary_capacity: "本机全局词典已满，无法加入此词条。",
  pair_has_local_suppression_or_pending_state: "本机同范围规则已忽略、撤销、暂停或仍待确认，无法安全替换。",
  pair_conflict_ambiguous: "本机有多条范围重叠的纠正规则，无法判断要替换哪一条。",
  pair_key_identity_conflict: "本机同一条规则标识对应不同内容；已阻止导入。",
  already_present: "本机已有相同内容，无需导入。",
  import_requires_local_only_pair: "本机已有全局纠正；替换后将保留为本机规则，不会加入全局词条。",
  pair_conflict: "本机同范围已有不同纠正。",
  pair_scope_overlap_or_ambiguous: "本机存在范围重叠或不明确的纠正，不能安全替换。",
  mapping_unavailable: "目标 App 规则在本机不可用。",
  mapping_disabled: "目标 App 规则已停用；导入不会重新启用它。",
  mapping_task_conflict: "本机 App 规则使用了与文件不同的写作任务。",
  mapping_selector_ambiguous: "多个本机 App 规则符合此目标，无法确定目标规则。",
  mapping_selector_overlap: "本机已有相近的 App 规则，无法安全新建目标规则。",
  forgotten_preference: "此偏好已在本机被忘记；导入不会恢复它。",
  preference_conflict: "本机此目标和任务已存在不同偏好。",
  new_local_mapping: "导入会为此目标新建并启用本机 App 规则；词典学习保持关闭，不会生成待确认候选。来源机权限和云端批准不会迁移。",
  global_rule_owner_will_be_added: "此词条目前由本机纠正规则提供。导入会另行加入全局词条，规则更改时仍保留。",
  pair_write_pending: "本机规则写入尚未得到确认。",
  pair_replaced_and_localized: "同范围纠正规则已替换，并保持为本机规则。",
  pair_conflict_rechecked: "应用前检测到本机规则已变化；本机规则已保留。",
  pair_blocked_after_recheck: "应用前重新检查发现规则已暂停、忽略或有歧义；本机规则已保留。",
  stale_pair_snapshot: "应用前检测到本机规则快照已变化，请重新预览。",
  learning_transfer_settings_not_saved: "个人词条或 App 偏好未能保存。",
  learning_transfer_pair_store_failed: "App 范围纠正规则未能保存。",
  learning_transfer_mapping_created_pair_failed: "已新建本机 App 规则，但对应纠正规则未能保存。",
  learning_transfer_dictionary_sync_pending: "规则已导入，但个人词典同步未完成。",
};

const PAGE_SIZE = 100;

function errorText(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  if (reason && typeof reason === "object" && "message" in reason && typeof reason.message === "string") return reason.message;
  return String(reason);
}

function isTransferError(reason: unknown, prefix: string): boolean {
  return errorText(reason).includes(prefix);
}

function withCount(source: string, count: number, translate: (source: string) => string): string {
  return translate(source).replace("{count}", String(count));
}

function itemSummary(item: TransferItem, translate: (source: string) => string): string {
  if (item.kind === "global_word") return item.details?.word ?? item.summary.replace(/^Global word:\s*/, "");
  if (item.kind === "writing_preference" && item.details?.dimension && item.details.value) {
    return `${translate(dimensionLabels[item.details.dimension])}: ${translate(valueLabels[item.details.value])}`;
  }
  return item.summary;
}

function localizeTaskInScope(item: TransferItem, translate: (source: string) => string): string {
  let summary = item.scopeSummary
    .replace("Global dictionary", translate("全局个人词条"))
    .replace("Direct local rule scope", translate("仅本机规则范围"));
  if (!item.details?.taskId) return summary;
  const debugTask = taskDebugNames[item.details.taskId];
  const localizedTask = translate("任务：{task}").replace("{task}", translate(taskLabels[item.details.taskId]));
  const debugPhrase = `selected task ${debugTask}`;
  return summary.includes(debugPhrase) ? summary.replace(debugPhrase, localizedTask) : `${summary} · ${localizedTask}`;
}

function localizeExistingValue(item: TransferItem, translate: (source: string) => string): string | null {
  const existingValue = item.details?.existingValue;
  if (!existingValue) return null;
  if (existingValue === "local_only") return translate("本机学习规则中已有");
  if (existingValue === "rule_owner") return translate("本机纠正规则");
  if (item.kind === "writing_preference") {
    const match = /^([A-Za-z]+) \(([A-Za-z]+)\)$/.exec(existingValue);
    if (match) {
      const value = preferenceValueDebugNames[match[1]];
      const status = preferenceStatusDebugNames[match[2]];
      if (value && status) return `${translate(valueLabels[value])} · ${translate(preferenceStatusLabels[status])}`;
    }
  }
  return existingValue;
}

function localizeReason(reason: string, translate: (source: string) => string): string {
  return translate(reasonLabels[reason] ?? reason);
}

function createsMappingForSelection(item: TransferItem, choice?: TransferChoice): boolean {
  return item.willCreateMapping === true
    && (item.status === "ready" || (item.status === "conflict" && choice === "replace"));
}

function TransferPager({ page, total, label, onPageChange }: {
  page: number;
  total: number;
  label: string;
  onPageChange: (page: number) => void;
}) {
  const { t } = useI18n();
  const pageCount = Math.ceil(total / PAGE_SIZE);
  if (pageCount < 2) return null;
  const from = page * PAGE_SIZE + 1;
  const to = Math.min(total, (page + 1) * PAGE_SIZE);

  return (
    <nav aria-label={t(label)} className="mt-3 flex flex-wrap items-center justify-between gap-2">
      <span aria-live="polite" className="text-xs tabular-nums text-secondary">
        {t("第 {from}–{to} 项，共 {total} 项")
          .replace("{from}", String(from))
          .replace("{to}", String(to))
          .replace("{total}", String(total))}
      </span>
      <div className="flex gap-2">
        <button type="button" disabled={page === 0} aria-label={t("上一页")} onClick={() => onPageChange(page - 1)} className={compactButtonClass}>{t("上一页")}</button>
        <button type="button" disabled={page + 1 >= pageCount} aria-label={t("下一页")} onClick={() => onPageChange(page + 1)} className={compactButtonClass}>{t("下一页")}</button>
      </div>
    </nav>
  );
}

export function LearningTransferSettings() {
  const { t } = useI18n();
  const operationRef = useRef<Operation | null>(null);
  const [operation, setOperation] = useState<Operation | null>(null);
  const [preview, setPreview] = useState<TransferPreview | null>(null);
  const [previewPage, setPreviewPage] = useState(0);
  const [choices, setChoices] = useState<Record<string, TransferChoice | undefined>>({});
  const [problem, setProblem] = useState<TransferProblem | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [results, setResults] = useState<{ item: TransferItem | undefined; result: TransferItemResult }[] | null>(null);
  const [resultsPage, setResultsPage] = useState(0);
  const [partialOutcome, setPartialOutcome] = useState(false);

  const begin = (next: Operation): boolean => {
    if (operationRef.current) return false;
    operationRef.current = next;
    setOperation(next);
    setProblem(null);
    setFeedback(null);
    return true;
  };

  const finish = () => {
    operationRef.current = null;
    setOperation(null);
  };

  const exportConfiguration = async () => {
    if (!begin("export")) return;
    try {
      const response = await invoke<ExportResponse>("export_learning_transfer");
      if (response.status === "cancelled") {
        setFeedback(t("已取消导出。"));
        return;
      }
      const exportedCount = response.globalWordCount + response.pairCount + response.preferenceCount;
      const summary = t("学习配置已导出：全局词条 {words} 项、纠正 {pairs} 项、偏好 {preferences} 项。")
        .replace("{words}", String(response.globalWordCount))
        .replace("{pairs}", String(response.pairCount))
        .replace("{preferences}", String(response.preferenceCount));
      const noExportedItems = exportedCount === 0 ? ` ${t("这个文件没有可迁移项目。")}` : "";
      const skippedItems = response.skippedCount > 0
        ? ` ${withCount("另有 {count} 项未导出。", response.skippedCount, t)}`
        : "";
      setFeedback(`${summary}${noExportedItems}${skippedItems}`);
    } catch {
      setProblem({ message: t("无法导出学习配置，请重试。"), retry: "export" });
    } finally {
      finish();
    }
  };

  const previewConfiguration = async () => {
    if (!begin("preview")) return;
    // The native picker can invalidate its previous preflight, so do not retain an
    // old preview while opening another file-selection flow.
    setPreview(null);
    setChoices({});
    setResults(null);
    setPartialOutcome(false);
    setPreviewPage(0);
    setResultsPage(0);
    try {
      const response = await invoke<PreviewResponse>("preview_learning_transfer");
      if (response.status === "cancelled") {
        setFeedback(t("已取消选择文件。"));
        return;
      }
      setPreview(response);
      setChoices(Object.fromEntries(response.items
        .filter((item) => item.status === "conflict")
        .map((item) => [item.itemId, item.choiceOptions.includes("keep_local") ? "keep_local" : undefined])));
    } catch (reason) {
      const invalid = isTransferError(reason, "learning_transfer_invalid_file:")
        || isTransferError(reason, "learning_transfer_unsupported_version:");
      setProblem({
        message: t(invalid
          ? "无法读取此学习配置；请选择有效的 VoiceFlow 配置文件。"
          : "无法生成学习配置预览，请重新选择文件或重试。"),
        retry: "preview",
      });
    } finally {
      finish();
    }
  };

  const applyConfiguration = async () => {
    if (!preview || !begin("apply")) return;
    const previewItems = preview.items;
    const choicesForApply = previewItems
      .filter((item) => item.status === "conflict")
      .flatMap((item) => {
        const action = choices[item.itemId];
        return action ? [{ itemId: item.itemId, action }] : [];
      });
    try {
      const response = await invoke<ApplyResponse>("apply_learning_transfer", {
        previewId: preview.previewId,
        choices: choicesForApply,
      });
      setResults(response.results.map((result) => ({
        result,
        item: previewItems.find((candidate) => candidate.itemId === result.itemId),
      })));
      setPartialOutcome(response.status === "partial" || response.results.some((result) =>
        result.status === "failed" || result.reason?.startsWith("learning_transfer_")));
      setResultsPage(0);
      setPreview(null);
      setChoices({});
    } catch (reason) {
      const stale = isTransferError(reason, "learning_transfer_stale_preview:");
      const invalid = isTransferError(reason, "learning_transfer_invalid_file:")
        || isTransferError(reason, "learning_transfer_unsupported_version:");
      setPreview(null);
      setChoices({});
      setProblem({
        message: t(stale
          ? "预览已过期或本机学习配置已变化，请重新预览并检查每项操作。"
          : invalid
            ? "无法读取此学习配置；请选择有效的 VoiceFlow 配置文件。"
            : "导入未完成；请重新预览文件后再试。"),
        retry: "preview",
      });
    } finally {
      finish();
    }
  };

  const retry = () => {
    if (problem?.retry === "export") void exportConfiguration();
    if (problem?.retry === "preview") void previewConfiguration();
  };

  const cancelPreview = () => {
    setPreview(null);
    setChoices({});
    setResults(null);
    setPartialOutcome(false);
    setPreviewPage(0);
    setFeedback(t("已取消预览；本机数据未变更。"));
    setProblem(null);
  };

  const readyCount = preview?.items.filter((item) => item.status === "ready").length ?? 0;
  const selectedReplacementCount = preview?.items.filter((item) => item.status === "conflict"
    && Boolean(choices[item.itemId]) && choices[item.itemId] !== "keep_local").length ?? 0;
  const unresolvedConflictCount = preview?.items.filter((item) => item.status === "conflict" && !choices[item.itemId]).length ?? 0;
  const willImportCount = readyCount + selectedReplacementCount;
  const sourceSummary = preview?.sourceSummary;
  const busy = operation !== null;
  const previewItems = preview?.items.slice(previewPage * PAGE_SIZE, (previewPage + 1) * PAGE_SIZE) ?? [];
  const visibleResults = results?.slice(resultsPage * PAGE_SIZE, (resultsPage + 1) * PAGE_SIZE) ?? [];
  const resultCounts = {
    imported: results?.filter(({ result }) => result.status === "imported").length ?? 0,
    kept: results?.filter(({ result }) => result.status === "kept_local").length ?? 0,
    alreadyPresent: results?.filter(({ result }) => result.status === "already_present").length ?? 0,
    blocked: results?.filter(({ result }) => result.status === "blocked").length ?? 0,
    failed: results?.filter(({ result }) => result.status === "failed").length ?? 0,
  };

  const itemHeader = (item: TransferItem) => (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <span className="text-xs font-medium text-secondary">{t(kindLabels[item.kind])}</span>
      <span aria-hidden="true" className="text-tertiary">·</span>
      <SettingsStatus
        label={t(statusLabels[item.status])}
        tone={item.status === "ready" ? "success" : item.status === "conflict" ? "warning" : item.status === "already_present" ? "neutral" : "error"}
      />
    </div>
  );

  return (
    <>
      {problem && (
        <div className="mt-4">
          <SettingsAlert onRetry={retry} retryLabel={t(problem.retry === "preview" ? "重新选择文件并预览" : "重试")}>
            {problem.message}
          </SettingsAlert>
        </div>
      )}
      <SettingsGroup
        title={t("学习配置迁移")}
        description={t("只迁移全局个人词条、已批准纠正，以及明确保存或批准的偏好。待确认候选、历史记录、录音、审计信息、偏好样例和设备权限不会导出。")}
      >
        <SettingsRow title={t("导出学习配置")} description={t("把当前可迁移的学习配置写入 VoiceFlow JSON 文件。")}>
          <button type="button" disabled={busy} aria-busy={operation === "export"} onClick={() => void exportConfiguration()} className={secondaryButtonClass}>
            {operation === "export" ? <LoaderCircle size={15} className="animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Download size={15} aria-hidden="true" />}
            {t(operation === "export" ? "正在导出…" : "导出学习配置")}
          </button>
        </SettingsRow>
        <SettingsRow title={t("导入学习配置")} description={t("选择 VoiceFlow 配置文件并预览冲突；选定操作前不会更改本机配置。")}>
          <button type="button" disabled={busy} aria-busy={operation === "preview"} onClick={() => void previewConfiguration()} className={secondaryButtonClass}>
            {operation === "preview" ? <LoaderCircle size={15} className="animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Upload size={15} aria-hidden="true" />}
            {t(operation === "preview" ? "正在生成预览…" : preview ? "重新选择文件" : "选择文件并预览")}
          </button>
        </SettingsRow>

        <div className="border-t border-border px-5 py-4">
          <p className="max-w-prose text-xs leading-5 text-secondary">
            {t("导入的全局词条按这台 Mac 当前生效的转写设置使用，与本机手动导入的词条相同。App 范围的纠正和偏好作为本机规则导入；来源机的权限和云端批准不会随文件迁移。")}
          </p>
          {feedback && <p role="status" aria-live="polite" className="mt-3 text-sm text-secondary">{feedback}</p>}

          {preview && (
            <div className="mt-4">
              <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
                <div className="min-w-0">
                  <h3 className="text-sm font-medium text-primary">{t("配置预览")}</h3>
                  {sourceSummary && (
                    <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs tabular-nums text-secondary">
                      <span>{t("全局词条：{count}").replace("{count}", String(sourceSummary.globalWordCount))}</span>
                      <span>{t("纠正：{count}").replace("{count}", String(sourceSummary.pairCount))}</span>
                      <span>{t("偏好：{count}").replace("{count}", String(sourceSummary.preferenceCount))}</span>
                    </div>
                  )}
                </div>
                <p className="min-w-0 max-w-full text-xs tabular-nums text-secondary sm:max-w-[30rem] sm:text-right">
                  {t("预览共 {total} 项：{ready} 项可导入，{conflicts} 项冲突，{alreadyPresent} 项本机已有，{blocked} 项无法导入。")
                    .replace("{total}", String(preview.items.length))
                    .replace("{ready}", String(preview.items.filter((item) => item.status === "ready").length))
                    .replace("{conflicts}", String(preview.items.filter((item) => item.status === "conflict").length))
                    .replace("{alreadyPresent}", String(preview.items.filter((item) => item.status === "already_present").length))
                    .replace("{blocked}", String(preview.items.filter((item) => item.status === "blocked").length))}
                </p>
              </div>

              {preview.items.length === 0 ? (
                <p className="mt-3 border-y border-border py-3 text-sm text-secondary">{t("这个文件没有可迁移项目。")}</p>
              ) : (
                <div className="mt-3 divide-y divide-border border-y border-border">
                  {previewItems.map((item) => {
                    const createsMapping = createsMappingForSelection(item, choices[item.itemId]);
                    return (
                      <div key={item.itemId} className="min-w-0 py-3">
                        {itemHeader(item)}
                        <p className="mt-1 break-words text-sm font-medium text-primary">{itemSummary(item, t)}</p>
                        {item.scopeSummary && <p className="mt-1 break-words text-xs leading-5 text-secondary">{localizeTaskInScope(item, t)}</p>}
                        {((item.status === "conflict" && localizeExistingValue(item, t)) || item.reason === "global_rule_owner_will_be_added") && (
                          <p className="mt-1 break-words text-xs leading-5 text-secondary">
                            {t(item.reason === "global_rule_owner_will_be_added" ? "本机当前来源：{value}" : "本机当前值：{value}")
                              .replace("{value}", localizeExistingValue(item, t) ?? "")}
                          </p>
                        )}
                        {createsMapping && <p className="mt-1 break-words text-xs leading-5 text-secondary">{localizeReason("new_local_mapping", t)}</p>}
                        {item.reason && item.reason !== "new_local_mapping" && <p className={`mt-1 break-words text-xs leading-5 ${item.status === "ready" || item.status === "already_present" ? "text-secondary" : "text-warning-ink"}`}>{localizeReason(item.reason, t)}</p>}
                        {item.status === "conflict" && (
                          <fieldset className="mt-3 flex flex-wrap gap-x-4 gap-y-2" disabled={busy}>
                            <legend className="mb-1 text-xs font-medium text-secondary">{t("选择冲突项目的处理方式")}</legend>
                            {item.choiceOptions.map((action) => (
                              <label key={action} className="inline-flex min-h-10 items-center gap-2 text-xs text-secondary">
                                <input
                                  type="radio"
                                  name={`transfer-choice-${item.itemId}`}
                                  value={action}
                                  checked={choices[item.itemId] === action}
                                  onChange={() => setChoices((current) => ({ ...current, [item.itemId]: action }))}
                                  className={`h-4 w-4 ${focusRingClass}`}
                                />
                                <span>{t(choiceLabels[action])}</span>
                              </label>
                            ))}
                          </fieldset>
                        )}
                        {item.status === "conflict" && choices[item.itemId] === "import_global" && (
                          <p className="mt-2 text-xs leading-5 text-secondary">{t("将此词条加入全局个人词典；以后按本机当前转写设置使用。")}</p>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
              <TransferPager page={previewPage} total={preview.items.length} label="预览项目分页" onPageChange={setPreviewPage} />

              <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
                <p className="min-w-0 basis-48 flex-1 text-xs leading-5 text-secondary">
                  {t("可导入项目会在你点击应用后写入本机。冲突默认保留本机；请逐项检查并选择是否替换。")}
                </p>
                <div className="flex flex-wrap items-center gap-2">
                  <button type="button" onClick={cancelPreview} disabled={busy} className={secondaryButtonClass}>{t("取消预览")}</button>
                  {preview.items.length > 0 && (
                    <button
                      type="button"
                      disabled={busy || willImportCount === 0 || unresolvedConflictCount > 0}
                      aria-busy={operation === "apply"}
                      onClick={() => void applyConfiguration()}
                      className={buttonClass}
                    >
                      {operation === "apply" && <LoaderCircle size={15} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                      {t(operation === "apply" ? "正在导入 {count} 项…" : "将导入 {count} 项到本机").replace("{count}", String(willImportCount))}
                    </button>
                  )}
                </div>
              </div>
              {willImportCount === 0 && preview.items.length > 0 && (
                <p role="status" className="mt-2 text-xs text-secondary">{t("没有可导入的项目。")}</p>
              )}
            </div>
          )}

          {results && (
            <section aria-labelledby="learning-transfer-results-title" className="mt-4">
              <h3 id="learning-transfer-results-title" className="text-sm font-medium text-primary">{t("导入结果")}</h3>
              {partialOutcome && (
                <div role="alert" className="mt-2 rounded-xl border border-warning/30 bg-warning/10 px-3 py-3 text-sm text-warning-ink">
                  {t("导入部分完成；请检查逐项状态和原因，再重新预览后重试未完成项。")}
                </div>
              )}
              {results.length === 0 ? (
                <p className="mt-2 text-sm text-secondary">{t("没有可导入的项目。")}</p>
              ) : (
                <>
                  <p className="mt-2 text-xs tabular-nums text-secondary">
                    {t("结果统计：{imported} 项导入，{kept} 项保留本机，{alreadyPresent} 项本机已有，{blocked} 项被阻止，{failed} 项失败。")
                      .replace("{imported}", String(resultCounts.imported))
                      .replace("{kept}", String(resultCounts.kept))
                      .replace("{alreadyPresent}", String(resultCounts.alreadyPresent))
                      .replace("{blocked}", String(resultCounts.blocked))
                      .replace("{failed}", String(resultCounts.failed))}
                  </p>
                  <div className="mt-2 divide-y divide-border border-y border-border">
                    {visibleResults.map(({ item, result }) => (
                      <div key={result.itemId} className="min-w-0 py-3">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          {item && <span className="text-xs font-medium text-secondary">{t(kindLabels[item.kind])}</span>}
                          <SettingsStatus
                            label={t(resultLabels[result.status])}
                            tone={result.status === "imported" ? "success" : result.status === "failed" || result.status === "blocked" ? "error" : "neutral"}
                          />
                        </div>
                        {item && <p className="mt-1 break-words text-sm font-medium text-primary">{itemSummary(item, t)}</p>}
                        {result.reason && <p className="mt-1 break-words text-xs leading-5 text-secondary">{localizeReason(result.reason, t)}</p>}
                      </div>
                    ))}
                  </div>
                  <TransferPager page={resultsPage} total={results.length} label="导入结果分页" onPageChange={setResultsPage} />
                </>
              )}
              <button type="button" onClick={() => void previewConfiguration()} disabled={busy} className={`${compactButtonClass} mt-3`}>
                {t("重新选择文件并预览")}
              </button>
            </section>
          )}
        </div>
      </SettingsGroup>
    </>
  );
}
