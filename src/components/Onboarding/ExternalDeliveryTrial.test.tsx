import { selectOption, openSelect } from "../../test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
import rustReceiptFixture from "../../test/fixtures/external-trial-receipt.json";
import { ExternalDeliveryTrial, parseExternalReceipt, type ExternalReceipt } from "./ExternalDeliveryTrial";
import type { TrialLease } from "../../lib/onboardingReadiness";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const permissions = { microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: false };
type SourceIdentity = { kind: "history_revision"; dictation_id: number; revision_id: number; text_sha256: string };
function historySourceIdentity(dictationId: number, revisionId: number): SourceIdentity {
  const textSha256 = revisionId === 21
    ? "b4fedcad8e5b120a98078874173685ef8d21214a6c0053c5bf7366702f6db189"
    : "a9c65768136303f0557b89c7de42393b3f2a6a93946b3a301f83a579f466ac88";
  return { kind: "history_revision", dictation_id: dictationId, revision_id: revisionId, text_sha256: textSha256 };
}
function fixture(lease: TrialLease, targetApp: "text_edit" | "chrome" = "text_edit", sourceIdentity?: SourceIdentity) {
  const selected = "REPLACE ONLY THIS";
  const inserted = "VoiceFlow 外部插入一次。";
  const prefix = targetApp === "chrome"
    ? "VoiceFlow V7 oracle f45d5da3-fe13-445c-b105-acf6a1894c64\n前缀🙂["
    : `controlled ${lease.trial_id}🙂[`;
  const suffix = targetApp === "chrome"
    ? "]后缀\nEnd f45d5da3-fe13-445c-b105-acf6a1894c64"
    : "]";
  return { lease, target_app: targetApp, before: `${prefix}${selected}${suffix}`, selected, inserted, after: `${prefix}${inserted}${suffix}`,
    selection_start_utf16: targetApp === "chrome" ? 62 : prefix.length, preparation_ms: 180_000, switch_ms: 5_000,
    ...(sourceIdentity ? { source_identity: sourceIdentity } : {}) };
}
function receipt(lease: TrialLease, changes: Partial<ExternalReceipt> = {}): ExternalReceipt {
  return { lease, recording_generation: "7", trigger_source: "app_command", trial_current: true,
    permissions: [true, true, false], target: { pid: 42, bundle_id: "com.apple.TextEdit", window_id: "1" },
    readback: "exact_once", clipboard_restoration: "restored", capture_stage: "complete", capture_reason: null, delivery_stage: "entered", reason: null,
    source: "product_same_field_ax_readback_not_independent_oracle", ...changes };
}
let current: TrialLease | null;
let observed: ExternalReceipt | null;
let run: (lease: TrialLease) => Promise<unknown>;
let release: (() => Promise<void>) | null;
let deferHistoryPreparation: boolean;
let pendingHistoryPreparations: Array<{ resolve: (value: unknown) => void; value: unknown; lease: TrialLease }>;
beforeEach(() => {
  current = null; observed = null; release = null; deferHistoryPreparation = false; pendingHistoryPreparations = [];
  run = async (lease) => { observed = receipt(lease); return observed; };
  invokeMock.mockImplementation(async (command, args) => {
    const input = args as { trialId: string; enabled?: boolean; targetApp?: "text_edit" | "chrome"; historySource?: { dictation_id: number; revision_id: number } | null; id?: number };
    const lease = { trial_id: input.trialId, config_generation: "1" };
    if (command === "set_onboarding_test_mode") {
      if (!input.enabled && release) await release();
      return { ...lease, enabled: input.enabled };
    }
    if (command === "get_history") return { items: [{ id: 11, created_at: "2026-10-18", final_text: "original final", raw_text: "raw transcript" }], has_more: false };
    if (command === "get_history_revisions") return [
      { dictation_id: 11, revision_id: 21, created_at: "2026-10-18T10:00:00Z", final_text: "first cleanup revision", cleanup_status: "ai_success", revision_reason: "ai_reclean", intent: { operation: "cleanup", source: "selected_text" } },
      { dictation_id: 11, revision_id: 22, created_at: "2026-10-18T10:01:00Z", final_text: "second cleanup revision", cleanup_status: "ai_success", revision_reason: "ai_reclean", intent: { operation: "cleanup", source: "selected_text" } },
    ];
    if (command === "prepare_onboarding_external") {
      const source = input.historySource ?? null;
      const sourceIdentity = source ? historySourceIdentity(source.dictation_id, source.revision_id) : undefined;
      const value = fixture(lease, input.targetApp, sourceIdentity);
      if (source && deferHistoryPreparation) {
        return new Promise((resolve) => pendingHistoryPreparations.push({ resolve, value, lease }));
      }
      return value;
    }
    if (command === "run_onboarding_external") return run(lease);
    if (command === "confirm_onboarding_external") return observed;
    return undefined;
  });
});
afterEach(async () => { cleanup(); await act(async () => {}); invokeMock.mockReset(); });
function setup(overrides: Partial<Parameters<typeof ExternalDeliveryTrial>[0]> = {}, language: "zh" | "en" = "zh") {
  const onConfirmed = vi.fn(); const onClose = vi.fn();
  const props = { configurationKey: "session:1:permissions:1", permissions, onGeneration: vi.fn(),
    onContinuation: (lease: TrialLease | null) => { current = lease; },
    continuationIsCurrent: (lease: TrialLease) => current?.trial_id === lease.trial_id && current.config_generation === lease.config_generation,
    refreshPermissions: vi.fn(async () => permissions), onConfirmed, onClose, ...overrides };
  return { ...render(<I18nProvider initialLanguage={language}><ExternalDeliveryTrial {...props} /></I18nProvider>), props, onConfirmed, onClose };
}
const copyButton = () => screen.findByRole("button", { name: "复制固定测试文字" });
const runButton = () => screen.getByRole("button", { name: "五秒后测试：现在切换到固定 TextEdit 文档" });
const confirmButton = () => screen.findByRole("button", { name: "返回后重新检查权限并记录本次事实" });
async function chooseUnknownProviderRevision(language: "zh" | "en") {
  const sourceLabel = language === "zh" ? "插入内容来源" : "Insertion source";
  const historyLabel = language === "zh" ? "历史记录" : "History";
  const revisionLabel = language === "zh" ? "成功整理版本" : "Successful cleanup revision";
  selectOption(screen.getByRole("combobox", { name: sourceLabel }), "history_revision");
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_history", expect.any(Object)));
  selectOption(await screen.findByRole("combobox", { name: historyLabel }), "11");
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_history_revisions", { id: 11 }));
  const revisions = await openSelect(screen.getByRole("combobox", { name: revisionLabel }));
  const option = await within(revisions).findByRole("option", {
    name: language === "zh" ? /整理版本 1/ : /Cleanup revision 1/,
  });
  fireEvent.click(option);
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("prepare_onboarding_external", expect.objectContaining({
    historySource: { dictation_id: 11, revision_id: 21 },
  })));
}
describe("controlled external continuation through the actual lease hook", () => {
  it("consumes the Rust-serialized pre-entry receipt and rejects contradictory tuples", () => {
    const raw: unknown = rustReceiptFixture;
    const lease = rustReceiptFixture.lease;
    const parsed = parseExternalReceipt(raw, lease, "text_edit");
    expect(parsed).toMatchObject({
      capture_stage: "focused_element",
      capture_reason: "accessibility_observation_unknown",
      delivery_stage: "not_entered",
      readback: "unknown",
      clipboard_restoration: "not_needed",
    });
    expect(parseExternalReceipt({ ...rustReceiptFixture, delivery_stage: "entered" }, lease, "text_edit")).toBeNull();
    expect(parseExternalReceipt({ ...rustReceiptFixture, readback: "exact_once" }, lease, "text_edit")).toBeNull();
    expect(parseExternalReceipt({ ...rustReceiptFixture, clipboard_restoration: "unknown" }, lease, "text_edit")).toBeNull();
  });

  it("does not copy or deliver on entry; explicit readback remains separate from restore", async () => {
    run = async (lease) => { observed = receipt(lease, { clipboard_restoration: "ownership_changed" }); return observed; };
    const { onConfirmed, props } = setup();
    await copyButton();
    expect(invokeMock.mock.calls.map(([command]) => command)).not.toContain("copy_onboarding_external_fixture");
    expect(invokeMock.mock.calls.map(([command]) => command)).not.toContain("run_onboarding_external");
    fireEvent.click(await copyButton());
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("copy_onboarding_external_fixture", { trialId: current?.trial_id }));
    fireEvent.click(runButton());
    expect(await screen.findByText("固定目标中已回读到恰好一次插入。")).toBeInTheDocument();
    expect(screen.getByText("剪贴板所有权已变化，未覆盖新内容。")).toBeInTheDocument();
    expect(onConfirmed).not.toHaveBeenCalled();
    fireEvent.click(await confirmButton());
    await waitFor(() => expect(onConfirmed).toHaveBeenCalledWith(expect.objectContaining({ readback: "exact_once", clipboard_restoration: "ownership_changed" })));
    expect(props.refreshPermissions).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.some(([name]) => ["start_recording", "preview_writing_mode", "paste_text"].includes(name))).toBe(false);
  });
  it("unknown permission readback cannot confirm or revive a stored result on regrant", async () => {
    const refresh = vi.fn(async () => { current = null; return null; });
    const { onConfirmed, rerender, props } = setup({ refreshPermissions: refresh });
    await copyButton(); fireEvent.click(runButton()); fireEvent.click(await confirmButton());
    await screen.findByText("权限、配置或测试期限已变化；旧结果不能恢复为当前成功，请重新准备。");
    expect(onConfirmed).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.map(([command]) => command)).not.toContain("confirm_onboarding_external");
    rerender(<ExternalDeliveryTrial {...props} permissions={null} configurationKey="session:1:permissions:2" />);
    rerender(<ExternalDeliveryTrial {...props} configurationKey="session:1:permissions:3" />);
    await copyButton();
    expect(screen.queryByText("固定目标中已回读到恰好一次插入。")).not.toBeInTheDocument();
    expect(onConfirmed).not.toHaveBeenCalled();
  });
  it("ignores a late old-lease result after a configuration/permission-window replacement", async () => {
    let done: (value: unknown) => void = () => {};
    let old: TrialLease | null = null;
    run = (lease) => { old = lease; return new Promise((resolve) => { done = resolve; }); };
    const { rerender, props, onConfirmed } = setup();
    await copyButton(); fireEvent.click(runButton());
    await waitFor(() => expect(old).not.toBeNull());
    rerender(<ExternalDeliveryTrial {...props} configurationKey="session:2:permissions:2" />);
    await act(async () => { done(receipt(old!)); });
    await copyButton();
    expect(screen.queryByText("固定目标中已回读到恰好一次插入。")).not.toBeInTheDocument();
    expect(onConfirmed).not.toHaveBeenCalled();
  });
  it("clipboard-only or unconfirmed target never becomes insertion success", async () => {
    run = async (lease) => {
      const inconsistent = receipt(lease, { target: null, readback: "unknown", reason: "target_changed" });
      expect(parseExternalReceipt(inconsistent, lease, "text_edit")).toBeNull();
      observed = receipt(lease, {
        target: null,
        readback: "unknown",
        clipboard_restoration: "not_needed",
        capture_stage: "focused_element",
        capture_reason: "accessibility_observation_unknown",
        delivery_stage: "not_entered",
        reason: "target_unavailable",
      });
      return observed;
    };
    const { onConfirmed } = setup(); await copyButton(); fireEvent.click(runButton());
    expect(await screen.findByText("插入未确认；请检查固定文档，勿盲目重试。")).toBeInTheDocument();
    expect(screen.queryByText("固定目标中已回读到恰好一次插入。")).not.toBeInTheDocument();
    fireEvent.click(await confirmButton());
    await waitFor(() => expect(onConfirmed).toHaveBeenCalledWith(expect.objectContaining({ readback: "unknown", target: null })));
  });
  it("shows native capture and pre-delivery facts without claiming clipboard restoration", async () => {
    run = async (lease) => {
      observed = receipt(lease, {
        target: null, readback: "unknown", clipboard_restoration: "not_needed",
        capture_stage: "focused_element", capture_reason: "accessibility_observation_unknown",
        delivery_stage: "not_entered", reason: "target_unavailable",
      });
      return observed;
    };
    setup(); await copyButton(); fireEvent.click(runButton());
    expect(await screen.findByText("插入未确认；请检查固定文档，勿盲目重试。")).toBeInTheDocument();
    expect(screen.getByText("原生焦点检查结果：辅助功能观察状态无法确认")).toBeInTheDocument();
    expect(screen.getByText("插入交付阶段：尚未进入插入交付流程")).toBeInTheDocument();
    expect(screen.getByText("本次测试无需恢复剪贴板。")).toBeInTheDocument();
  });
  it("rejects inconsistent native-stage facts instead of treating them as a receipt", async () => {
    run = async (lease) => receipt(lease, {
      capture_stage: "unknown_stage" as ExternalReceipt["capture_stage"],
    });
    setup(); await copyButton(); fireEvent.click(runButton());
    expect(await screen.findByText("外部测试结果身份无法确认；请检查目标，不要盲目重试。")).toBeInTheDocument();
    expect(screen.queryByText("固定目标中已回读到恰好一次插入。")).not.toBeInTheDocument();
  });
  it("binds the Chrome trial to its selected native app and accepts only the Chrome receipt", async () => {
    run = async (lease) => {
      observed = receipt(lease, { target: { pid: 42, bundle_id: "com.google.Chrome", window_id: "1" } });
      return observed;
    };
    const { onConfirmed } = setup();
    await copyButton();
    selectOption(screen.getByRole("combobox", { name: "测试目标" }), "chrome");
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("prepare_onboarding_external", expect.objectContaining({ targetApp: "chrome" })));
    await copyButton();
    fireEvent.click(screen.getByRole("button", { name: "五秒后测试：现在切换到受控 Chrome 测试页" }));
    expect(await screen.findByText("固定目标中已回读到恰好一次插入。")).toBeInTheDocument();
    fireEvent.click(await confirmButton());
    await waitFor(() => expect(onConfirmed).toHaveBeenCalledWith(expect.objectContaining({
      target: expect.objectContaining({ bundle_id: "com.google.Chrome" }),
      readback: "exact_once",
    })));
  });
  it("keeps a successful revision with unknown provider provenance selectable without calling it local", async () => {
    setup({}, "zh");
    await chooseUnknownProviderRevision("zh");
    expect(await screen.findByText("将插入所选的已保存整理版本")).toBeInTheDocument();
    expect(screen.queryByText("将插入所选的本机整理版本")).not.toBeInTheDocument();
  });
  it("uses saved-revision wording for unknown provenance in English", async () => {
    setup({}, "en");
    await chooseUnknownProviderRevision("en");
    expect(await screen.findByText("The selected saved cleanup revision will be inserted")).toBeInTheDocument();
    expect(screen.queryByText("The selected local cleanup revision will be inserted")).not.toBeInTheDocument();
  });
  it("invalidates an in-flight source lease across source switches and rejects a receipt for another revision", async () => {
    deferHistoryPreparation = true;
    setup();
    const historyLoadCount = () => invokeMock.mock.calls.filter(([command]) => command === "get_history").length;
    const revisionLoadCount = () => invokeMock.mock.calls.filter(([command]) => command === "get_history_revisions").length;
    const prepareCount = () => invokeMock.mock.calls.filter(([command]) => command === "prepare_onboarding_external").length;
    let priorCount = historyLoadCount();
    selectOption(screen.getByRole("combobox", { name: "插入内容来源" }), "history_revision");
    await waitFor(() => expect(historyLoadCount()).toBeGreaterThan(priorCount));
    priorCount = revisionLoadCount();
    selectOption(await screen.findByRole("combobox", { name: "历史记录" }), "11");
    await waitFor(() => expect(revisionLoadCount()).toBeGreaterThan(priorCount));
    const revisions1 = await openSelect(screen.getByRole("combobox", { name: "成功整理版本" }));
    fireEvent.click(await within(revisions1).findByRole("option", { name: /整理版本 1/ }));
    await waitFor(() => expect(pendingHistoryPreparations).toHaveLength(1));
    const stale = pendingHistoryPreparations[0];

    priorCount = prepareCount();
    selectOption(screen.getByRole("combobox", { name: "插入内容来源" }), "fixed_fixture");
    await waitFor(() => expect(prepareCount()).toBeGreaterThan(priorCount));
    expect(invokeMock).toHaveBeenCalledWith("prepare_onboarding_external", expect.objectContaining({ historySource: null }));
    priorCount = historyLoadCount();
    selectOption(screen.getByRole("combobox", { name: "插入内容来源" }), "history_revision");
    await waitFor(() => expect(historyLoadCount()).toBeGreaterThan(priorCount));
    priorCount = revisionLoadCount();
    selectOption(await screen.findByRole("combobox", { name: "历史记录" }), "11");
    await waitFor(() => expect(revisionLoadCount()).toBeGreaterThan(priorCount));
    const revisions2 = await openSelect(screen.getByRole("combobox", { name: "成功整理版本" }));
    fireEvent.click(await within(revisions2).findByRole("option", { name: /整理版本 2/ }));
    await waitFor(() => expect(pendingHistoryPreparations).toHaveLength(2));
    const currentPreparation = pendingHistoryPreparations[1];
    expect(currentPreparation.lease.trial_id).not.toBe(stale.lease.trial_id);
    expect(invokeMock).toHaveBeenCalledWith("prepare_onboarding_external", expect.objectContaining({
      historySource: { dictation_id: 11, revision_id: 22 },
    }));

    await act(async () => { stale.resolve(stale.value); });
    expect(screen.queryByText("目标仍是本次受控测试文档；插入内容来自上方所选历史版本。")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "五秒后测试：现在切换到固定 TextEdit 文档" })).not.toBeInTheDocument();

    await act(async () => { currentPreparation.resolve(currentPreparation.value); });
    expect(await screen.findByText("目标仍是本次受控测试文档；插入内容来自上方所选历史版本。")).toBeInTheDocument();
    run = async (lease) => {
      observed = receipt(lease, { source_identity: { ...historySourceIdentity(11, 22), text_sha256: "c".repeat(64) } });
      return observed;
    };
    fireEvent.click(screen.getByRole("button", { name: "五秒后测试：现在切换到固定 TextEdit 文档" }));
    expect(await screen.findByText("外部测试结果身份无法确认；请检查目标，不要盲目重试。")).toBeInTheDocument();
    expect(screen.queryByText("固定目标中已回读到恰好一次插入。")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "run_onboarding_external")).toHaveLength(1);
  });
  it("cancel IPC is not release: close waits for the matching native lease receipt", async () => {
    let acknowledge: () => void = () => {};
    const { onClose } = setup(); await copyButton();
    release = () => new Promise((resolve) => { acknowledge = resolve; });
    fireEvent.click(screen.getByRole("button", { name: "关闭外部测试" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("cancel_onboarding_external", expect.any(Object)));
    expect(onClose).not.toHaveBeenCalled(); expect(screen.getByRole("region", { name: "受控外部插入测试" })).toBeInTheDocument();
    await act(async () => { release = null; acknowledge(); });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
});
