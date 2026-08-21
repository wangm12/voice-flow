import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
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
  });

  async function renderHud() {
    render(<IslandWindow />);
    await waitFor(() => expect(handlers.has("dictation://partial")).toBe(true));
  }

  it("shows prefetch words on the HUD without paste or history commands", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "recording", session_generation: 3, context_label: "WeChat · 口语" },
      });
      handlers.get("dictation://partial")!({
        payload: { session_generation: 3, text: "你好世界" },
      });
    });

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语 · 你好世界");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--partial");
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
});
