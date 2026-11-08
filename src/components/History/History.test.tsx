import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
import { History, type HistoryItem } from "./History";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const item: HistoryItem = {
  id: 2,
  created_at: "2026-08-04 19:00:00",
  raw_text: "hello world",
  final_text: "Hello world.",
  duration: 1.2,
  status: "ok",
};

describe("History", () => {
  it("keeps one date header across appended pages and preserves a row's edit draft", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-04T19:05:00Z"));
    const props = { reload: vi.fn(), hasMore: true, loading: false, onLoadMore: vi.fn() };
    const { rerender } = render(<History {...props} items={[item]} />);
    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    const editor = screen.getByRole("textbox", { name: "编辑整理结果" });
    expect(editor).toHaveFocus();
    fireEvent.change(editor, { target: { value: "a draft to keep" } });
    rerender(<History {...props} items={[item, { ...item, id: 1, created_at: "2026-08-04 18:00:00", final_text: "Earlier today." }]} />);
    expect(screen.getAllByRole("heading", { name: "今天" })).toHaveLength(1);
    expect(screen.getByRole("textbox", { name: "编辑整理结果" })).toBe(editor);
    expect(editor).toHaveValue("a draft to keep");
    rerender(<History {...props} items={[item, { ...item, id: 1, created_at: "2026-08-03 18:00:00", final_text: "Yesterday." }]} />);
    expect(screen.getAllByRole("heading", { name: "昨天" })).toHaveLength(1);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("reports copy success for two seconds and shows a real failure without success feedback", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValueOnce(undefined);
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    const copy = screen.getByRole("button", { name: "复制这条历史记录到剪贴板" });
    await act(async () => fireEvent.click(copy));
    expect(copy).toHaveTextContent("已复制");
    expect(invokeMock).toHaveBeenCalledWith("repaste_history", { id: item.id });
    invokeMock.mockRejectedValueOnce(new Error("Clipboard changed"));
    await act(async () => fireEvent.click(copy));
    expect(screen.getByRole("alert")).toHaveTextContent("Clipboard changed");
    expect(copy).not.toHaveTextContent("已复制");
    invokeMock.mockResolvedValueOnce(undefined);
    await act(async () => fireEvent.click(copy));
    expect(copy).toHaveTextContent("已复制");
    act(() => vi.advanceTimersByTime(2000));
    expect(copy).toHaveTextContent("复制");
    expect(copy).not.toHaveTextContent("已复制");
    invokeMock.mockRejectedValueOnce(new Error("Clipboard unavailable"));
    await act(async () => fireEvent.click(copy));
    expect(screen.getByRole("alert")).toHaveTextContent("Clipboard unavailable");
    expect(copy).not.toHaveTextContent("已复制");
  });

  it("withdraws pending deletion when editing begins", () => {
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    expect(screen.getByRole("button", { name: "确认删除这条历史记录" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    expect(screen.queryByRole("button", { name: "确认删除这条历史记录" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更多操作" })).toBeDisabled();
    expect(invokeMock).not.toHaveBeenCalled();
  });
  it("can cancel an empty edit without saving or acting on the old text", () => {
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "" } });
    expect(screen.getByRole("button", { name: "保存历史版本" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "复制这条历史记录到剪贴板" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "更多操作" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "取消编辑" }));
    expect(screen.queryByRole("textbox", { name: "编辑整理结果" })).not.toBeInTheDocument();
    expect(screen.getByText(item.final_text!)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "编辑这条历史记录" })).toHaveFocus();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("uses Escape to discard the draft without saving a revision", () => {
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    const editor = screen.getByRole("textbox", { name: "编辑整理结果" });
    fireEvent.change(editor, { target: { value: "uncommitted" } });
    fireEvent.keyDown(editor, { key: "Escape", isComposing: true });
    expect(editor).toBeInTheDocument();
    fireEvent.keyDown(editor, { key: "Escape" });
    expect(screen.queryByRole("textbox", { name: "编辑整理结果" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "编辑这条历史记录" })).toHaveFocus();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each(["保存历史版本", "取消编辑"])("cancels editing with Escape from %s and consumes it before window shortcuts", (name) => {
    const windowKeyDown = vi.fn();
    window.addEventListener("keydown", windowKeyDown);
    try {
      render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
      fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
      const editor = screen.getByRole("textbox", { name: "编辑整理结果" });
      expect(editor).toHaveFocus();
      fireEvent.change(editor, { target: { value: "a draft to discard" } });
      const action = screen.getByRole("button", { name });
      action.focus();
      fireEvent.keyDown(action, { key: "Escape" });
      expect(windowKeyDown).not.toHaveBeenCalled();
      expect(screen.queryByRole("textbox", { name: "编辑整理结果" })).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "编辑这条历史记录" })).toHaveFocus();
      fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
      expect(screen.getByRole("textbox", { name: "编辑整理结果" })).toHaveValue(item.final_text);
      expect(invokeMock).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", windowKeyDown);
    }
  });

  it("returns focus to the row menu when deletion is cancelled by button or Escape", () => {
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    const menu = screen.getByRole("button", { name: "更多操作" });
    const requestDelete = () => {
      fireEvent.click(menu);
      fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
      expect(screen.getByRole("button", { name: "取消删除" })).toHaveFocus();
    };
    requestDelete();
    fireEvent.click(screen.getByRole("button", { name: "取消删除" }));
    expect(menu).toHaveFocus();
    expect(screen.queryByRole("button", { name: "确认删除这条历史记录" })).not.toBeInTheDocument();
    requestDelete();
    const confirm = screen.getByRole("button", { name: "确认删除这条历史记录" });
    confirm.focus();
    fireEvent.keyDown(confirm, { key: "Escape" });
    expect(menu).toHaveFocus();
    expect(screen.queryByRole("button", { name: "取消删除" })).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("deletes only after the explicit text confirmation", async () => {
    invokeMock.mockResolvedValue(undefined);
    const reload = vi.fn();
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    const confirm = screen.getByRole("button", { name: "确认删除这条历史记录" });
    expect(confirm).toHaveTextContent("确认删除");
    expect(invokeMock).not.toHaveBeenCalled();
    fireEvent.click(confirm);
    await waitFor(() => expect(reload).toHaveBeenCalledOnce());
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("delete_history", { id: item.id });
  });

  it("moves focus to the next visible row after the deleted item is removed", async () => {
    const rows = [item, { ...item, id: 1, final_text: "Next visible row." }, { ...item, id: 0, final_text: "Older row." }];
    function ControlledHistory() {
      const [items, setItems] = useState(rows);
      return <History items={items} reload={() => setItems((current) => current.filter((entry) => entry.id !== item.id))} hasMore={false} loading={false} onLoadMore={vi.fn()} />;
    }
    invokeMock.mockResolvedValue(undefined);
    render(<ControlledHistory />);
    fireEvent.click(screen.getAllByRole("button", { name: "更多操作" })[0]);
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除这条历史记录" }));
    await waitFor(() => expect(screen.queryByText("Hello world.")).not.toBeInTheDocument());
    const nextRow = document.querySelector(`[data-history-id="1"]`);
    expect(nextRow).toBeInTheDocument();
    expect(nextRow?.querySelector("[aria-haspopup='menu']")).toHaveFocus();
  });

  it("uses the previous visible row after deleting the last row, or search when it was the only row", async () => {
    const earlier = { ...item, id: 1, final_text: "Earlier row." };
    function LastRowHistory() {
      const [items, setItems] = useState([earlier, item]);
      return <History items={items} reload={() => setItems((current) => current.filter((entry) => entry.id !== item.id))} hasMore={false} loading={false} onLoadMore={vi.fn()} />;
    }
    invokeMock.mockResolvedValue(undefined);
    const { unmount } = render(<LastRowHistory />);
    fireEvent.click(screen.getAllByRole("button", { name: "更多操作" })[1]);
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除这条历史记录" }));
    await waitFor(() => expect(document.querySelector(`[data-history-id="1"] [aria-haspopup='menu']`)).toHaveFocus());
    unmount();

    function SoleRowHistory() {
      const [items, setItems] = useState([item]);
      return <History items={items} reload={() => setItems([])} hasMore={false} loading={false} onLoadMore={vi.fn()} />;
    }
    const { container } = render(<SoleRowHistory />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除这条历史记录" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "搜索历史记录" })).toHaveFocus());
    expect(container.querySelector("[data-history-id] ")).not.toBeInTheDocument();
  });

  it("does not reclaim focus after the user moves during deletion, and does not move focus on failure", async () => {
    let finishDelete!: () => void;
    const pendingDelete = new Promise<void>((resolve) => { finishDelete = resolve; });
    invokeMock.mockImplementation((command) => command === "delete_history" ? pendingDelete : Promise.resolve(undefined));
    const rows = [item, { ...item, id: 1, final_text: "Surviving row." }];
    function ControlledHistory() {
      const [items, setItems] = useState(rows);
      return <History items={items} reload={() => setItems((current) => current.filter((entry) => entry.id !== item.id))} hasMore={false} loading={false} onLoadMore={vi.fn()} />;
    }
    render(<ControlledHistory />);
    fireEvent.click(screen.getAllByRole("button", { name: "更多操作" })[0]);
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除这条历史记录" }));
    const search = screen.getByRole("textbox", { name: "搜索历史记录" });
    search.focus();
    await act(async () => { finishDelete(); await pendingDelete; });
    await waitFor(() => expect(screen.queryByText("Hello world.")).not.toBeInTheDocument());
    expect(search).toHaveFocus();

    cleanup();
    invokeMock.mockRejectedValueOnce(new Error("synthetic delete failure"));
    render(<History items={rows} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getAllByRole("button", { name: "更多操作" })[0]);
    fireEvent.click(screen.getByRole("menuitem", { name: "删除记录" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除这条历史记录" }));
    await screen.findByRole("alert");
    expect(document.querySelector(`[data-history-id="1"] [aria-haspopup='menu']`)).not.toHaveFocus();
    expect(document.querySelector(`[data-history-id="2"]`)).toBeInTheDocument();
  });

  it("does not announce an unknown clipboard result as copied", () => {
    render(<History items={[{ ...item, status: "unverified", fallback_reason: "clipboard_ownership_unverified" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    expect(screen.getByRole("img", { name: "交付未确认" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "已复制" })).not.toBeInTheDocument();
  });

  it("keeps legacy cleanup provenance quiet and distinguishes observed not-called from unknown", () => {
    const props = { reload: vi.fn(), hasMore: false, loading: false, onLoadMore: vi.fn() };
    const { rerender } = render(<History {...props} items={[item]} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));
    expect(screen.queryByText("整理过程")).not.toBeInTheDocument();
    rerender(<History {...props} items={[{ ...item, id: item.id + 1, raw_text: "", final_text: "", cleanup_attempt_kind: "not_called", cleanup_transformations: [] }]} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看详情" }));
    expect(screen.getByText("请求状态：未调用整理服务")).toBeInTheDocument();
    expect(screen.queryByText("响应调整：")).not.toBeInTheDocument();
  });

  it("shows actual transformation codes and separate first failure and final guard facts", () => {
    const facts = {
      ...item,
      status: "unverified",
      fallback_reason: "paste_unverified",
      cleanup_attempt_kind: "network",
      cleanup_failure_reason: "network_failure",
      cleanup_final_guard_reason: "final_protected_token_changed",
      cleanup_transformations: ["strip_internal_cleanup_metadata", "restore_spoken_layout", "restore_prepared_preamble"],
    };
    render(<I18nProvider initialLanguage="zh"><History items={[facts]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));
    expect(screen.getByRole("img", { name: "交付未确认" })).toBeInTheDocument();
    expect(screen.getByText(/请求状态：网络连接失败/)).toBeInTheDocument();
    expect(screen.getByText(/整理原因：网络连接失败/)).toBeInTheDocument();
    expect(screen.getByText(/最终保真检查：名称、数字或事实未通过保真检查/)).toBeInTheDocument();
    expect(screen.getByText(/移除整理用内部标记、恢复口述中的列表或结构、恢复本机准备稿中的前置内容/)).toBeInTheDocument();
    expect(screen.queryByText("服务商整理候选")).not.toBeInTheDocument();
  });

  it("localizes cleanup facts in English and does not expose unknown codes", () => {
    render(<I18nProvider initialLanguage="en"><History items={[{ ...item, raw_text: "", final_text: "", cleanup_attempt_kind: "network", cleanup_failure_reason: "network_failure", cleanup_final_guard_reason: "final_protected_token_changed", cleanup_transformations: ["restore_spoken_layout", "unknown_private_code"] }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "View details" }));
    expect(screen.getByText(/Request status: Network connection failed/)).toBeInTheDocument();
    expect(screen.getByText(/Final preservation check: A name, number, or fact did not pass the preservation check/)).toBeInTheDocument();
    expect(screen.getByText(/Restored a spoken list or structure, Other response adjustment/)).toBeInTheDocument();
    expect(screen.queryByText("unknown_private_code")).not.toBeInTheDocument();
  });

  it("shows known native cleanup outcomes with localized causes", () => {
    render(<I18nProvider initialLanguage="en"><History items={[{ ...item, cleanup_attempt_kind: "not_called", cleanup_failure_reason: "native_memory_pressure_critical" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "View original" }));
    expect(screen.getByText(/Request status: Cleanup provider not called/)).toBeInTheDocument();
    expect(screen.getByText(/Cleanup reason: Critical memory pressure; local cleanup was not started/)).toBeInTheDocument();
  });

  it("keeps a rejected cleanup candidate separate while copying the final History result", async () => {
    invokeMock.mockResolvedValue(undefined);
    const rejected = {
      ...item,
      asr_text: "provider ASR text",
      provider_cleaned_candidate: "ASR-side candidate",
      cleanup_rejected_candidate: "unaccepted cleanup candidate",
      cleanup_guard_reason: "provider_protected_token_changed",
    };
    render(<History items={[rejected]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));
    fireEvent.click(screen.getByText("服务商候选（保真检查未采纳）"));
    expect(screen.getByText("unaccepted cleanup candidate")).toBeInTheDocument();
    expect(screen.getByText("ASR-side candidate")).toBeInTheDocument();
    expect(screen.getByText(/名称、数字或事实未通过保真检查/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "复制这条历史记录到剪贴板" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("repaste_history", { id: item.id }));
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  beforeEach(() => {
    invokeMock.mockReset();
    vi.restoreAllMocks();
  });

  it("requests another cursor page from the parent", () => {
    const onLoadMore = vi.fn();
    render(<History items={[item]} reload={vi.fn()} hasMore loading={false} onLoadMore={onLoadMore} />);

    fireEvent.click(screen.getByRole("button", { name: "加载更早的记录" }));

    expect(onLoadMore).toHaveBeenCalledOnce();
  });

  it("exports through the native command and reports the destination", async () => {
    invokeMock.mockResolvedValue("/Users/test/Downloads/voiceflow-history.json");
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "导出记录" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("export_history"));
    expect(await screen.findByText("已导出到：/Users/test/Downloads/voiceflow-history.json")).toHaveAttribute("role", "status");
  });

  it("keeps export feedback in place, prevents duplicate dialogs, and recovers after failure", async () => {
    let rejectExport!: (reason: Error) => void;
    invokeMock.mockImplementationOnce(() => new Promise((_, reject) => { rejectExport = reject; }));
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    const records = screen.getByRole("button", { name: "导出记录" });
    fireEvent.click(records);
    expect(records).toBeDisabled();
    expect(records).toHaveAttribute("aria-busy", "true");
    expect(records).toHaveTextContent("导出中…");
    fireEvent.click(records);
    fireEvent.click(screen.getByRole("button", { name: "导出音频" }));
    expect(invokeMock).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "清空全部数据" })).toBeDisabled();
    rejectExport(new Error("Unable to write export"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Unable to write export");
    expect(records).toBeEnabled();
    expect(records).toHaveTextContent("导出记录");
    expect(records).toHaveAttribute("aria-busy", "false");
  });

  it("clears all local data after confirmation", async () => {
    invokeMock.mockResolvedValue(undefined);
    const reload = vi.fn();
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "清空全部数据" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Keychain 中的 API Key 会保留");
    fireEvent.click(screen.getByRole("dialog").querySelector("button:last-child")!);

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("clear_all_data"));
    expect(reload).toHaveBeenCalledOnce();
  });

  it("keeps clear confirmation pending, prevents duplicate requests, and offers an in-place retry", async () => {
    let reject!: (reason: unknown) => void;
    invokeMock.mockImplementation((command) => command === "clear_all_data" ? new Promise((_resolve, fail) => { reject = fail; }) : Promise.resolve(undefined));
    const reload = vi.fn();
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "清空全部数据" }));
    const confirm = screen.getByRole("dialog").querySelector("button:last-child")!;
    fireEvent.click(confirm); fireEvent.click(confirm);
    expect(confirm).toBeDisabled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "clear_all_data")).toHaveLength(1);
    await act(async () => reject(new Error("disk busy")));
    expect(screen.getByRole("alert")).toHaveTextContent("disk busy");
    expect(reload).not.toHaveBeenCalled();
    invokeMock.mockResolvedValue(undefined);
    fireEvent.click(confirm);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(reload).toHaveBeenCalledOnce();
  });

  it("surfaces export failures instead of silently losing the action", async () => {
    invokeMock.mockRejectedValue(new Error("disk full"));
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "导出记录" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("导出失败：Error: disk full");
  });

  it("shows a readable status and reason instead of an internal error key", () => {
    render(
      <History
        items={[{ ...item, status: "degraded", degraded_reason: "llm_cleanup_failed", context_profile_id: "native.general" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.queryByText("已保留原文")).not.toBeInTheDocument();
    expect(screen.getByText("文字整理失败，原始转录已保留，可重试")).toBeInTheDocument();
    expect(screen.queryByText("llm_cleanup_failed")).not.toBeInTheDocument();
    expect(screen.queryByText("native.general")).not.toBeInTheDocument();
  });

  it("shows the critical-memory raw-ASR notice and reason in History", () => {
    render(
      <History
        items={[{ ...item, status: "degraded", cleanup_status: "critical_memory_pressure", degraded_reason: "critical_memory_pressure" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );
    expect(screen.getByText("内存压力过高，已保留 ASR 原稿")).toBeInTheDocument();
    expect(screen.getByText("系统内存压力过高，未运行本地整理；已保留 ASR 原稿")).toBeInTheDocument();
  });

  it("shows the neutral local-memory raw-ASR notice and reason in History", () => {
    render(
      <History
        items={[{ ...item, status: "degraded", cleanup_status: "local_memory_unavailable", degraded_reason: "local_memory_unavailable" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );
    expect(screen.getByText("本机内存预算不足，已保留 ASR 原稿")).toBeInTheDocument();
    expect(screen.getByText("本机内存预算不足，未运行本地整理；已保留 ASR 原稿")).toBeInTheDocument();
    expect(screen.queryByText(/系统内存压力过高/)).not.toBeInTheDocument();
  });

  it("shows the localized critical-pressure error without treating selected-text reclean as a saved revision", async () => {
    const reload = vi.fn();
    invokeMock.mockRejectedValueOnce(new Error("System memory pressure is critical; selected text and existing History were left unchanged"));
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "重新整理" }));
    fireEvent.click(screen.getByRole("button", { name: "从原文重新整理并复制" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("所选历史文本和已有历史记录均未更改");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("reclean_history", { id: item.id, operation: "cleanup" });
    expect(reload).toHaveBeenCalledOnce();
  });

  it("shows a neutral memory-budget error without treating selected-text reclean as a saved revision", async () => {
    const reload = vi.fn();
    invokeMock.mockRejectedValueOnce(new Error("Local memory budget is unavailable; selected text and existing History were left unchanged"));
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "重新整理" }));
    fireEvent.click(screen.getByRole("button", { name: "从原文重新整理并复制" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("所选历史文本和已有历史记录均未更改");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(reload).toHaveBeenCalledOnce();
  });

  it("keeps the cleaned text primary and lets the user compare the raw transcript", () => {
    render(<History items={[{ ...item, cleanup_status: "ai_success" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    expect(screen.getByText("Hello world.")).toBeInTheDocument();
    expect(screen.getByText("AI 已整理")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));

    expect(screen.getByText("清理前")).toBeInTheDocument();
    expect(screen.getByText("hello world")).toBeInTheDocument();
    expect(screen.getByText("清理后")).toBeInTheDocument();
  });

  it("shows the selected ASR provider and model provenance", () => {
    render(<History items={[{ ...item, engine: "deepgram:nova-3" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    expect(screen.getByText("deepgram:nova-3")).toBeInTheDocument();
  });

  it("shows provider output separately from local transcript preparation", () => {
    render(
      <History
        items={[{ ...item, asr_text: "um, raw provider words", raw_text: "dictionary-replaced words" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));

    expect(screen.getByText("ASR 识别原文")).toBeInTheDocument();
    expect(screen.getByText("um, raw provider words")).toBeInTheDocument();
    expect(screen.getByText("dictionary-replaced words")).toBeInTheDocument();
  });

  it("does not label legacy rows as if they had saved provider output", () => {
    render(<History items={[{ ...item, asr_text: null }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看原文" }));

    expect(screen.getByText("清理前")).toBeInTheDocument();
    expect(screen.queryByText("ASR 识别原文")).not.toBeInTheDocument();
  });

  it("loads and shows revision differences without replacing the parent record", async () => {
    invokeMock.mockResolvedValue([
      {
        revision_id: 8,
        created_at: "2026-08-04 19:01:00",
        final_text: "Hello team.",
        cleanup_status: "ai_success",
        revision_reason: "ai_reclean",
      },
    ]);
    render(<History items={[{ ...item, revision_count: 1 }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "更多操作" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "查看版本" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_history_revisions", { id: item.id }));
    expect(await screen.findByText("版本差异")).toBeInTheDocument();
    expect(screen.getByText("Hello world.")).toBeInTheDocument();
    expect(screen.getByText("Hello team.")).toBeInTheDocument();
    expect(screen.getByText(/AI 重新整理/)).toBeInTheDocument();
  });

  it("renders sqlite utc timestamps as relative time", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-04T19:05:00Z"));
    render(
      <I18nProvider initialLanguage="zh">
        <History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />
      </I18nProvider>,
    );
    expect(screen.getByText(/5 分钟前/)).toBeInTheDocument();
  });

  it("shows the proven clipboard destination and the target cause", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "history",
            delivery_method: "clipboard",
            fallback_reason: "target_unavailable",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "已复制" })).toBeInTheDocument();
    expect(
      screen.getByText("无法确认输入目标；已复制到剪贴板，请手动粘贴"),
    ).toBeInTheDocument();
    expect(screen.queryByText("处理未完成，原始转录已保留，可重试")).not.toBeInTheDocument();
  });

  it("uses the saved History method and keeps target-change copy cause-only", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "copied",
            delivery_method: "history",
            fallback_reason: "target_changed",
            delivery_user_reason: "输入目标已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />
    );

    expect(screen.getByRole("img", { name: "已保存到历史记录" })).toBeInTheDocument();
    expect(screen.getByText("目标已变化")).toBeInTheDocument();
    expect(screen.queryByText(/手动粘贴|⌘V/)).not.toBeInTheDocument();
    expect(screen.queryByText(/已保留在剪贴板/)).not.toBeInTheDocument();
  });

  it("uses a stored History status when a legacy row has no delivery method", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "history",
            delivery_method: null,
            delivery_user_reason: "输入目标已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />
    );

    expect(screen.getByRole("img", { name: "已保存到历史记录" })).toBeInTheDocument();
    expect(screen.queryByText(/已保留在剪贴板/)).not.toBeInTheDocument();
  });

  it("does not turn a failed History write into a saved status", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "failed",
            delivery_method: "history",
            fallback_reason: "history_save_failed",
            delivery_user_reason: "文字已保存到历史记录",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />
    );

    expect(screen.getByRole("img", { name: "识别失败" })).toBeInTheDocument();
    expect(screen.getByText("无法保存到历史记录，请检查存储空间后重试")).toBeInTheDocument();
    expect(screen.queryByText("已保存到历史记录")).not.toBeInTheDocument();
  });

  it("keeps degraded processing visible when the final delivery method was clipboard", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "degraded",
            degraded: true,
            delivery_method: "clipboard",
            fallback_reason: "partial_asr_failure",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />
    );

    expect(screen.getByRole("img", { name: "已保留原文" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "已复制" })).not.toBeInTheDocument();
  });

  it("shows a successful History retry when the user's newer clipboard was preserved", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "history",
            delivery_method: "history",
            fallback_reason: "retry_clipboard_changed",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "已保存到历史记录" })).toBeInTheDocument();
    expect(
      screen.getByText(
        "剪贴板在重试期间已更新，保留了当前内容；转写结果已保存在历史记录，可手动复制",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("自动插入失败，文字已复制到剪贴板")).not.toBeInTheDocument();
  });

  it("keeps degraded retry copy-only messaging truthful when clipboard ownership changes", () => {
    render(
      <History
        items={[
          {
            ...item,
            status: "degraded",
            degraded: true,
            degraded_reason: "llm_cleanup_failed",
            delivery_method: "history",
            fallback_reason: "retry_clipboard_changed",
          },
        ]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "已保留原文" })).toBeInTheDocument();
    expect(
      screen.getByText(
        "文字整理失败，原始转录已保留，可重试 · 剪贴板在重试期间已更新，保留了当前内容；转写结果已保存在历史记录，可手动复制",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/已插入原文/)).not.toBeInTheDocument();
  });

  it("translates History retry clipboard abstention in English", () => {
    render(
      <I18nProvider initialLanguage="en">
        <History
          items={[
            {
              ...item,
              status: "history",
              delivery_method: "history",
              fallback_reason: "retry_clipboard_ownership_unverified",
            },
          ]}
          reload={vi.fn()}
          hasMore={false}
          loading={false}
          onLoadMore={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(
      screen.getByText(
        "Could not confirm whether the clipboard changed during retry; the transcription was saved in History and can be copied manually.",
      ),
    ).toBeInTheDocument();
  });

  it("distinguishes an unverified paste attempt from copied delivery", () => {
    render(
      <History
        items={[{ ...item, status: "unverified", fallback_reason: "paste_unverified" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "交付未确认" })).toBeInTheDocument();
    expect(
      screen.getByText("输入状态无法确认，未重复粘贴；请检查输入框和历史记录"),
    ).toBeInTheDocument();
  });

  it("translates unverified paste copy in English instead of mixing languages", () => {
    render(
      <I18nProvider initialLanguage="en">
        <History
          items={[{ ...item, status: "unverified", fallback_reason: "paste_unverified" }]}
          reload={vi.fn()}
          hasMore={false}
          loading={false}
          onLoadMore={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(
      screen.getByText(
        "The input state could not be confirmed, so paste was not retried. Check the field and History.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("已复制，请按 ⌘V")).not.toBeInTheDocument();
  });

  it("keeps the paste producer's exact uncertainty ahead of a degraded cleanup reason", () => {
    render(
      <History
        items={[{
          ...item,
          status: "unverified",
          degraded: true,
          degraded_reason: "llm_cleanup_failed",
          fallback_reason: "paste_readback_unavailable",
          delivery_user_reason: "已尝试粘贴，但无法读取输入框结果；请检查输入框和历史记录，未重复粘贴",
        }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "交付未确认" })).toBeInTheDocument();
    expect(
      screen.getByText("已尝试粘贴，但无法读取输入框结果；请检查输入框和历史记录，未重复粘贴"),
    ).toBeInTheDocument();
    expect(screen.queryByText("文字整理失败，原始转录已保留，可重试")).not.toBeInTheDocument();
  });

  it("keeps loading older records available while searching", () => {
    const onLoadMore = vi.fn();
    render(<History items={[item]} reload={vi.fn()} hasMore loading={false} onLoadMore={onLoadMore} />);

    fireEvent.change(screen.getByRole("textbox", { name: "搜索历史记录" }), { target: { value: "missing" } });

    fireEvent.click(screen.getByRole("button", { name: "加载更多记录以继续搜索" }));
    expect(onLoadMore).toHaveBeenCalledOnce();
  });

  it("debounces server searches by 200 milliseconds", () => {
    vi.useFakeTimers();
    const onQueryChange = vi.fn();
    render(
      <History
        items={[item]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
        onQueryChange={onQueryChange}
      />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: "搜索历史记录" }), {
      target: { value: "VoiceFlow" },
    });
    expect(onQueryChange).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(199);
    });
    expect(onQueryChange).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(onQueryChange).toHaveBeenCalledWith("VoiceFlow");
  });

  it("distinguishes no global search matches from an unloaded search range", () => {
    const { rerender } = render(
      <History
        items={[item]}
        reload={vi.fn()}
        hasMore
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "搜索历史记录" }), {
      target: { value: "missing" },
    });
    expect(screen.getByText("当前已加载的记录中没有匹配项。")).toBeInTheDocument();

    rerender(
      <History
        items={[]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "搜索历史记录" }), {
      target: { value: "missing" },
    });
    expect(screen.getByText("全库没有匹配的记录。")).toBeInTheDocument();
  });

  it("applies content-visibility to each row in a long list", () => {
    const items = Array.from({ length: 51 }, (_, index) => ({ ...item, id: index + 1 }));
    const { container } = render(
      <History items={items} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />,
    );

    expect(container.querySelectorAll(".history-list--windowed")).toHaveLength(51);
  });

  it("shows an explicit initial loading state", () => {
    render(<History items={[]} reload={vi.fn()} hasMore={false} loading={true} onLoadMore={vi.fn()} />);

    expect(screen.getByRole("status", { name: "正在加载历史记录…" })).toBeInTheDocument();
    expect(screen.queryByText("还没有记录，按热键说一句吧")).not.toBeInTheDocument();
  });

  const zhihuSuggestion = { pair_key: "知呼\u001e知乎", before_span: "知呼", after: "知乎" };
  const pythonSuggestion = { pair_key: "配森\u001epython", before_span: "配森", after: "Python" };

  it("shows CJK dictionary candidates from Rust after saving an edit", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [zhihuSuggestion];
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("suggest_dictionary_entries", { before: "知呼", after: "知乎" }),
    );
    expect(await screen.findByRole("button", { name: '确认 “知呼 → 知乎”' })).toBeInTheDocument();
  });

  it("keeps a manual edit open after learning fails and retries the same save", async () => {
    invokeMock
      .mockRejectedValueOnce("history_dictionary_learning_retry")
      .mockResolvedValueOnce({ revision_id: 41, changed: false, learning: { status: "promoted" } })
      .mockResolvedValueOnce([]);
    const reload = vi.fn();
    render(
      <History
        items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]}
        reload={reload}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    const editor = screen.getByRole("textbox", { name: "编辑整理结果" });
    fireEvent.change(editor, { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "历史版本已保存，但词典学习暂时未完成；请再次保存重试。",
    );
    expect(screen.getByRole("textbox", { name: "编辑整理结果" })).toHaveValue("知乎");
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

    await waitFor(() => expect(screen.queryByRole("textbox", { name: "编辑整理结果" })).not.toBeInTheDocument());
    expect(invokeMock).toHaveBeenNthCalledWith(1, "save_history_revision", {
      id: item.id,
      final_text: "知乎",
      revision_reason: "manual_edit",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "save_history_revision", {
      id: item.id,
      final_text: "知乎",
      revision_reason: "manual_edit",
    });
    expect(reload).toHaveBeenCalledOnce();
  });

  it("reports when a scoped dictionary candidate is still collecting edits", async () => {
    invokeMock
      .mockResolvedValueOnce({ revision_id: 42, changed: true, learning: { status: "pending", hits: 1, required: 2 } })
      .mockResolvedValueOnce([]);
    render(<History items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

    expect(await screen.findByText("历史版本已保存；词典候选仍在收集中 (1/2)")).toHaveAttribute("role", "status");
  });

  it.each(["partial_settings_failure", "partial_dictionary_sync_failure"])(
    "keeps a saved History revision retryable when dictionary synchronization is partial (%s)",
    async (status) => {
      invokeMock
        .mockResolvedValueOnce({ revision_id: 43, changed: true, learning: { status } })
        .mockResolvedValueOnce([]);
      render(<History items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

      fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
      const editor = screen.getByRole("textbox", { name: "编辑整理结果" });
      fireEvent.change(editor, { target: { value: "知乎" } });
      fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

      expect(await screen.findByText("历史版本已保存；个人词典同步未完成，请再次保存重试。"))
        .toHaveAttribute("role", "status");
      expect(screen.getByRole("textbox", { name: "编辑整理结果" })).toHaveValue("知乎");
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    },
  );

  it("shows backend dictionary capacity rejection and leaves the candidate available", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [zhihuSuggestion];
      if (command === "promote_learn_pair") throw "dictionary_capacity_or_invalid_word";
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));
    const candidateButton = await screen.findByRole("button", { name: '确认 “知呼 → 知乎”' });
    fireEvent.click(candidateButton);

    expect(await screen.findByRole("alert")).toHaveTextContent("词条无效或已达到上限。");
    expect(invokeMock).toHaveBeenCalledWith("promote_learn_pair", {
      pairKey: zhihuSuggestion.pair_key,
      beforeSurface: "知呼",
      afterSurface: "知乎",
      historyId: item.id,
    });
    expect(invokeMock.mock.calls.some(([command]) => command === "get_settings")).toBe(false);
    expect(screen.getByRole("button", { name: '确认 “知呼 → 知乎”' })).toBeEnabled();
  });

  it("lets the backend resolve casing against the current dictionary", async () => {
    const caseVariantSuggestion = { ...pythonSuggestion, after: "python" };
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [caseVariantSuggestion];
      if (command === "get_settings") return { dictionary: ["Python"] };
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "配森", final_text: "配森", context_profile_id: "chat.personal" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "Python" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));
    fireEvent.click(await screen.findByRole("button", { name: '确认 “配森 → python”' }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("promote_learn_pair", {
        pairKey: caseVariantSuggestion.pair_key,
        beforeSurface: "配森",
        afterSurface: "python",
        historyId: item.id,
      }),
    );
    expect(invokeMock.mock.calls.some(([command]) => command === "get_settings")).toBe(false);
  });

  it("keeps a dictionary candidate available to retry after a partial promotion", async () => {
    let promoteAttempts = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [zhihuSuggestion];
      if (command === "promote_learn_pair") {
        promoteAttempts += 1;
        if (promoteAttempts === 1) throw "learning_dictionary_partial:settings_save_failed";
      }
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));
    const candidateName = '确认 “知呼 → 知乎”';
    fireEvent.click(await screen.findByRole("button", { name: candidateName }));

    expect(await screen.findByRole("alert")).toHaveTextContent("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
    expect(screen.getByRole("button", { name: candidateName })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: candidateName }));
    await waitFor(() => expect(promoteAttempts).toBe(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: candidateName })).not.toBeInTheDocument());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("hides dictionary suggestions when learning is disabled", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [];
      if (command === "get_settings") return { dictionary: [], dictionary_learn_enabled: false };
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "知呼", final_text: "知呼" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "知乎" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("suggest_dictionary_entries", { before: "知呼", after: "知乎" }),
    );
    expect(screen.queryByText("可能的词典建议")).not.toBeInTheDocument();
  });

  it("exports saved audio through the native command", async () => {
    invokeMock.mockResolvedValue("/Users/test/Downloads/voiceflow-gold");
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "导出音频" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("export_gold_corpus"));
    expect(await screen.findByText("已导出音频到：/Users/test/Downloads/voiceflow-gold")).toHaveAttribute("role", "status");
  });

  it("writes a confirmed candidate into the dictionary", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [pythonSuggestion];
      if (command === "get_settings") return { dictionary: ["VoiceFlow"] };
      return undefined;
    });
    render(
      <History
        items={[{ ...item, raw_text: "配森", final_text: "配森" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "编辑这条历史记录" }));
    fireEvent.change(screen.getByRole("textbox", { name: "编辑整理结果" }), { target: { value: "Python" } });
    fireEvent.click(screen.getByRole("button", { name: "保存历史版本" }));
    fireEvent.click(await screen.findByRole("button", { name: '确认 “配森 → Python”' }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("promote_learn_pair", {
        pairKey: pythonSuggestion.pair_key,
        beforeSurface: "配森",
        afterSurface: "Python",
        historyId: item.id,
      }),
    );
  });
});
