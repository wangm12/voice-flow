import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { WritingPreview } from "./WritingPreview";
import type { WritingMode } from "./ContextSettings";
import rejectedFallbackFixture from "../test/fixtures/writing-preview-rejected-fallback.json";
import nativeLocalErrorFixture from "../test/fixtures/native-local-error.json";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const mode: WritingMode = { id: "general", label: "通用", family: "general", prompt: "简洁，保留事实。", builtin: true };

describe("WritingPreview", () => {
  afterEach(cleanup);
  beforeEach(() => { invokeMock.mockReset(); });

  it("sends only an explicit sample and draft, and identifies model versus fallback results", async () => {
    invokeMock.mockResolvedValue({ saved: { text: "已保存输出", status: "model", elapsed_ms: 420 }, draft: { text: "保留 API", status: "guard_fallback", elapsed_ms: 100 } });
    render(<WritingPreview mode={mode} compareSaved />);
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: "保留 API" } });
    fireEvent.click(screen.getByRole("button", { name: "试跑并对比" }));
    expect(await screen.findByText("已保存输出")).toBeInTheDocument();
    expect(screen.getByText("模型结果 · 0.4s")).toBeInTheDocument();
    expect(screen.getByText("事实保护规则触发，已回退 · 0.1s")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("preview_writing_mode", { request: { request_id: expect.any(String), text: "保留 API", mode, compare_saved: true } });
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["preview_writing_mode"]);
  });

  it.each([
    ["empty_completion", "模型返回空内容"], ["truncated", "模型回复被截断"],
    ["missing_stop", "模型回复未完整结束"], ["tool_call", "模型返回了工具调用"],
    ["unknown_future_guard", "模型回复无法按支持的格式确认"],
  ])("preserves shared typed guard %s without claiming model success", async (guard, label) => {
    invokeMock.mockResolvedValue({ saved: null, draft: { text: "保留原文", status: "provider_fallback", elapsed_ms: 5,
      model_candidate: null, provider_error: { kind: "guard", guard }, final_guard_rejected: false } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(new RegExp(label))).toBeInTheDocument();
    expect(screen.queryByText(/^模型结果/)).not.toBeInTheDocument();
  });

  it("separates an available model candidate from final guard fallback and Off", async () => {
    invokeMock.mockResolvedValueOnce({ saved: null, draft: { text: "API 原文", status: "guard_fallback", elapsed_ms: 5,
      model_candidate: "candidate changed API", provider_error: null, final_guard_rejected: true } })
      .mockResolvedValueOnce({ saved: null, draft: { text: "原文", status: "off", elapsed_ms: 0,
        model_candidate: null, provider_error: null, final_guard_rejected: false } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/最终事实保护未接受候选/)).toBeInTheDocument();
    expect(screen.getByText("candidate changed API")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/整理已关闭，未调用模型/)).toBeInTheDocument();
    expect(screen.queryByText("candidate changed API")).not.toBeInTheDocument();
  });

  it("shows a complete provider candidate only with its typed preservation reason", async () => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "保留原稿", status: "guard_fallback", elapsed_ms: 10,
      cleanup_rejection: { candidate: "candidate changed the protected number", reason: "protected_token_changed", stage: "provider_output" },
      provider_error: { kind: "guard", guard: "protected_token_changed" }, final_guard_rejected: false,
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText("保留原稿")).toBeInTheDocument();
    fireEvent.click(screen.getByText("服务商候选（保真检查未采纳）"));
    expect(screen.getByText("candidate changed the protected number")).toBeInTheDocument();
    const rejectedCandidate = screen.getByText("服务商候选（保真检查未采纳）").closest("details");
    expect(rejectedCandidate).not.toBeNull();
    expect(within(rejectedCandidate as HTMLElement).getByText(/名称、数字或事实未通过保真检查/)).toBeInTheDocument();
  });

  it("uses the canonical serialized guard reason in the preview", async () => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "保留准备稿", status: "guard_fallback", elapsed_ms: 5,
      provider_error: { kind: "guard", guard: "explicit_layout_changed" }, final_guard_rejected: false,
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/列表或结构未通过保真检查/)).toBeInTheDocument();
    expect(screen.queryByText("具体原因未确认")).not.toBeInTheDocument();
  });

  it("shows bounded native input admission facts without transcript or transport text", async () => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "保留准备稿", status: "provider_fallback", elapsed_ms: 8,
      provider_error: nativeLocalErrorFixture,
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/^本机整理输入超过允许容量，已在发送前拒绝 · 0\.0s$/)).toBeInTheDocument();
    expect(screen.getByText(/4097 \/ 4096 UTF-8 字节/)).toBeInTheDocument();
    expect(screen.getByText(/输入准入/)).toBeInTheDocument();
    expect(screen.getByText(/已确认请求未发送/)).toBeInTheDocument();
    expect(screen.queryByText(/private transcript|transport body/i)).not.toBeInTheDocument();
  });

  it.each([
    ["a future status", { status_code: "future_status", stage: "input_admission", dispatched: false, input_utf8_bytes: 5000, max_input_bytes: 4096 }],
    ["a contradictory dispatch fact", { status_code: "input_too_large", stage: "input_admission", dispatched: true, input_utf8_bytes: 5000, max_input_bytes: 4096 }],
    ["a malformed count", { status_code: "input_too_large", stage: "input_admission", dispatched: false, input_utf8_bytes: -1, max_input_bytes: 4096 }],
    ["an incomplete native fact", { status_code: "input_too_large", stage: "input_admission", input_utf8_bytes: 5000, max_input_bytes: 4096 }],
  ])("does not infer pre-send refusal from %s", async (_label, nativeFailure) => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "保留准备稿", status: "provider_fallback", elapsed_ms: 8,
      provider_error: { kind: "local_native", guard: null, native_failure: { kind: "local_native", ...nativeFailure } },
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/请求发送状态未确认/)).toBeInTheDocument();
    expect(screen.getByText(/本机整理服务状态未确认/)).toBeInTheDocument();
    expect(screen.queryByText(/已在发送前拒绝/)).not.toBeInTheDocument();
    expect(screen.queryByText(/UTF-8 bytes/)).not.toBeInTheDocument();
  });

  it("shows parsed provider text separately from the shared adapter result and its transformations", async () => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "I will send the report.", status: "model", elapsed_ms: 12,
      provider_candidate: "um, I will send the report",
      model_candidate: "I will send the report",
      cleanup_transformations: ["strip_internal_cleanup_metadata", "restore_spoken_layout"],
      provider_error: null, final_guard_rejected: false,
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText("I will send the report.")).toBeInTheDocument();
    expect(screen.getByText("解析后的服务商候选（共享整理前）")).toBeInTheDocument();
    expect(screen.getByText("um, I will send the report")).toBeInTheDocument();
    expect(screen.getByText("共享整理器接受稿（最终保真检查前）")).toBeInTheDocument();
    expect(screen.getByText("I will send the report", { exact: true })).toBeInTheDocument();
    fireEvent.click(screen.getByText("共享整理转换"));
    expect(screen.getByText("清理整理元数据")).toBeInTheDocument();
    expect(screen.getByText("恢复口述结构")).toBeInTheDocument();
  });

  it("identifies a final-output candidate separately from a provider rejection", async () => {
    invokeMock.mockResolvedValue({ saved: null, draft: {
      text: "保留原稿", status: "guard_fallback", elapsed_ms: 10,
      cleanup_rejection: { candidate: "final candidate", reason: "explicit_layout_changed", stage: "final_output" },
      provider_error: null, final_guard_rejected: true,
    } });
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    fireEvent.click(await screen.findByText("最终整理候选（保真检查未采纳）"));
    expect(screen.getByText("final candidate")).toBeInTheDocument();
    expect(screen.getByText(/列表或结构未通过保真检查/)).toBeInTheDocument();
  });

  it("renders the Rust-produced provider and fallback rejection wire fixture", async () => {
    invokeMock.mockResolvedValue(rejectedFallbackFixture);
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/服务商候选未采纳，本地回退结果也未通过最终保真检查/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("服务商候选（保真检查未采纳）"));
    expect(screen.getByText("Keep API v3.")).toBeInTheDocument();
    expect(screen.getByText("Keep API v2.")).toBeInTheDocument();
  });

  it("cancels an edited draft and ignores its late result", async () => {
    let resolve: (value: unknown) => void = () => {};
    invokeMock.mockImplementation((command) => command === "preview_writing_mode" ? new Promise((done) => { resolve = done; }) : Promise.resolve(undefined));
    const { rerender } = render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    const request = invokeMock.mock.calls[0][1] as { request: { request_id: string } };
    rerender(<WritingPreview mode={{ ...mode, prompt: "用短句。" }} compareSaved />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", { requestId: request.request.request_id }));
    resolve({ saved: null, draft: { text: "过期结果", status: "model", elapsed_ms: 10 } });
    await waitFor(() => expect(screen.getByRole("button", { name: "试跑并对比" })).toBeEnabled());
    expect(screen.queryByText("过期结果")).not.toBeInTheDocument();
  });

  it("shows local-only status honestly and never renders a provider error payload", async () => {
    invokeMock.mockResolvedValueOnce({ saved: null, draft: { text: "本地输出", status: "local_only", elapsed_ms: 1 } }).mockRejectedValueOnce("private provider detail");
    render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByText(/本地处理，未调用模型/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("试跑未完成");
    expect(screen.queryByText("private provider detail")).not.toBeInTheDocument();
  });

  it("blocks empty samples and cancels when closed", async () => {
    invokeMock.mockImplementation(() => new Promise(() => {}));
    const { unmount } = render(<WritingPreview mode={mode} compareSaved={false} />);
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: " " } });
    expect(screen.getByRole("button", { name: "试跑当前语气" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "试跑当前语气" })).toHaveAccessibleDescription("请先填写试跑文本。");
    fireEvent.change(screen.getByLabelText("试跑文本"), { target: { value: "样例" } });
    fireEvent.click(screen.getByRole("button", { name: "试跑当前语气" }));
    unmount();
    expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", expect.any(Object));
  });

  it("returns focus to the run button after a manual cancellation", async () => {
    invokeMock.mockImplementation((command) => command === "preview_writing_mode" ? new Promise(() => {}) : Promise.resolve(undefined));
    render(<WritingPreview mode={mode} compareSaved={false} />);
    const run = screen.getByRole("button", { name: "试跑当前语气" });
    fireEvent.click(run);
    const cancel = screen.getByRole("button", { name: "取消" });
    cancel.focus();
    fireEvent.click(cancel);
    expect(run).toBeEnabled();
    expect(run).toHaveFocus();
    expect(invokeMock).toHaveBeenCalledWith("cancel_writing_preview", expect.any(Object));
  });
});
