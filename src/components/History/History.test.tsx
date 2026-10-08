import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

  it("does not announce an unknown clipboard result as copied", () => {
    render(<History items={[{ ...item, status: "unverified", fallback_reason: "clipboard_ownership_unverified" }]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);
    expect(screen.getByRole("img", { name: "交付未确认" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "已复制" })).not.toBeInTheDocument();
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
    expect(screen.getByText("AI 整理失败，已插入原文")).toBeInTheDocument();
    expect(screen.queryByText("llm_cleanup_failed")).not.toBeInTheDocument();
    expect(screen.queryByText("native.general")).not.toBeInTheDocument();
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

  it("shows clipboard fallback instead of a cleanup failure when the target is unavailable", () => {
    render(
      <History
        items={[{ ...item, status: "copied", fallback_reason: "target_unavailable" }]}
        reload={vi.fn()}
        hasMore={false}
        loading={false}
        onLoadMore={vi.fn()}
      />,
    );

    expect(screen.getByRole("img", { name: "已复制" })).toBeInTheDocument();
    expect(screen.getByText("未能确认输入目标，文字已复制到剪贴板，请手动粘贴")).toBeInTheDocument();
    expect(screen.queryByText("处理未完成，原始转录已保留，可重试")).not.toBeInTheDocument();
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

  it("confirms a dictionary candidate into settings and respects the 256 cap", async () => {
    const existing = Array.from({ length: 256 }, (_, index) => `word-${index}`);
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [zhihuSuggestion];
      if (command === "get_settings") return { dictionary: existing };
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
    fireEvent.click(await screen.findByRole("button", { name: '确认 “知呼 → 知乎”' }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_settings"));
    expect(invokeMock.mock.calls.some(([command]) => command === "promote_learn_pair")).toBe(false);
    expect(invokeMock.mock.calls.some(([command]) => command === "update_settings_patch")).toBe(false);
    expect(await screen.findByRole("alert")).toHaveTextContent("词条无效或已达到上限。");
  });

  it("promotes a second before when the after is already in the dictionary", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return [pythonSuggestion];
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
