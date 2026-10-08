import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { IslandWindow } from "./IslandWindow";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const invokeMock = vi.mocked(invoke);

describe("IslandWindow HUD partials", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: true,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });
    listenMock.mockImplementation((event: string, handler: (event: { payload: unknown }) => void) => {
      handlers.set(event, handler);
      return Promise.resolve(vi.fn());
    });
  });

  it.each(["tap", "hold_to_talk"])("exposes the actual %s recording gesture in HUD guidance", async (mode) => {
    render(<IslandWindow />);
    await waitFor(() => expect(handlers.has("dictation://state")).toBe(true));
    act(() => handlers.get("dictation://state")?.({ payload: { state: "recording", session_generation: 1, recording_mode: mode, recording_hotkey: "Fn" } }));
    const status = screen.getByRole("status");
    expect(status.getAttribute("aria-label")).toContain(mode === "tap" ? "再按一次结束" : "松开后结束");
    expect(status.getAttribute("title")).toContain("Esc 取消");
    act(() => handlers.get("dictation://state")?.({ payload: { state: "idle", session_generation: 2 } }));
    expect(screen.getByRole("status", { hidden: true })).not.toHaveAttribute("title");
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
    vi.useRealTimers();
  });

  async function renderHud() {
    render(<IslandWindow />);
    await waitFor(() => expect(handlers.has("dictation://partial")).toBe(true));
  }

  it("keeps prefetch words off the HUD without paste or history commands", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "recording", session_generation: 3, context_label: "WeChat · 口语" },
      });
      handlers.get("dictation://partial")!({
        payload: { session_generation: 3, text: "你好世界" },
      });
    });

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语");
    expect(document.querySelector(".voice-pill-caption")?.textContent ?? "").not.toContain("你好世界");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveClass("voice-pill-caption--partial");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("clears in-progress words on idle", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "recording", session_generation: 3, context_label: "WeChat · 口语" },
      });
      handlers.get("dictation://partial")!({
        payload: { session_generation: 3, text: "你好世界" },
      });
      handlers.get("dictation://state")!({
        payload: { state: "idle", session_generation: 4 },
      });
    });

    expect(screen.queryByText("你好世界")).not.toBeInTheDocument();
  });

  it("composes a Chinese HUD context label from app and style parts", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "recording",
          session_generation: 3,
          context_app: "Cursor",
          context_style: "prompt_or_code",
          context_label: "Cursor · Code",
        },
      });
    });

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · 代码");
  });

  it("shows the actual context source label without the matched rule label", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "processing",
          phase: "cleanup",
          session_generation: 3,
          context_source: {
            source: "ax",
            label: "forged label with private title",
            matched_rule_label: "Gmail personal rule",
          },
        },
      });
    });

    expect(document.querySelector(".voice-pill-context-source")).toHaveTextContent("辅助功能文字");
    expect(document.querySelector(".voice-pill-context-source")).not.toHaveTextContent("forged label");
    expect(screen.queryByText("Gmail personal rule")).not.toBeInTheDocument();
  });

  it("clears the source badge on a new generation and on idle", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "processing",
          session_generation: 3,
          context_source: { source: "ocr", label: "On-device OCR" },
        },
      });
    });
    expect(document.querySelector(".voice-pill-context-source")).toHaveTextContent("本机 OCR");

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "cleanup", session_generation: 4 },
      });
    });
    expect(document.querySelector(".voice-pill-context-source")).not.toBeInTheDocument();

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "idle", session_generation: 4 },
      });
    });
    expect(document.querySelector(".voice-pill-context-source")).not.toBeInTheDocument();
  });

  it("keeps the active app on thinking when a later processing event omits context", async () => {
    await renderHud();
    vi.useFakeTimers();
    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "recording",
          session_generation: 3,
          context_app: "Cursor",
          context_style: "prompt_or_code",
        },
      });
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · 代码");

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "waiting_retry", session_generation: 3 },
      });
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · 代码");

    act(() => {
      vi.advanceTimersByTime(2_500);
    });
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();

    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "processing",
          phase: "cleanup",
          session_generation: 3,
          context_app: "Cursor",
          context_style: "prompt_or_code",
        },
      });
    });
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
    vi.useRealTimers();
  });

  it("shows a promotion undo toast on the island", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    await waitFor(() => expect(handlers.has("learn_pairs://promoted")).toBe(true));
    act(() => {
      handlers.get("learn_pairs://promoted")!({
        payload: { pair_key: "知呼\u001e知乎", before: "知呼", after: "知乎" },
      });
    });

    expect(await screen.findByRole("status")).toHaveTextContent("已学 知呼→知乎");
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_island_learn_interactive", { interactive: true }),
    );
    fireEvent.click(screen.getByRole("button", { name: "撤销" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "知呼\u001e知乎" }),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false }),
    );
  });

  it("undoes both style and intensity pair keys", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    await waitFor(() => expect(handlers.has("learn_pairs://promoted")).toBe(true));
    act(() => {
      handlers.get("learn_pairs://promoted")!({
        payload: {
          pair_key: "learn:style:wechat",
          pair_keys: ["learn:style:wechat", "learn:intensity:down:wechat"],
          before: "好的",
          after: "好的哈哈",
        },
      });
    });

    fireEvent.click(await screen.findByRole("button", { name: "撤销" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "learn:style:wechat" }),
    );
    expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "learn:intensity:down:wechat" });
  });

  it("keeps the native toast size mode open across promotions and releases it on close", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    act(() => handlers.get("learn_pairs://promoted")!({
      payload: { pair_key: "first", before: "旧字", after: "旧词" },
    }));
    expect(await screen.findByRole("status")).toHaveStyle({ height: "38px" });
    act(() => handlers.get("learn_pairs://promoted")!({
      payload: { pair_key: "second", before: "新字", after: "新词" },
    }));
    expect(screen.getByRole("status")).toHaveTextContent("新字→新词");
    expect(invokeMock.mock.calls.filter(([command, args]) =>
      command === "set_island_learn_interactive" && (args as { interactive: boolean }).interactive,
    )).toHaveLength(1);
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(invokeMock).toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });
    expect(invokeMock).toHaveBeenCalledWith("hide_island_if_idle");
  });

  it("preserves an undo error and retry entry, then dismisses only after successful retry", async () => {
    let undoAttempts = 0;
    let finishRetry!: () => void;
    const retry = new Promise<void>((resolve) => { finishRetry = resolve; });
    invokeMock.mockImplementation(async (command) => {
      if (command === "undo_learn_pair") {
        undoAttempts += 1;
        if (undoAttempts === 1) throw new Error("synthetic undo failure");
        return retry;
      }
      return undefined;
    });
    await renderHud();
    act(() => handlers.get("learn_pairs://promoted")!({
      payload: { pair_key: "failed-pair", before: "知呼", after: "知乎" },
    }));
    fireEvent.click(await screen.findByRole("button", { name: "撤销" }));

    expect(await screen.findByText("撤销未完成")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "撤销未完成，请重试。");
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试撤销" })).toBeEnabled();
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });

    fireEvent.click(screen.getByRole("button", { name: "重试撤销" }));
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "撤销" })).toBeDisabled();
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    await act(async () => { finishRetry(); await retry; });
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(invokeMock).toHaveBeenCalledWith("hide_island_if_idle");
    expect(invokeMock).toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });
    expect(undoAttempts).toBe(2);
  });

  it.each(["success", "failure"])(
    "keeps a newer promotion toast when the previous undo ends with %s",
    async (outcome) => {
      let finishUndo!: () => void;
      let failUndo!: (error: Error) => void;
      const pendingUndo = new Promise<void>((resolve, reject) => {
        finishUndo = resolve;
        failUndo = reject;
      });
      invokeMock.mockImplementation(async (command, args) => {
        if (command === "undo_learn_pair" && (args as { pairKey?: string }).pairKey === "old-pair") {
          return pendingUndo;
        }
        return undefined;
      });
      await renderHud();
      act(() => handlers.get("learn_pairs://promoted")!({
        payload: { pair_key: "old-pair", before: "旧字", after: "旧词" },
      }));
      fireEvent.click(await screen.findByRole("button", { name: "撤销" }));
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "old-pair" }));
      act(() => handlers.get("learn_pairs://promoted")!({
        payload: { pair_key: "new-pair", before: "新字", after: "新词" },
      }));
      expect(screen.getByRole("status")).toHaveTextContent("已学 新字→新词");

      await act(async () => {
        if (outcome === "success") finishUndo();
        else failUndo(new Error("synthetic late failure"));
        await pendingUndo.catch(() => undefined);
      });
      expect(screen.getByRole("status")).toHaveTextContent("已学 新字→新词");
      expect(screen.queryByText("撤销未完成")).not.toBeInTheDocument();
      expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
      await waitFor(() => expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "撤销" }));
      await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
      expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "new-pair" });
    },
  );

  it("retains the toast when one of its paired undo commands fails", async () => {
    let failedOnce = false;
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "undo_learn_pair" && (args as { pairKey?: string }).pairKey === "intensity-pair" && !failedOnce) {
        failedOnce = true;
        throw new Error("synthetic second-key failure");
      }
      return undefined;
    });
    await renderHud();
    act(() => handlers.get("learn_pairs://promoted")!({
      payload: { pair_key: "style-pair", pair_keys: ["style-pair", "intensity-pair"], before: "好的", after: "好的哈哈" },
    }));
    fireEvent.click(await screen.findByRole("button", { name: "撤销" }));
    expect(await screen.findByText("撤销未完成")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "style-pair" });
    expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "intensity-pair" });
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    fireEvent.click(screen.getByRole("button", { name: "重试撤销" }));
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(invokeMock.mock.calls.filter(([command]) => command === "undo_learn_pair")).toHaveLength(4);
  });

});
