import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { I18nProvider } from "../lib/i18n";
import nativeLearningAuditWire from "../test/fixtures/dictionary-learning-audit-native.json";
import { auditObservation, auditPair, auditRuntime, auditSnapshot } from "../test/dictionaryLearningAuditFixtures";
import { DictionaryLearningAudit } from "./DictionaryLearningAudit";
import type { DictionaryLearningAudit as Snapshot } from "../lib/dictionaryLearningAudit";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
let eventHandler: ((event: { payload: unknown }) => void) | undefined;
let read: () => Promise<unknown>;
let release: Mock<UnlistenFn>;
let unexpected: string[];
const triggerName = "学习审查：来源、原因与保存步骤";
function openAudit() { fireEvent.click(screen.getByRole("button", { name: triggerName })); }
function emit(runtime_id: string, audit_revision: string) { act(() => eventHandler?.({ payload: { runtime_id, audit_revision } })); }
function openActivityCard(result: HTMLElement, sequence: number, actionLabel: string) {
  const summary = [...result.querySelectorAll("summary")].find((node) => node.textContent?.trim().startsWith(`#${sequence} · ${actionLabel} ·`));
  if (!summary) throw new Error(`Missing activity ${sequence}`);
  const card = summary.closest("details");
  if (!card) throw new Error(`Activity ${sequence} has no card`);
  fireEvent.click(summary);
  return { summary, card: within(card) };
}
function openPairCard(result: HTMLElement, pairMarker: string) {
  const summary = [...result.querySelectorAll("summary")].find((node) => node.textContent?.includes(pairMarker));
  if (!summary) throw new Error(`Missing pair ${pairMarker}`);
  const card = summary.closest("details");
  if (!card) throw new Error(`Pair ${pairMarker} has no card`);
  fireEvent.click(summary);
  return within(card);
}
function deferred() {
  let resolve!: (value: unknown) => void;
  const promise = new Promise<unknown>((done) => { resolve = done; });
  return { promise, resolve };
}
beforeEach(() => {
  unexpected = [];
  eventHandler = undefined;
  release = vi.fn<UnlistenFn>();
  read = async () => auditSnapshot();
  invokeMock.mockReset();
  listenMock.mockReset();
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_dictionary_learning_audit") return read();
    unexpected.push(command);
    throw new Error(`Unexpected native command: ${command}`);
  });
  listenMock.mockImplementation(async (name, handler): Promise<UnlistenFn> => {
    expect(name).toBe("dictionary_learning_audit://changed");
    eventHandler = handler as (event: { payload: unknown }) => void;
    return release;
  });
});
afterEach(() => { cleanup(); expect(unexpected).toEqual([]); vi.restoreAllMocks(); });

