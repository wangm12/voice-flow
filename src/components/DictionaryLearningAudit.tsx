import { useId, useRef, useState, type ReactNode } from "react";
import { useI18n } from "../lib/i18n";
import { compactButtonClass, focusRingClass } from "../lib/theme";
import { useDictionaryLearningAudit, type LearningAuditProblem } from "../lib/useDictionaryLearningAudit";
import type { LearningAuditActivity, LearningAuditClassification, LearningAuditPair, LearningAuditPersistence } from "../lib/dictionaryLearningAudit";

const labels: Record<string, string> = {
  name: "人名", term: "术语", rejected: "已拒绝", unchanged_or_empty: "文字相同或为空", case_only: "仅大小写变化", surface_rejected: "不符合可学习词形",
  pending: "待确认", db_promoted: "数据库已记为晋升", tombstoned: "已忽略或撤销", eligible_now: "本次词典与记录状态符合筛选", not_eligible_now: "本次词典或记录状态未通过筛选",
  word_pair: "词语替换", style_preference: "风格偏好", cleanup_intensity: "整理强度",
  in_progress: "进行中", closed: "本次活动已结束", superseded: "已被后续状态取代", unknown: "未知",
  checking: "检查中", started: "已开始观察", skipped: "未开始观察", registered: "同一输入框检查通过", unverified: "插入未核验", read_owner_unavailable: "无法确认输入框读取身份", delivery_owner_mismatch: "插入与输入框读取身份不匹配", read_lease_expired: "输入框读取许可已过期", delivery_fact_mismatch: "插入交付事实不匹配", secure: "安全输入", unreadable: "无法读取输入框",
  stale_or_cancelled: "已过期或取消", no_grant: "学习未获准", protected_target: "受保护的目标", no_mapping: "没有匹配的 App 规则", ax_or_learning_not_allowed: "文字来源或学习未获准", scene_not_allowed: "此场景不允许学习", target_changed: "目标已改变", ambiguous_inserted_range: "插入范围不明确", fixture_baseline_mismatch: "受控基线不匹配",
  not_observed: "未观察", observing: "正在观察", nochange: "未检测到变化", single_token: "检测到短词变化", multiple_tokens: "检测到多个词项变化", short_style_rewrite: "检测到短风格改写", style_signal: "检测到风格信号", ambiguous: "无法明确分类",
  not_attempted: "未尝试", already_db_promoted: "数据库已有晋升记录", tombstone_or_ignored: "保留忽略或撤销记录", cap: "达到候选容量", completed: "该调用已完成", failed: "该调用失败", unknown_scope: "规则范围未知", dictation_origin_unknown: "听写来源未知", scope_conflict: "现有规则范围冲突", duplicate_observation: "已记录同一观察", db_promotion_failed: "数据库晋升失败",
  db_promotion_recorded: "已写入晋升标记", settings_save_completed: "设置保存调用已完成", store_error: "存储调用失败", candidate_db_returned: "候选数据库调用已返回", style_draft_returned: "风格草稿调用已返回", no_settings_change: "没有设置变更", manual_scoped_pair_saved: "App 范围规则已保存",
  add_dictionary_entries: "添加词条", confirm_pair: "确认规则", ignore_pair: "忽略规则", undo_pair: "撤销规则", remove_dictionary_word: "删除词典词条", confirm_style_draft: "确认风格样例", dismiss_style_draft: "忽略风格草稿", add_manual_scoped_pair: "添加 App 范围规则",
  dictionary_command: "词典操作命令", manual_confirmation_with_history_reference: "带历史引用的手动确认", not_provided: "未提供", lookup_succeeded: "历史引用查找成功", lookup_unknown: "历史引用未确认",
  requested: "已请求", stale: "已过期",
};

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0"><dt className="text-tertiary">{label}</dt><dd className="break-words text-secondary">{children}</dd></div>;
}
function Classification({ value }: { value: LearningAuditClassification | null }) {
  const { t } = useI18n();
  return <>{value ? <>{t(labels[value.class])}{value.reason !== null && <> · {t(labels[value.reason])}</>}</> : t("未提供或不适用")}</>;
}
function Persistence({ label, value }: { label: string; value: LearningAuditPersistence }) {
  const { t } = useI18n();
  return <Fact label={label}>{t(labels[value.status])}{value.reason !== null && <> · {t(labels[value.reason])}</>}</Fact>;
}
function Pair({ pair }: { pair: LearningAuditPair }) {
  const { t } = useI18n();
  const scope = pair.recorded_scope;
  return <details className="vf-inline-disclosure rounded-lg border border-border p-3 text-xs leading-5">
    <summary className={`cursor-pointer break-words rounded text-primary ${focusRingClass}`}>
      {t(labels[pair.rule_kind])} · {pair.before_surface === "" ? t("空字符串") : pair.before_surface} → {pair.after_surface === "" ? t("空字符串") : pair.after_surface} · {t(labels[pair.record_state])}
    </summary>
    <dl className="mt-3 grid min-w-0 gap-3 sm:grid-cols-2">
      <Fact label={t("当前分类（本次读取推导）")}>{pair.classification_authority === "not_word_pair" ? t("不适用：不是词语规则") : <Classification value={pair.current_classification} />}</Fact>
      <Fact label={t("候选记录数 / 当前门槛")}>{pair.candidate_hits} / {pair.current_promote_threshold === null ? t("不适用或未知") : pair.current_promote_threshold}</Fact>
      <Fact label={t("原始学习来源")}>{t("未持久化，来源未知")}</Fact>
      <Fact label={t("忽略或撤销原因")}>{pair.tombstone_reason === null ? t("未提供或不适用") : t("旧记录未保存具体原因")}</Fact>
      <Fact label={t("记录的场景（不代表当前授权）")}><dl>{([["场景", scope.family], ["App 规则", scope.mapping_id], ["网站", scope.browser_host], ["App 标识", scope.native_bundle]] as const).map(([label, value]) => <div key={label}><dt className="inline">{t(label)}: </dt><dd className="inline">{value === null ? t("未提供") : value === "" ? t("空字符串") : value}</dd></div>)}</dl></Fact>
      <Fact label={t("词典内包含目标词（读取值）")}>{t(pair.current_dictionary_contains_after ? "是" : "否")}</Fact>
      <Fact label={t("替换筛选投影")}>{t(labels[pair.replacement_eligibility])}</Fact>
      <Fact label={t("现有普通控件投影（不是操作授权）")}>{t("确认")} {t(pair.current_review.confirm ? "可用" : "不可用")} · {t("忽略")} {t(pair.current_review.ignore ? "可用" : "不可用")} · {t("撤销")} {t(pair.current_review.undo ? "可用" : "不可用")}</Fact>
      <Fact label={t("记录最后变更")}>{pair.latest_record_change_at}</Fact>
      <Fact label={t("输出词形匹配标记（不是替换凭据）")}>{pair.last_output_surface_match_at === null ? t("未提供") : pair.last_output_surface_match_at}</Fact>
    </dl>
    <p className="mt-3 text-tertiary">{t("已晋升词语按记录的 App 与场景范围参与现有筛选；当前 App、网站、映射或任务信息缺失或不匹配，或目标保护不允许时不会应用。本页分别展示记录范围、词典成员和数据库标记，不核实当前范围是否匹配，也不证明已实际替换。")}</p>
  </details>;
}
function Activity({ activity, pair }: { activity: LearningAuditActivity; pair: LearningAuditPair | undefined }) {
  const { t } = useI18n();
  const fact = activity.fact;
  return <details className="vf-inline-disclosure rounded-lg border border-border p-3 text-xs leading-5">
    <summary className={`cursor-pointer rounded text-primary ${focusRingClass}`}>
      #{activity.sequence} · {t(fact.kind === "post_paste_attempt" ? "插入后观察尝试" : labels[fact.action])} · {t(labels[activity.lifecycle])}
    </summary>
    <dl className="mt-3 grid min-w-0 gap-3 sm:grid-cols-2">
      <Fact label={t("记录时间（显示值）")}>{activity.created_at}</Fact>
      <Fact label={t("关联存储规则（本次读取投影）")}>{activity.pair_ref === null ? t("未关联；活动不保留词条内容") : pair ? <>{pair.before_surface} → {pair.after_surface}</> : t("关联规则当前未返回")}</Fact>
      {fact.kind === "post_paste_attempt" ? <>
        <Fact label={t("来源")}>{t("插入后的现有输入框观察流程")}</Fact>
        <Fact label={t("开始观察的检查")}>{t(labels[fact.registration.status])} · {fact.registration.reason === null ? t("原因尚未提供") : t(labels[fact.registration.reason])}</Fact>
        <Fact label={t("应用内部插入回执")}>{t(fact.verified_paste ? "内部已核验；不是独立裁判" : "未核验")}</Fact>
        <Fact label={t("观察结果")}>{t(labels[fact.outcome])}</Fact>
        <Fact label={t("本次观察的分类")}>{<Classification value={fact.classification} />}</Fact>
        <Fact label={t("候选决定")}>{t(labels[fact.candidate_decision])}</Fact>
        <Fact label={t("编辑归因")}>{t(fact.attribution === "whole_field_difference_only" ? "只证明整个输入框有变化；不证明识别错误、插入段被纠正或人的意图" : "未观察到归因")}</Fact>
      </> : <>
        <Fact label={t("来源")}>{t(labels[fact.source])}</Fact>
        <Fact label={t("历史引用")}>{t(labels[fact.history_reference])}</Fact>
        <Fact label={t("操作回执")}>{t(labels[fact.outcome])}</Fact>
      </>}
      <Persistence label={t("候选数据库步骤")} value={fact.candidate_store} />
      <Persistence label={t("设置文件步骤")} value={fact.settings_store} />
    </dl>
    <p className="mt-3 text-tertiary">{t("某一步完成不代表所有步骤都成功；失败也不表示没有副作用。历史引用不是识别错误或人工纠正的证明。")}</p>
    <details className="mt-3"><summary className={`cursor-pointer rounded ${focusRingClass}`}>{t("技术来源")}</summary><dl className="mt-2 space-y-2"><Fact label={t("活动标识")}>{activity.activity_id}</Fact><Fact label={t("观察代际")}>{activity.observer_epoch === null ? t("未提供或不适用") : activity.observer_epoch}</Fact></dl></details>
  </details>;
}
function problemText(problem: LearningAuditProblem): string {
  if (problem.code === "pair_projection_capacity") return "规则投影超过有界读取容量，审查暂不可用；没有把部分规则当作完整结果。现有词典控件仍可使用。";
  if (problem.code === "audit_not_available_in_research_mode") return "研究模式不提供普通学习审查；不会读取研究记录补全此页面。";
  if (problem.code === "audit_authority_unavailable" || problem.code === "audit_identity_unavailable") return "学习审查的来源身份无法确认，请稍后手动刷新。";
  if (problem.code === "unsupported_receipt" || problem.code === "invalid_invalidation") return "学习审查收到不支持或不完整的回执；未作为空记录或成功处理。";
  if (problem.code === "outdated_receipt") return "较旧的审查回执已忽略，请手动刷新。";
  if (problem.code === "subscription_unavailable") return "未能接收审查更新提示；显示的读取结果可能已过期。";
  return "学习审查未能读取；已有内容仅供查看，请手动重试。";
}
function AuditContents() {
  const { t } = useI18n();
  const { snapshot, pending, stale, problem, refresh } = useDictionaryLearningAudit();
  const [pairLimit, setPairLimit] = useState(20);
  const [activityLimit, setActivityLimit] = useState(20);
  const pairs = new Map<string, LearningAuditPair>(snapshot?.pairs.map((pair): [string, LearningAuditPair] => [pair.pair_ref, pair]));
  return <div className="space-y-4">
    <p className="text-xs leading-5 text-secondary">{t("本次运行最多保留 128 项无文本活动，重启后不保留；保存的规则仍来自本机词典数据库。阅读不会采集输入框、调用模型或改变学习授权。")}</p>
    <div className="flex flex-wrap items-center gap-3"><button type="button" disabled={pending} aria-busy={pending} onClick={refresh} className={compactButtonClass}>{t(pending ? "正在读取审查…" : "刷新学习审查")}</button><p role="status" className="text-xs text-secondary">{t(pending ? "正在读取；先前内容可能已过期" : stale ? "此审查尚未确认最新；内容仅供查看" : "已读取审查；不是跨存储事务或操作授权")}</p></div>
    {problem && <p role="alert" className="text-xs leading-5 text-error-ink">{t(problemText(problem))}{problem.code === "pair_projection_capacity" && <> {problem.resource} · {problem.limit}</>}</p>}
    {snapshot && <div className="space-y-4" aria-label={t(stale || pending ? "可能过期的学习审查" : "学习审查读取结果")}>
      {(stale || pending) && <p className="rounded-lg border border-warning/30 p-3 text-xs text-warning-ink">{t("以下是可能过期或读取中发生变化的投影，不能作为当前规则、授权或操作依据。")}</p>}
      <p className="text-xs leading-5 text-secondary">{t("稳定版本只指审查元数据版本未变；未观察跨存储修改区间，在途写入数量未知。数据库晋升、设置保存、词典成员和后续替换是不同事实。")}</p>
      <dl className="grid gap-3 text-xs sm:grid-cols-2"><Fact label={t("已移出活动窗口")}>{snapshot.coverage.dropped_entries}</Fact><Fact label={t("已丢失活动更新")}>{snapshot.coverage.dropped_updates}</Fact></dl>
      <section aria-label={t("本次运行的学习活动")}><h3 className="text-sm font-medium text-primary">{t("本次运行的学习活动")}</h3>
        <p className="mt-1 text-xs text-tertiary">{t("显示 {shown} / {total} 项活动").replace("{shown}", String(Math.min(activityLimit, snapshot.activity.length))).replace("{total}", String(snapshot.activity.length))}</p>
        <div className="mt-3 space-y-2">{[...snapshot.activity].reverse().slice(0, activityLimit).map((activity) => <Activity key={activity.activity_id} activity={activity} pair={activity.pair_ref === null ? undefined : pairs.get(activity.pair_ref)} />)}</div>
        {snapshot.activity.length === 0 && <p className="mt-3 text-xs text-secondary">{t("本次运行没有保留的活动；不代表过去没有观察、拒绝或操作。")}</p>}
        {snapshot.activity.length > activityLimit && <button type="button" className={`${compactButtonClass} mt-3`} onClick={() => setActivityLimit((limit) => limit + 20)}>{t("显示更多活动")}</button>}
      </section>
      <section aria-label={t("保存的学习规则审查")}><h3 className="text-sm font-medium text-primary">{t("保存的学习规则审查")}</h3>
        <p className="mt-1 text-xs text-tertiary">{t("显示 {shown} / {total} 条规则；本次读取未截断规则").replace("{shown}", String(Math.min(pairLimit, snapshot.pairs.length))).replace("{total}", String(snapshot.pairs.length))}</p>
        <div className="mt-3 space-y-2">{snapshot.pairs.slice(0, pairLimit).map((pair) => <Pair key={pair.pair_ref} pair={pair} />)}</div>
        {snapshot.pairs.length === 0 && <p className="mt-3 text-xs text-secondary">{t("本次读取没有保存的规则。")}</p>}
        {snapshot.pairs.length > pairLimit && <button type="button" className={`${compactButtonClass} mt-3`} onClick={() => setPairLimit((limit) => limit + 20)}>{t("显示更多规则")}</button>}
      </section>
      <p className="text-xs leading-5 text-secondary">{t("如需确认、忽略或撤销，请使用本页现有普通规则控件；审查不执行或跳转到历史操作。实际使用次数另见学习统计，按词条和本机处理计数，不是识别准确率或成功插入次数。")}</p>
      <details className="text-xs leading-5 text-secondary"><summary className={`cursor-pointer rounded ${focusRingClass}`}>{t("审查范围与技术身份")}</summary><dl className="mt-3 space-y-2"><Fact label={t("运行标识")}>{snapshot.runtime_id}</Fact><Fact label={t("审查版本 / 清除代际 / 观察代际")}>{snapshot.audit_revision} / {snapshot.clear_generation} / {snapshot.observer_epoch}</Fact><Fact label={t("保留序号范围")}>{snapshot.coverage.oldest_sequence === null ? t("未提供或不适用") : snapshot.coverage.oldest_sequence} — {snapshot.coverage.latest_sequence === null ? t("未提供或不适用") : snapshot.coverage.latest_sequence}</Fact><Fact label={t("读取一致性")}>{snapshot.coverage.projection_consistency}</Fact><Fact label={t("规则资源上限")}>1024 · 2097152 UTF-8 · 16777216 JSON</Fact></dl></details>
    </div>}
  </div>;
}

export function DictionaryLearningAudit() {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const id = useId();
  return <section className="mt-6 rounded-xl border border-border bg-card">
    <h2><button ref={trigger} id={`${id}-title`} type="button" aria-expanded={open} aria-controls={id} className={`min-h-11 w-full rounded-xl px-5 py-4 text-left text-sm font-medium text-primary ${focusRingClass}`} onClick={() => setOpen(!open)}>{t("学习审查：来源、原因与保存步骤")}</button></h2>
    <div id={id} role="region" aria-labelledby={`${id}-title`} hidden={!open} inert={!open} className="min-w-0 space-y-4 border-t border-border p-5 [overflow-wrap:anywhere]">
      {open && <AuditContents />}
      <button type="button" className={compactButtonClass} onClick={() => { setOpen(false); if (document.hasFocus()) trigger.current?.focus({ preventScroll: true }); }}>{t("收起学习审查")}</button>
    </div>
  </section>;
}
