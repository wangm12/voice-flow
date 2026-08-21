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

    fireEvent.click(screen.getByRole("button", { name: "下载历史记录" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("export_history"));
    expect(await screen.findByRole("status")).toHaveTextContent("已导出到：/Users/test/Downloads/voiceflow-history.json");
  });

  it("clears all local data after confirmation", async () => {
    invokeMock.mockResolvedValue(undefined);
    const reload = vi.fn();
    render(<History items={[item]} reload={reload} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "清空全部数据" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("不会删除 Keychain 中的 API Key");
    fireEvent.click(screen.getByRole("button", { name: "确定继续" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("clear_all_data"));
    expect(reload).toHaveBeenCalledOnce();
  });

  it("surfaces export failures instead of silently losing the action", async () => {
    invokeMock.mockRejectedValue(new Error("disk full"));
    render(<History items={[item]} reload={vi.fn()} hasMore={false} loading={false} onLoadMore={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "下载历史记录" }));

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
    fireEvent.click(screen.getByRole("button", { name: "查看原文" }));

    expect(screen.getByText("清理前")).toBeInTheDocument();
    expect(screen.getByText("hello world")).toBeInTheDocument();
    expect(screen.getByText("清理后")).toBeInTheDocument();
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

    fireEvent.click(screen.getByRole("button", { name: "查看版本" }));

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

    expect(screen.getByRole("img", { name: "已尝试写入" })).toBeInTheDocument();
    expect(screen.getByText("已尝试写入输入框，请确认目标内容")).toBeInTheDocument();
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

  it("shows CJK dictionary candidates from Rust after saving an edit", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return ["知乎"];
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
    expect(await screen.findByRole("button", { name: '确认 “知乎”' })).toBeInTheDocument();
  });

  it("confirms a dictionary candidate into settings and respects the 256 cap", async () => {
    const existing = Array.from({ length: 256 }, (_, index) => `word-${index}`);
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return ["知乎"];
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
    fireEvent.click(await screen.findByRole("button", { name: '确认 “知乎”' }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_settings"));
    expect(invokeMock.mock.calls.some(([command]) => command === "update_settings_patch")).toBe(false);
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

  it("writes a confirmed candidate into the dictionary", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "suggest_dictionary_entries") return ["Python"];
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
    fireEvent.click(await screen.findByRole("button", { name: '确认 “Python”' }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("update_settings_patch", {
        patch: { dictionary: ["VoiceFlow", "Python"] },
      }),
    );
  });
});