describe("normal Settings learning audit", () => {
  it("renders native manual-scoped outcomes and current skip/decision reasons in Chinese", async () => {
    read = async () => nativeLearningAuditWire;
    render(<DictionaryLearningAudit />);
    openAudit();
    const result = await screen.findByLabelText("学习审查读取结果");
    expect([...result.querySelectorAll("summary")].filter((node) => node.textContent?.includes("添加 App 范围规则"))).toHaveLength(3);
    const requested = openActivityCard(result, 1, "添加 App 范围规则");
    expect(requested.summary).toHaveTextContent("进行中");
    expect(requested.card.getByText("操作回执").parentElement).toHaveTextContent("已请求");
    expect(requested.card.getByText("候选数据库步骤").parentElement).toHaveTextContent("未尝试");
    expect(requested.card.getByText("设置文件步骤").parentElement).toHaveTextContent("未尝试");
    const saved = openActivityCard(result, 2, "添加 App 范围规则");
    expect(saved.summary).toHaveTextContent("本次活动已结束");
    expect(saved.card.getByText("操作回执").parentElement).toHaveTextContent("该调用已完成");
    expect(saved.card.getByText("候选数据库步骤").parentElement).toHaveTextContent("该调用已完成 · App 范围规则已保存");
    expect(saved.card.getByText("设置文件步骤").parentElement).toHaveTextContent("未尝试");
    const failed = openActivityCard(result, 3, "添加 App 范围规则");
    expect(failed.summary).toHaveTextContent("本次活动已结束");
    expect(failed.card.getByText("操作回执").parentElement).toHaveTextContent("该调用失败");
    expect(failed.card.getByText("候选数据库步骤").parentElement).toHaveTextContent("该调用失败 · 存储调用失败");
    expect(failed.card.getByText("设置文件步骤").parentElement).toHaveTextContent("未尝试");
    const baselineSkip = openActivityCard(result, 4, "插入后观察尝试");
    expect(baselineSkip.card.getByText("开始观察的检查").parentElement).toHaveTextContent("未开始观察 · 受控基线不匹配");
    expect(baselineSkip.card.getByText("候选决定").parentElement).toHaveTextContent("未尝试");
    const ambiguousSkip = openActivityCard(result, 5, "插入后观察尝试");
    expect(ambiguousSkip.card.getByText("开始观察的检查").parentElement).toHaveTextContent("未开始观察 · 插入范围不明确");
    expect(ambiguousSkip.card.getByText("候选决定").parentElement).toHaveTextContent("未尝试");
    const scopeConflict = openActivityCard(result, 6, "插入后观察尝试");
    expect(scopeConflict.card.getByText("候选决定").parentElement).toHaveTextContent("现有规则范围冲突");
    const duplicate = openActivityCard(result, 7, "插入后观察尝试");
    expect(duplicate.card.getByText("候选决定").parentElement).toHaveTextContent("已记录同一观察");
    const promotionFailure = openActivityCard(result, 8, "插入后观察尝试");
    expect(promotionFailure.card.getByText("候选决定").parentElement).toHaveTextContent("数据库晋升失败");
    expect(promotionFailure.card.getByText("候选数据库步骤").parentElement).toHaveTextContent("该调用失败 · 存储调用失败");
    const pair = openPairCard(result, "知呼 → 知乎");
    expect(pair.getByText(/已晋升词语按记录的 App 与场景范围/)).toBeVisible();
  });
  it("renders the same native wire with English labels instead of internal enum names", async () => {
    read = async () => nativeLearningAuditWire;
    render(<I18nProvider initialLanguage="en"><DictionaryLearningAudit /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "Learning audit: sources, reasons, and save steps" }));
    const result = await screen.findByLabelText("Learning audit read result");
    expect([...result.querySelectorAll("summary")].filter((node) => node.textContent?.includes("Add app-scoped rule"))).toHaveLength(3);
    const requested = openActivityCard(result, 1, "Add app-scoped rule");
    expect(requested.summary).toHaveTextContent("In progress");
    expect(requested.card.getByText("Operation result").parentElement).toHaveTextContent("Requested");
    expect(requested.card.getByText("Candidate database step").parentElement).toHaveTextContent("Not attempted");
    expect(requested.card.getByText("Settings file step").parentElement).toHaveTextContent("Not attempted");
    const saved = openActivityCard(result, 2, "Add app-scoped rule");
    expect(saved.summary).toHaveTextContent("Activity closed");
    expect(saved.card.getByText("Operation result").parentElement).toHaveTextContent("This call completed");
    expect(saved.card.getByText("Candidate database step").parentElement).toHaveTextContent("This call completed · App-scoped rule saved");
    expect(saved.card.getByText("Settings file step").parentElement).toHaveTextContent("Not attempted");
    const failed = openActivityCard(result, 3, "Add app-scoped rule");
    expect(failed.summary).toHaveTextContent("Activity closed");
    expect(failed.card.getByText("Operation result").parentElement).toHaveTextContent("This call failed");
    expect(failed.card.getByText("Candidate database step").parentElement).toHaveTextContent("This call failed · Store call failed");
    expect(failed.card.getByText("Settings file step").parentElement).toHaveTextContent("Not attempted");
    const baselineSkip = openActivityCard(result, 4, "Post-insertion observation attempt");
    expect(baselineSkip.card.getByText("Observer admission").parentElement).toHaveTextContent("Observation not started · Controlled baseline mismatch");
    expect(baselineSkip.card.getByText("Candidate decision").parentElement).toHaveTextContent("Not attempted");
    const ambiguousSkip = openActivityCard(result, 5, "Post-insertion observation attempt");
    expect(ambiguousSkip.card.getByText("Observer admission").parentElement).toHaveTextContent("Observation not started · Inserted range is ambiguous");
    expect(ambiguousSkip.card.getByText("Candidate decision").parentElement).toHaveTextContent("Not attempted");
    const scopeConflict = openActivityCard(result, 6, "Post-insertion observation attempt");
    expect(scopeConflict.card.getByText("Candidate decision").parentElement).toHaveTextContent("Conflicts with a rule in another scope");
    const duplicate = openActivityCard(result, 7, "Post-insertion observation attempt");
    expect(duplicate.card.getByText("Candidate decision").parentElement).toHaveTextContent("This observation was already recorded");
    const promotionFailure = openActivityCard(result, 8, "Post-insertion observation attempt");
    expect(promotionFailure.card.getByText("Candidate decision").parentElement).toHaveTextContent("Database promotion failed");
    expect(promotionFailure.card.getByText("Candidate database step").parentElement).toHaveTextContent("This call failed · Store call failed");
    const pair = openPairCard(result, "知呼 → 知乎");
    expect(pair.getByText(/Promoted terms participate in existing filters for their recorded app and scene scope/)).toBeVisible();
    expect(result.textContent ?? "").not.toMatch(/manual_scoped_pair_saved|ambiguous_inserted_range|scope_conflict/);
  });
  it("reads only on demand, closes and reopens without a mutation or focus jump", async () => {
    render(<><input aria-label="Other work" /><DictionaryLearningAudit /></>);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
    openAudit();
    const other = screen.getByRole("textbox", { name: "Other work" });
    other.focus();
    await screen.findByLabelText("学习审查读取结果");
    expect(other).toHaveFocus();
    expect(screen.queryByRole("button", { name: "确认" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "撤销" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "收起学习审查" }));
    expect(release).toHaveBeenCalledOnce();
    openAudit();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    await screen.findByLabelText("学习审查读取结果");
    expect(invokeMock.mock.calls.every(([command]) => command === "get_dictionary_learning_audit")).toBe(true);
  });

  it("displays legacy unknowns, non-word tombstones, partial legs and whole-field attribution separately", async () => {
    const observation = auditObservation();
    if (observation.fact.kind !== "post_paste_attempt") throw new Error("fixture kind");
    observation.fact.candidate_decision = "db_promoted";
    observation.fact.settings_store = { status: "failed", reason: "store_error" };
    const pair = auditPair({ rule_kind: "cleanup_intensity", record_state: "tombstoned", current_classification: null, current_promote_threshold: null, classification_authority: "not_word_pair", tombstone_reason: "unknown_not_persisted", candidate_hits: 0 });
    read = async () => auditSnapshot({ pairs: [pair], activity: [observation] });
    render(<DictionaryLearningAudit />);
    openAudit();
    const result = await screen.findByLabelText("学习审查读取结果");
    for (const summary of result.querySelectorAll("summary")) fireEvent.click(summary);
    expect(within(result).getByText("不适用：不是词语规则")).toBeVisible();
    expect(within(result).getByText("未持久化，来源未知")).toBeVisible();
    expect(within(result).getByText("旧记录未保存具体原因")).toBeVisible();
    expect(within(result).getByText(/不证明识别错误、插入段被纠正或人的意图/)).toBeVisible();
    expect(within(result).getByText(/该调用失败/)).toBeVisible();
    expect(within(result).getByText(/候选数据库调用已返回/)).toBeVisible();
    expect(within(result).getByText("0 / 不适用或未知")).toBeVisible();
    expect(result).toHaveTextContent("9007199254740993");
    expect(screen.queryByRole("button", { name: "忘记" })).not.toBeInTheDocument();
  });

  it("coalesces a burst into at most one trailing fetch and never accepts an older receipt", async () => {
    const first = deferred(); const second = deferred();
    let calls = 0;
    read = () => ++calls === 1 ? first.promise : calls === 2 ? second.promise : Promise.resolve(auditSnapshot({ audit_revision: "9007199254741001" }));
    render(<DictionaryLearningAudit />);
    openAudit();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    emit(auditRuntime, "9007199254740998");
    emit(auditRuntime, "9007199254740999");
    await act(async () => first.resolve(auditSnapshot()));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    emit(auditRuntime, "9007199254741001");
    await act(async () => second.resolve(auditSnapshot({ audit_revision: "9007199254740999" })));
    await waitFor(() => expect(screen.getByRole("button", { name: "刷新学习审查" })).toBeEnabled());
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("alert")).toHaveTextContent("较旧");
    fireEvent.click(screen.getByRole("button", { name: "刷新学习审查" }));
    await screen.findByLabelText("学习审查读取结果");
    expect(invokeMock).toHaveBeenCalledTimes(3);
  });

  it("shows a failed manual review with its already completed settings leg and unknown History reference", async () => {
    read = async () => auditSnapshot({ activity: [auditObservation({ observer_epoch: null, pair_ref: null,
      fact: { kind: "review_action", action: "confirm_pair", source: "manual_confirmation_with_history_reference", history_reference: "lookup_unknown", outcome: "failed",
        candidate_store: { status: "failed", reason: "store_error" }, settings_store: { status: "completed", reason: "settings_save_completed" } } })] });
    render(<DictionaryLearningAudit />);
    openAudit();
    const result = await screen.findByLabelText("学习审查读取结果");
    fireEvent.click(within(result).getByText(/#1 · 确认规则/));
    expect(within(result).getByText("历史引用未确认")).toBeVisible();
    expect(within(result).getByText(/设置保存调用已完成/)).toBeVisible();
    expect(within(result).getByText(/存储调用失败/)).toBeVisible();
    expect(within(result).getByText(/失败也不表示没有副作用/)).toBeVisible();
    expect(screen.queryByRole("button", { name: "确认规则" })).not.toBeInTheDocument();
  });

  it("fences runtime changes and ignores late invalidations from a retired runtime", async () => {
    const nextRuntime = "44444444-4444-4444-8444-444444444444";
    render(<DictionaryLearningAudit />);
    openAudit();
    await screen.findByLabelText("学习审查读取结果");
    read = async () => auditSnapshot({ runtime_id: nextRuntime, audit_revision: "1", clear_generation: "0", activity: [] });
    emit(nextRuntime, "1");
    await screen.findByText("本次运行没有保留的活动；不代表过去没有观察、拒绝或操作。");
    expect(invokeMock).toHaveBeenCalledTimes(2);
    emit(auditRuntime, "18446744073709551615");
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("学习审查读取结果")).toHaveTextContent(nextRuntime);
  });

  it("rejects a lower clear generation even when a manual read returns a higher revision", async () => {
    read = async () => auditSnapshot({ clear_generation: "5" });
    render(<DictionaryLearningAudit />);
    openAudit();
    await screen.findByLabelText("学习审查读取结果");
    read = async () => auditSnapshot({ clear_generation: "4", audit_revision: "9007199254740995", activity: [] });
    fireEvent.click(screen.getByRole("button", { name: "刷新学习审查" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("较旧");
    expect(screen.getByLabelText("可能过期的学习审查")).toHaveTextContent("9007199254740994 / 5 / 9007199254740993");
    expect(screen.queryByText("本次运行没有保留的活动；不代表过去没有观察、拒绝或操作。")).not.toBeInTheDocument();
  });

  it("keeps earlier content visibly stale on typed capacity failure, without empty or truncated success", async () => {
    render(<DictionaryLearningAudit />);
    openAudit();
    await screen.findByLabelText("学习审查读取结果");
    read = async () => { throw { code: "pair_projection_capacity", resource: "utf8_bytes", limit: "2097152" }; };
    emit(auditRuntime, "9007199254740995");
    expect(await screen.findByRole("alert")).toHaveTextContent("没有把部分规则当作完整结果");
    expect(screen.getByLabelText("可能过期的学习审查")).toHaveTextContent("星澜协议");
    expect(screen.queryByText("本次读取没有保存的规则。")).not.toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("does not retry malformed invalidations or treat changed-during-read as fresh", async () => {
    const value = auditSnapshot();
    read = async () => ({ ...value, coverage: { ...value.coverage, projection_consistency: "changed_during_read" } });
    render(<DictionaryLearningAudit />);
    openAudit();
    await screen.findByLabelText("可能过期的学习审查");
    act(() => eventHandler?.({ payload: { runtime_id: auditRuntime, audit_revision: 2 } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("不支持或不完整");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(screen.queryByLabelText("学习审查读取结果")).not.toBeInTheDocument();
  });

  it("drops callbacks after close and never imports the old read into a reopened session", async () => {
    const old = deferred();
    read = () => old.promise;
    render(<DictionaryLearningAudit />);
    openAudit();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("button", { name: "收起学习审查" }));
    read = async () => auditSnapshot({ pairs: [], activity: [], clear_generation: "1", audit_revision: "9007199254740995" });
    openAudit();
    await screen.findByLabelText("学习审查读取结果");
    await act(async () => old.resolve(auditSnapshot()));
    expect(screen.queryByText(/星澜协议/)).not.toBeInTheDocument();
    expect(release).toHaveBeenCalledOnce();
    expect(screen.getByText("本次读取没有保存的规则。")).toBeVisible();
  });

  it("keeps unsupported or research-unavailable receipts distinct from empty records", async () => {
    read = async () => { throw { code: "audit_not_available_in_research_mode" }; };
    render(<DictionaryLearningAudit />);
    openAudit();
    expect(await screen.findByRole("alert")).toHaveTextContent("不会读取研究记录");
    read = async () => ({ ...auditSnapshot(), observer_epoch: undefined });
    fireEvent.click(screen.getByRole("button", { name: "刷新学习审查" }));
    expect(await screen.findByText("学习审查收到不支持或不完整的回执；未作为空记录或成功处理。")).toBeVisible();
    expect(screen.queryByText("本次读取没有保存的规则。")).not.toBeInTheDocument();
  });

  it("keeps a successful read stale when update subscription fails, without automatic retry", async () => {
    listenMock.mockRejectedValue(new Error("native listener unavailable"));
    render(<DictionaryLearningAudit />);
    openAudit();
    expect(await screen.findByRole("alert")).toHaveTextContent("未能接收审查更新提示");
    expect(screen.getByLabelText("可能过期的学习审查")).toHaveTextContent("星澜协议");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(1);
  });

  it("offers English evidence labels without reusing actual term counts as per-pair success", async () => {
    const snapshot: Snapshot = auditSnapshot();
    snapshot.coverage.dropped_entries = "9007199254740993";
    snapshot.coverage.dropped_updates = "2";
    read = async () => snapshot;
    render(<I18nProvider initialLanguage="en"><DictionaryLearningAudit /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "Learning audit: sources, reasons, and save steps" }));
    await screen.findByLabelText("Learning audit read result");
    expect(screen.getByText("Evicted activities").parentElement).toHaveTextContent("9007199254740993");
    expect(screen.getByText(/counts terms used in local processing|count terms used in local processing/)).toBeVisible();
    expect(invokeMock.mock.calls.every(([command]) => command === "get_dictionary_learning_audit")).toBe(true);
  });
});
