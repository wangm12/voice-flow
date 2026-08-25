import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TryItStep } from "./TryItStep";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

describe("TryItStep", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    listenMock.mockImplementation((event: string, handler: (event: { payload: unknown }) => void) => {
      handlers.set(event, handler);
      return Promise.resolve(vi.fn());
    });
  });

  afterEach(() => {
    cleanup();
    listenMock.mockReset();
  });

  function renderDictationTrial() {
    return render(
      <TryItStep
        trial="dictation"
        recording={false}
        processing={false}
        hotkeyDisplay="⌘ ⇧ Space"
        selectedActionHotkeyDisplay="⌘ ⇧ /"
        activationMode="tap"
      />,
    );
  }

  it("inserts onboarding dictation results into the trial box", async () => {
    renderDictationTrial();
    await waitFor(() => expect(handlers.has("dictation://onboarding-result")).toBe(true));

    act(() => {
      handlers.get("dictation://onboarding-result")!({
        payload: { final_text: "你好世界" },
      });
    });

    expect(screen.getByLabelText("语音输入试用框")).toHaveValue("你好世界");
  });

  it("keeps clipboard fallback copy after the HUD returns to idle", async () => {
    renderDictationTrial();
    await waitFor(() => expect(handlers.has("dictation://state")).toBe(true));

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "copied", fallback_reason: "target_unavailable" },
      });
      handlers.get("dictation://state")!({
        payload: { state: "idle" },
      });
    });

    expect(screen.getByRole("status")).toHaveTextContent(
      "未能确认输入目标，文字已复制到剪贴板，请手动粘贴",
    );
  });
});
