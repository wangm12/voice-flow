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
});
