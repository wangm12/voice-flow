import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

  it("shows an explicit initial loading state", () => {
    render(<History items={[]} reload={vi.fn()} hasMore={false} loading={true} onLoadMore={vi.fn()} />);

    expect(screen.getByRole("status", { name: "正在加载历史记录…" })).toBeInTheDocument();
    expect(screen.queryByText("还没有记录，按热键说一句吧")).not.toBeInTheDocument();
  });
});
