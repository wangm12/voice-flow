import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { audioCapturePresentation, IslandWindow, recoveryActionLabelForOutcome, recoveryDestinationForOutcome, recoveryMessageForOutcome } from "./IslandWindow";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { emitMock, listenMock } = vi.hoisted(() => ({ emitMock: vi.fn(), listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ emit: emitMock, listen: listenMock }));

const invokeMock = vi.mocked(invoke);

function recoveryPayload(
  session_generation: number,
  outcome: "history_saved" | "audio_retained" | "unknown_failure" | "no_audio",
  recoverable_session_id: string | null = null,
  ticket_revision = 1,
  save_error = outcome === "audio_retained",
) {
  return { ticket_revision, session_generation, outcome, recoverable_session_id, save_error };
}

function historyRecoveryPayload(
  session_generation: number,
  ticket_revision = 1,
  copy_status: "ready" | "copying" | "copied" | "unknown" | "failed" = "ready",
  history_state: "pending" | "saved" | "failed" = "failed",
) {
  return {
    ticket_revision,
    session_generation,
    delivery_transaction_id: "0123456789abcdef0123456789abcdef",
    history_transaction_id: "dictation-1234-abc-1",
    history_state,
    copy_status,
  };
}

function expectRecoveryCopy(message: string) {
  const announcement = document.querySelector(".voice-pill .voice-pill__sr-only[role='status']");
  expect(announcement).toBeInTheDocument();
  expect(announcement).toHaveAttribute("aria-live", "polite");
  expect(announcement).toHaveTextContent(message);
  expect(screen.getByText(message, { selector: ".voice-pill__recovery-message" })).toBeInTheDocument();
}

function learnToast() {
  return document.querySelector<HTMLElement>(".island-learn-toast");
}

async function findLearnToast() {
  await waitFor(() => expect(learnToast()).toBeInTheDocument());
  return learnToast()!;
}

describe("audio capture display ownership", () => {
  const translate = (source: string) => source;

  it("keeps live status, effective device, selected device, and unknown separate", () => {
    const capture = {
      session_generation: 7,
      status: "receiving_audio" as const,
      device_name: "AirPods Pro",
      input_selection: "MacBook Pro Microphone",
    };
    expect(audioCapturePresentation(capture, "recording", translate)).toEqual({
      status: "正在接收麦克风音频",
      deviceName: "AirPods Pro",
      deviceSource: "effective",
      deviceSourceLabel: "实际",
      selectionLabel: "所选：MacBook Pro Microphone",
      deviceTitle: "实际录音设备：AirPods Pro · 所选输入设备：MacBook Pro Microphone",
    });
    expect(audioCapturePresentation(capture, "processing", translate)).toEqual({
      status: null,
      deviceName: "AirPods Pro",
      deviceSource: "effective",
      deviceSourceLabel: "实际",
      selectionLabel: "所选：MacBook Pro Microphone",
      deviceTitle: "实际录音设备：AirPods Pro · 所选输入设备：MacBook Pro Microphone",
    });
    expect(audioCapturePresentation({ ...capture, device_name: null }, "processing", translate)).toMatchObject({
      status: null,
      deviceName: "MacBook Pro Microphone",
      deviceSource: "selected",
      deviceSourceLabel: "所选",
      selectionLabel: "所选：MacBook Pro Microphone",
    });
    expect(audioCapturePresentation(null, "processing", translate)).toEqual({
      status: null,
      deviceName: null,
      deviceSource: "unknown",
      deviceSourceLabel: "未知",
      selectionLabel: "默认输入",
      deviceTitle: "录音设备未知 · 默认输入",
    });
  });
});

describe("audio recovery actions", () => {
  it.each([
    ["history_saved", "history", "录音已保存到历史记录。"],
    ["audio_retained", "history", "恢复音频已保留，但未能确认历史保存。"],
    ["unknown_failure", "history", "录音恢复状态未知，请检查历史记录。"],
    ["no_audio", "microphone", "麦克风无法打开；更改输入设备后重试。"],
  ] as const)("keeps %s recovery scoped to its safe destination", (outcome, destination, message) => {
    expect(recoveryDestinationForOutcome(outcome)).toBe(destination);
    expect(recoveryMessageForOutcome(outcome)).toBe(message);
  });

  it("shows a retained-session save failure without claiming an unverified History write", () => {
    expect(recoveryMessageForOutcome("audio_retained", true)).toBe("未能保存到历史，恢复音频仍已保留。请重试。");
  });

  it("offers an exact-session History save when audio was retained", () => {
    expect(recoveryActionLabelForOutcome("audio_retained")).toBe("保存到历史");
    expect(recoveryActionLabelForOutcome("unknown_failure")).toBe("检查历史");
  });
});

describe("IslandWindow HUD partials", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    emitMock.mockReset();
    emitMock.mockResolvedValue(undefined);
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
    const pill = document.querySelector(".voice-pill");
    expect(pill?.getAttribute("title")).toContain(mode === "tap" ? "再按一次结束" : "松开后结束");
    expect(pill?.getAttribute("title")).toContain("Esc 取消");
    act(() => handlers.get("dictation://state")?.({ payload: { state: "idle", session_generation: 2 } }));
    expect(document.querySelector(".voice-pill")?.getAttribute("title")).toBeNull();
  });

  it("shows clamshell substitution and the resolved system default while ignoring stale generations", async () => {
    await renderHud();
    await waitFor(() => expect(handlers.has("audio://capture")).toBe(true));
    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 18 } });
      handlers.get("audio://capture")!({ payload: {
        session_generation: 18,
        status: "receiving_audio",
        device_name: "External Microphone",
        requested_selection: "MacBook Pro Microphone",
        clamshell_substituted: true,
        system_default_name: "External Microphone",
      } });
    });
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("External Microphone");
    expect(document.querySelector(".voice-pill__subtitle")?.getAttribute("title")).toContain("实际录音设备：External Microphone");
    expect(document.querySelector(".voice-pill__subtitle")?.getAttribute("title")).toContain("所选输入设备：MacBook Pro Microphone");

    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 19 } });
      handlers.get("audio://capture")!({ payload: {
        session_generation: 19,
        status: "receiving_audio",
        device_name: "MacBook Pro Microphone",
        requested_selection: "",
        clamshell_substituted: false,
        system_default_name: "System Default Headset",
      } });
    });
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("MacBook Pro Microphone");
    expect(document.querySelector(".voice-pill__subtitle")?.getAttribute("title")).toContain("默认输入：System Default Headset");

    act(() => handlers.get("audio://capture")!({ payload: {
      session_generation: 18,
      status: "device_error",
      device_name: "Stale Microphone",
      requested_selection: "Old Device",
    } }));
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("MacBook Pro Microphone");
    expect(document.querySelector(".voice-pill__subtitle")?.getAttribute("title")).toContain("默认输入：System Default Headset");
  });

  it("shows the opened microphone and current input status, ignoring stale device events", async () => {
    await renderHud();
    await waitFor(() => expect(handlers.has("audio://capture")).toBe(true));
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "starting", session_generation: 7 },
      });
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 7,
          status: "waiting_first_frame",
          device_name: "AirPods Pro",
        },
      });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("等待麦克风");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("尚未收到音频");

    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 7 } });
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 7,
          status: "receiving_audio",
          device_name: "AirPods Pro",
        },
      });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("录音中");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("AirPods Pro");

    act(() => {
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 6,
          status: "device_error",
          device_name: "Old microphone",
        },
      });
    });
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("AirPods Pro");

    act(() => {
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 7,
          status: "no_audio_yet",
          device_name: "AirPods Pro",
        },
      });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("麦克风尚无输入");

    act(() => {
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 7,
          status: "no_recent_audio",
          device_name: "AirPods Pro",
        },
      });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("麦克风无响应");
  });

  it("offers only a recovery destination for the matching failed generation", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 20 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(20, "no_audio"),
    }));
    expectRecoveryCopy("麦克风无法打开；更改输入设备后重试。");
    expect(screen.getByRole("button", { name: "选择麦克风" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "关闭恢复提示" })).toBeInTheDocument();

    act(() => handlers.get("dictation://state")!({ payload: { state: "starting", session_generation: 21 } }));
    expect(screen.queryByText("麦克风无法打开；更改输入设备后重试。")).not.toBeInTheDocument();

    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(20, "history_saved", null, 2, false),
    }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
  });

  it("uses the actual delivery method to distinguish clipboard and History outcomes", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({
      payload: {
        state: "copied",
        session_generation: 41,
        delivery_method: "clipboard",
        fallback_reason: "target_changed",
      },
    }));
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("输入目标已变化");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("文字已复制");

    act(() => handlers.get("dictation://state")!({
      payload: {
        state: "history",
        session_generation: 41,
        delivery_method: "history",
        fallback_reason: "target_changed",
      },
    }));
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("输入目标已变化");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("结果已存历史");
  });

  it("keeps History-copy recovery bound to the current visible ticket and copies only on explicit action", async () => {
    const ticket = historyRecoveryPayload(44, 3, "ready", "pending");
    invokeMock.mockImplementation(async (command) => command === "copy_history_recovery"
      ? {
        ...ticket,
        ticket_revision: 4,
        copy_status: "copied",
        reason_code: null,
      }
      : undefined);
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 44 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: ticket }));

    expectRecoveryCopy("历史记录仍在保存；你也可以复制这条听写。");
    fireEvent.click(await screen.findByRole("button", { name: "复制听写文字" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("copy_history_recovery", {
      request: {
        ticketRevision: 3,
        sessionGeneration: 44,
        deliveryTransactionId: "0123456789abcdef0123456789abcdef",
        historyTransactionId: "dictation-1234-abc-1",
      },
    }));
    await waitFor(() => expect(screen.getByRole("button", { name: "已复制到剪贴板" })).toBeDisabled());
    expect(screen.getByText("听写文字已复制到剪贴板，请手动粘贴。")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
  });

  it("ignores an older copy receipt after a newer ticket revision", async () => {
    let finishCopy: ((receipt: unknown) => void) | undefined;
    const first = historyRecoveryPayload(45, 1, "ready", "failed");
    invokeMock.mockImplementation((command) => command === "copy_history_recovery"
      ? new Promise((resolve) => { finishCopy = resolve; })
      : Promise.resolve(undefined));
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "history", delivery_method: "history", session_generation: 45 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: first }));
    fireEvent.click(await screen.findByRole("button", { name: "复制听写文字" }));
    await waitFor(() => expect(finishCopy).toBeTypeOf("function"));

    const newer = { ...first, ticket_revision: 2, copy_status: "failed" as const };
    act(() => handlers.get("dictation://history-recovery")!({ payload: newer }));
    await act(async () => {
      finishCopy?.({ ...first, copy_status: "copied", reason_code: null });
    });

    expect(screen.getByRole("button", { name: "重试复制文字" })).toBeEnabled();
    expect(screen.queryByText("听写文字已复制到剪贴板，请手动粘贴。")).not.toBeInTheDocument();
  });

  it("shows a failed copy receipt on the same ticket and leaves explicit retry available", async () => {
    const ticket = historyRecoveryPayload(49, 1, "ready", "failed");
    invokeMock.mockImplementation(async (command) => command === "copy_history_recovery"
      ? { ...ticket, ticket_revision: 2, copy_status: "failed", reason_code: "source_unavailable" }
      : undefined);
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "history", session_generation: 49 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: ticket }));
    fireEvent.click(await screen.findByRole("button", { name: "复制听写文字" }));

    expect(await screen.findByRole("button", { name: "重试复制文字" })).toBeEnabled();
    expect(document.querySelectorAll(".voice-pill__secondary--recovery")).toHaveLength(1);
    expect(screen.getByText("未能复制恢复文字；你可以重试。")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("copy_history_recovery", expect.objectContaining({
      request: expect.objectContaining({ sessionGeneration: 49 }),
    }));
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
  });

  it("does not reopen collapsed or dismissed recovery for a stale or late ticket revision", async () => {
    invokeMock.mockResolvedValue(undefined);
    const current = historyRecoveryPayload(50, 3, "ready", "failed");
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 50 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: current }));

    fireEvent.click(screen.getByRole("button", { name: "关闭详情" }));
    await waitFor(() => expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument());
    act(() => handlers.get("dictation://history-recovery")!({ payload: { ...current, ticket_revision: 2 } }));
    expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "查看详情" }));
    fireEvent.click(await screen.findByRole("button", { name: "关闭恢复提示" }));
    await waitFor(() => expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument());
    act(() => handlers.get("dictation://history-recovery")!({ payload: { ...current, ticket_revision: 4 } }));
    expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument();
  });

  it("does not accept a late History ticket from A after B starts or reopen a successful hidden HUD", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 46 } }));
    act(() => handlers.get("dictation://state")!({ payload: { state: "starting", session_generation: 47 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: historyRecoveryPayload(46) }));
    expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("copy_history_recovery", expect.anything());

    act(() => handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 48 } }));
    act(() => handlers.get("dictation://state")!({ payload: { state: "processing", phase: "delivery", session_generation: 48 } }));
    act(() => handlers.get("dictation://history-recovery")!({ payload: historyRecoveryPayload(48) }));
    expect(document.querySelector(".voice-pill__secondary--recovery")).toBeInTheDocument();
    act(() => handlers.get("dictation://state")!({ payload: {
      state: "done",
      delivery_method: "paste",
      fallback_reason: null,
      session_generation: 48,
    } }));
    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden-now");
    expect(screen.queryByText("历史保存未确认；恢复文字仍可从本机记录中复制。")).not.toBeInTheDocument();
    act(() => handlers.get("dictation://history-recovery")!({ payload: { ...historyRecoveryPayload(48), ticket_revision: 2 } }));
    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".voice-pill__secondary--recovery")).not.toBeInTheDocument();
  });

  it("clears a prior and late History ticket for an uncertain paste without opening details", async () => {
    await renderHud();
    const stateEvent = handlers.get("dictation://state")!;
    act(() => stateEvent({ payload: { state: "processing", phase: "delivery", session_generation: 51 } }));
    const ticket = historyRecoveryPayload(51);
    act(() => handlers.get("dictation://history-recovery")!({ payload: ticket }));
    expect(document.querySelector(".voice-pill__secondary--recovery")).toBeInTheDocument();

    act(() => stateEvent({ payload: {
      state: "unverified",
      delivery_method: "paste_unverified",
      fallback_reason: "paste_mutation_uncertain",
      session_generation: 51,
    } }));

    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("已尝试输入");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("先检查输入框");
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).not.toContain("历史记录");
    expect(screen.queryByRole("button", { name: "查看详情" })).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__secondary--recovery")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "复制听写文字" })).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__secondary--details")).not.toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenLastCalledWith("set_island_details_visible", {
      expectedGeneration: 51,
      visible: false,
    }));

    act(() => handlers.get("dictation://history-recovery")!({ payload: { ...ticket, ticket_revision: 2 } }));
    expect(document.querySelector(".voice-pill__secondary--recovery")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "复制听写文字" })).not.toBeInTheDocument();
  });

  it("opens an exact recovery ticket tray and lets the same details owner collapse it", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 42 } }));
    act(() => handlers.get("audio://recovery")!({ payload: recoveryPayload(42, "history_saved") }));

    expectRecoveryCopy("录音已保存到历史记录。");
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_details_visible", {
      expectedGeneration: 42,
      visible: true,
    }));
    fireEvent.click(screen.getByRole("button", { name: /关闭详情|查看详情/ }));
    await waitFor(() => expect(screen.queryByText("录音已保存到历史记录。", {
      selector: ".voice-pill__recovery-message",
    })).not.toBeInTheDocument());
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_details_visible", {
      expectedGeneration: 42,
      visible: false,
    }));
  });

  it("accepts recovery immediately after a new-generation error before React commits", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "starting", session_generation: 29 },
      });
      handlers.get("dictation://state")!({
        payload: { state: "error", session_generation: 30 },
      });
      handlers.get("audio://recovery")!({
        payload: recoveryPayload(30, "no_audio"),
      });
    });

    expectRecoveryCopy("麦克风无法打开；更改输入设备后重试。");
    expect(screen.getByRole("button", { name: "选择麦克风" })).toBeInTheDocument();
  });

  it.each(["hud-dismissed", "confirmed paste"] as const)("keeps effective idle admission after %s for adjacent callbacks", async (kind) => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "processing", session_generation: 20 } }));
    if (kind === "hud-dismissed") {
      act(() => handlers.get("dictation://hud-dismissed")!({ payload: { session_generation: 20 } }));
    } else {
      act(() => handlers.get("dictation://state")!({
        payload: {
          state: "done",
          delivery_method: "paste",
          fallback_reason: null,
          session_generation: 20,
        },
      }));
    }

    expect(document.querySelector(".voice-pill")).toHaveClass("voice-pill--idle");
    invokeMock.mockClear();
    act(() => handlers.get("dictation://partial")!({ payload: { session_generation: 21, text: "ignored while idle" } }));
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_details_visible", {
      expectedGeneration: 21,
      visible: false,
    });

    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 20 } });
      handlers.get("audio://recovery")!({ payload: recoveryPayload(20, "audio_retained", "session-20", 2) });
    });
    expectRecoveryCopy("未能保存到历史，恢复音频仍已保留。请重试。");
    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", {
      sessionId: "session-20",
      ticketRevision: 2,
    }));
  });

  it("rejects stale same-batch idle after progress and admits a later partial generation", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 20 } }));
    act(() => {
      handlers.get("dictation://progress")!({ payload: { session_generation: 21, progress: 0.5 } });
      handlers.get("dictation://state")!({ payload: { state: "idle", session_generation: 20 } });
    });
    act(() => handlers.get("audio://recovery")!({ payload: recoveryPayload(21, "history_saved") }));
    expectRecoveryCopy("录音已保存到历史记录。");
    invokeMock.mockClear();

    act(() => handlers.get("dictation://partial")!({ payload: { session_generation: 22, text: "legitimate newer hidden partial" } }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
    expect(screen.queryByText("legitimate newer hidden partial")).not.toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_details_visible", {
      expectedGeneration: 22,
      visible: false,
    }));

    act(() => handlers.get("audio://recovery")!({ payload: recoveryPayload(21, "history_saved", null, 2) }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
  });

  it.each(["idle partial", "stale quota"] as const)("keeps recovery ownership after an ignored %s event", async (ignoredEvent) => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 20 } });
      handlers.get("audio://recovery")!({ payload: recoveryPayload(20, "audio_retained", "session-20", 1) });
    });

    if (ignoredEvent === "idle partial") {
      act(() => {
        handlers.get("dictation://state")!({ payload: { state: "idle", session_generation: 20 } });
        handlers.get("dictation://partial")!({ payload: { session_generation: 21, text: "ignored" } });
      });
    } else {
      act(() => {
        handlers.get("selected-action://state")!({ payload: { state: "running", session_generation: 22, action_sequence: 1 } });
        handlers.get("quota://rate_limited")!({ payload: { session_generation: 21, retry_after_secs: 3 } });
      });
    }

    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(20, "audio_retained", "session-20-revised", 2),
    }));
    expectRecoveryCopy("未能保存到历史，恢复音频仍已保留。请重试。");
    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", {
      sessionId: "session-20-revised",
      ticketRevision: 2,
    }));
  });

  it.each(["partial", "progress", "limit", "quota"] as const)("keeps same-generation recovery and clears it for an admitted newer %s event", async (kind) => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 30 } });
      handlers.get("audio://recovery")!({ payload: recoveryPayload(30, "audio_retained", "session-30", 1) });
    });

    const emitEvent = (generation: number) => {
      if (kind === "partial") {
        handlers.get("dictation://partial")!({ payload: { session_generation: generation, text: "hidden" } });
      } else if (kind === "progress") {
        handlers.get("dictation://progress")!({ payload: { session_generation: generation, progress: 0.5 } });
      } else if (kind === "limit") {
        handlers.get("audio://limit")!({ payload: { session_generation: generation, max_recording_secs: 600 } });
      } else {
        handlers.get("quota://rate_limited")!({ payload: { session_generation: generation, retry_after_secs: 3 } });
      }
    };

    act(() => emitEvent(29));
    act(() => emitEvent(30));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(30, "audio_retained", "session-30-revised", 2),
    }));
    expectRecoveryCopy("未能保存到历史，恢复音频仍已保留。请重试。");
    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", {
      sessionId: "session-30-revised",
      ticketRevision: 2,
    }));

    act(() => emitEvent(31));
    expect(screen.queryByText("未能保存到历史，恢复音频仍已保留。请重试。")).not.toBeInTheDocument();
  });

  it("clears old recovery when an accepted partial advances the generation", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 44 } }));
    act(() => handlers.get("audio://recovery")!({ payload: recoveryPayload(44, "history_saved") }));
    expectRecoveryCopy("录音已保存到历史记录。");

    act(() => handlers.get("dictation://partial")!({ payload: { session_generation: 44, text: "same session" } }));
    expect(screen.getByText("录音已保存到历史记录。")).toBeInTheDocument();
    act(() => handlers.get("dictation://partial")!({ payload: { session_generation: 45, text: "new session" } }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
    expect(screen.queryByText("new session")).not.toBeInTheDocument();

    act(() => handlers.get("audio://recovery")!({ payload: recoveryPayload(44, "history_saved", null, 2) }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
  });

  it("opens History for a saved session without resending or pasting its text", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 22 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(22, "history_saved"),
    }));
    fireEvent.click(await screen.findByRole("button", { name: "查看历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("open_recovery_destination", { destination: "history", ticketRevision: 1 }));
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
  });

  it("opens the issue-specific Settings route from the current terminal details owner", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    act(() => handlers.get("dictation://state")!({
      payload: { state: "error", fallback_reason: "microphone_required", session_generation: 33 },
    }));

    fireEvent.click(screen.getByRole("button", { name: "查看详情" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_details_visible", {
      expectedGeneration: 33,
      visible: true,
    }));
    fireEvent.click(await screen.findByRole("button", { name: "打开麦克风设置" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("open_hud_destination", {
      destination: "microphone",
      expectedGeneration: 33,
    }));
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
  });

  it("keeps a current recovery save busy when an older HUD dismissal arrives", async () => {
    let finishSave: (() => void) | undefined;
    invokeMock.mockImplementation((command) => command === "restore_audio_recovery_to_history"
      ? new Promise<void>((resolve) => { finishSave = () => resolve(); })
      : Promise.resolve(undefined));
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 34 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(34, "audio_retained", "session-34"),
    }));

    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(finishSave).toBeTypeOf("function"));
    act(() => handlers.get("dictation://hud-dismissed")!({ payload: { session_generation: 33 } }));

    expect(screen.getByRole("button", { name: "保存到历史" })).toBeDisabled();
    expect(document.querySelector(".voice-pill__secondary--recovery")).toHaveAttribute("aria-busy", "true");
    await act(async () => { finishSave?.(); });
  });

  it("keeps the current recovery error when an older HUD dismissal arrives", async () => {
    invokeMock.mockRejectedValue(new Error("open failed"));
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 36 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(36, "history_saved"),
    }));

    fireEvent.click(await screen.findByRole("button", { name: "查看历史" }));
    await waitFor(() => expect(screen.getByText("无法打开目标设置或历史记录，请重试。")).toBeInTheDocument());
    act(() => handlers.get("dictation://hud-dismissed")!({ payload: { session_generation: 35 } }));

    expect(screen.getByText("无法打开目标设置或历史记录，请重试。")).toBeInTheDocument();
  });

  it("dismisses only the exact native recovery ticket without a frontend visibility toggle", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 23 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(23, "history_saved", null, 9, false),
    }));

    fireEvent.click(await screen.findByRole("button", { name: "关闭恢复提示" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("dismiss_audio_recovery", { ticketRevision: 9 }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_recovery_actions", expect.anything());

    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(23, "history_saved", null, 9, false),
    }));
    expect(screen.queryByText("录音已保存到历史记录。")).not.toBeInTheDocument();
  });

  it("retries only the retained session's History save and keeps the action on failure", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 24 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(24, "audio_retained", "session-24"),
    }));
    invokeMock.mockImplementation(async (command) => {
      if (command === "restore_audio_recovery_to_history") throw new Error("private error details");
      return undefined;
    });

    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", { sessionId: "session-24", ticketRevision: 1 }));
    expectRecoveryCopy("未能保存到历史，恢复音频仍已保留。请重试。");
    expect(screen.getByText("保存到历史失败，请重试。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存到历史" })).toBeEnabled();
    expect(invokeMock).not.toHaveBeenCalledWith("open_recovery_destination", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
  });

  it("opens History only after the exact retained session is saved", async () => {
    invokeMock.mockImplementation(async (command) => command === "restore_audio_recovery_to_history"
      ? recoveryPayload(25, "history_saved", null, 2, false)
      : undefined);
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 25 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(25, "audio_retained", "session-25"),
    }));

    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", { sessionId: "session-25", ticketRevision: 1 }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("open_recovery_destination", { destination: "history", ticketRevision: 2 }));
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
  });

  it("does not open History when a newer recording starts during the exact-session save", async () => {
    let finishSave: (() => void) | undefined;
    invokeMock.mockImplementation((command) => command === "restore_audio_recovery_to_history"
      ? new Promise<void>((resolve) => { finishSave = () => resolve(); })
      : Promise.resolve(undefined));
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 26 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(26, "audio_retained", "session-26"),
    }));

    fireEvent.click(await screen.findByRole("button", { name: "保存到历史" }));
    await waitFor(() => expect(finishSave).toBeTypeOf("function"));
    act(() => handlers.get("dictation://state")!({ payload: { state: "starting", session_generation: 27 } }));
    await act(async () => { finishSave?.(); });

    expect(invokeMock).toHaveBeenCalledWith("restore_audio_recovery_to_history", {
      sessionId: "session-26",
      ticketRevision: 1,
    });
    expect(invokeMock).not.toHaveBeenCalledWith("open_recovery_destination", expect.anything());
  });

  it("ignores older recovery revisions and refuses an unbound retained-session action", async () => {
    await renderHud();
    act(() => handlers.get("dictation://state")!({ payload: { state: "error", session_generation: 28 } }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(28, "history_saved", null, 4, false),
    }));
    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(28, "unknown_failure", null, 3, true),
    }));
    expectRecoveryCopy("录音已保存到历史记录。");
    expect(screen.queryByText("录音恢复状态未知，请检查历史记录。")).not.toBeInTheDocument();

    act(() => handlers.get("audio://recovery")!({
      payload: recoveryPayload(28, "audio_retained", null, 5, true),
    }));
    expectRecoveryCopy("录音恢复状态未知，请检查历史记录。");
    expect(screen.getByRole("button", { name: "检查历史" })).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("restore_audio_recovery_to_history", expect.anything());
  });

  it("stops presenting retained capture status as live when processing begins", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "starting", session_generation: 9 },
      });
      handlers.get("audio://capture")!({
        payload: {
          session_generation: 9,
          status: "receiving_audio",
          device_name: "AirPods Pro",
          input_selection: "MacBook Pro Microphone",
        },
      });
      handlers.get("dictation://state")!({ payload: { state: "recording", session_generation: 9 } });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("录音中");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("AirPods Pro");

    act(() => handlers.get("dictation://state")!({
      payload: { state: "processing", phase: "cleanup", phase_confirmed: true, session_generation: 9 },
    }));

    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("处理中");
    expect(document.querySelector(".voice-pill__subtitle")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__elapsed")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").not.toContain("AirPods Pro");
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
    emitMock.mockReset();
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

    const hudLabel = document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "";
    expect(hudLabel).toContain("WeChat · 口语");
    expect(hudLabel).not.toContain("你好世界");
    expect(document.querySelector(".voice-pill__subtitle")).not.toHaveTextContent("你好世界");
    expect(invokeMock).not.toHaveBeenCalledWith("repaste_history", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("retry_dictation", expect.anything());
    expect(invokeMock.mock.calls.every(([command]) =>
      command === "set_island_details_visible" || command === "get_delivery_settlement_snapshot",
    )).toBe(true);
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

  it("hides only the current fallback-free paste and preserves onboarding feedback", async () => {
    invokeMock.mockResolvedValue(undefined);
    await renderHud();
    const stateEvent = handlers.get("dictation://state")!;
    await waitFor(() => expect(handlers.has("learn_pairs://promoted")).toBe(true));

    act(() => {
      handlers.get("learn_pairs://promoted")!({
        payload: { pair_key: "知呼\u001e知乎", before: "知呼", after: "知乎" },
      });
      stateEvent({ payload: { state: "recording", session_generation: 40 } });
      stateEvent({
        payload: {
          state: "done",
          delivery_method: "paste",
          fallback_reason: null,
          session_generation: 40,
        },
      });
    });
    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".island-learn-toast")).toBeInTheDocument();
    act(() => stateEvent({ payload: { state: "idle", session_generation: 40 } }));
    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden-now");

    act(() => {
      stateEvent({ payload: { state: "recording", session_generation: 41 } });
      stateEvent({
        payload: {
          state: "unverified",
          delivery_method: "paste_unverified",
          fallback_reason: "paste_unverified",
          session_generation: 41,
        },
      });
    });
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("已尝试输入");

    act(() => {
      stateEvent({ payload: { state: "recording", session_generation: 42 } });
      stateEvent({
        payload: {
          state: "done",
          delivery_method: "onboarding",
          fallback_reason: null,
          session_generation: 42,
        },
      });
    });
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("已完成");

    act(() => {
      stateEvent({ payload: { state: "recording", session_generation: 43 } });
      stateEvent({
        payload: {
          state: "done",
          delivery_method: "paste",
          fallback_reason: null,
          session_generation: 42,
        },
      });
    });
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden-now");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("录音中");
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

    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").toContain("Cursor · 代码");
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

    const hudLabel = document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "";
    expect(hudLabel).toContain("辅助功能文字");
    expect(hudLabel).not.toContain("forged label");
    expect(hudLabel).not.toContain("Gmail personal rule");
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
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").toContain("本机 OCR");

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "cleanup", session_generation: 4 },
      });
    });
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").not.toContain("本机 OCR");

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "idle", session_generation: 4 },
      });
    });
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").not.toContain("本机 OCR");
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
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").toContain("Cursor · 代码");

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "waiting_retry", session_generation: 3 },
      });
    });
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").toContain("Cursor · 代码");

    act(() => {
      vi.advanceTimersByTime(2_500);
    });
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "").not.toContain("Cursor · 代码");

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
    expect(document.querySelector(".voice-pill__subtitle")).not.toBeInTheDocument();
    vi.useRealTimers();
  });

  it("shows retry waiting and rejects selected-action events from older generations or sequences", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: {
          state: "processing",
          phase: "waiting_retry",
          phase_confirmed: false,
          retry_after_secs: 2.2,
          session_generation: 31,
        },
      });
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("处理中");
    expect(document.querySelector(".voice-pill__elapsed")).not.toBeInTheDocument();

    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "asr", phase_confirmed: true, session_generation: 32 },
      });
      handlers.get("selected-action://state")!({
        payload: { state: "listening", session_generation: 31, action_sequence: 40 },
      });
      handlers.get("selected-action://state")!({
        payload: { state: "preparing_rewrite", session_generation: 32, action_sequence: 12 },
      });
      handlers.get("selected-action://state")!({
        payload: { state: "listening", session_generation: 32, action_sequence: 11 },
      });
    });

    const status = document.querySelector(".voice-pill")?.getAttribute("aria-label") ?? "";
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("处理中");
    expect(status).not.toContain("正在识别");
    expect(status).not.toContain("正在听取操作");
  });

  it("keeps a preview_ready text action out of History after unverified dictation", async () => {
    await renderHud();
    act(() => {
      handlers.get("dictation://state")!({
        payload: { state: "processing", phase: "delivery", session_generation: 33 },
      });
      handlers.get("selected-action://state")!({
        payload: { state: "preview_ready", session_generation: 33, action_sequence: 14 },
      });
      handlers.get("dictation://state")!({
        payload: {
          state: "unverified",
          delivery_method: "paste_unverified",
          fallback_reason: "paste_unverified",
          session_generation: 33,
        },
      });
    });

    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("已尝试输入");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("先检查输入框");
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).not.toMatch(/History|rewrite/i);
    fireEvent.click(screen.getByRole("button", { name: "查看详情" }));
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("先检查输入框");
    expect(document.querySelector(".voice-pill__details-text")).not.toHaveTextContent(/历史记录|History|正在准备改写/i);
    expect(screen.queryByRole("button", { name: /历史记录|History/i })).not.toBeInTheDocument();
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

    const toast = await findLearnToast();
    expect(toast).toHaveTextContent("已记住这个词");
    expect(toast).toHaveTextContent("知呼→知乎");
    expect(toast).toHaveStyle({ height: "48px" });
    expect(toast.querySelector(".island-learn-toast__icon svg")).toBeInTheDocument();
    expect(toast.querySelectorAll(".island-learn-toast__action")).toHaveLength(2);
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
    expect(await findLearnToast()).toHaveStyle({ height: "48px" });
    act(() => handlers.get("learn_pairs://promoted")!({
      payload: { pair_key: "second", before: "新字", after: "新词" },
    }));
    expect(learnToast()).toHaveTextContent("已记住这个词");
    expect(learnToast()).toHaveTextContent("新字→新词");
    expect(invokeMock.mock.calls.filter(([command, args]) =>
      command === "set_island_learn_interactive" && (args as { interactive: boolean }).interactive,
    )).toHaveLength(1);
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });
    const close = screen.getByRole("button", { name: "关闭" });
    expect(fireEvent.mouseDown(close, { cancelable: true })).toBe(false);
    fireEvent.click(close);
    await waitFor(() => expect(learnToast()).not.toBeInTheDocument());
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
    const undo = await screen.findByRole("button", { name: "撤销" });
    expect(fireEvent.mouseDown(undo, { cancelable: true })).toBe(false);
    fireEvent.click(undo);

    expect(await screen.findByText("撤销尚未完成")).toBeInTheDocument();
    expect(screen.getByText("可以再次尝试")).toBeInTheDocument();
    expect(learnToast()).toHaveAttribute("aria-label", "撤销未完成，请重试。");
    expect(learnToast()).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试撤销" })).toBeEnabled();
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    expect(invokeMock).not.toHaveBeenCalledWith("set_island_learn_interactive", { interactive: false });

    fireEvent.click(screen.getByRole("button", { name: "重试撤销" }));
    expect(learnToast()).toBeInTheDocument();
    expect(screen.getByText("正在撤销学习")).toBeInTheDocument();
    expect(screen.getByText("请稍候")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "撤销" })).toBeDisabled();
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    await act(async () => { finishRetry(); await retry; });
    await waitFor(() => expect(learnToast()).not.toBeInTheDocument());
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
      expect(learnToast()).toHaveTextContent("已记住这个词");
      expect(learnToast()).toHaveTextContent("新字→新词");

      await act(async () => {
        if (outcome === "success") finishUndo();
        else failUndo(new Error("synthetic late failure"));
        await pendingUndo.catch(() => undefined);
      });
      expect(learnToast()).toHaveTextContent("已记住这个词");
      expect(learnToast()).toHaveTextContent("新字→新词");
      expect(screen.queryByText("撤销尚未完成")).not.toBeInTheDocument();
      expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
      await waitFor(() => expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "撤销" }));
      await waitFor(() => expect(learnToast()).not.toBeInTheDocument());
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
    expect(await screen.findByText("撤销尚未完成")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "style-pair" });
    expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", { pairKey: "intensity-pair" });
    expect(invokeMock).not.toHaveBeenCalledWith("hide_island_if_idle");
    fireEvent.click(screen.getByRole("button", { name: "重试撤销" }));
    await waitFor(() => expect(learnToast()).not.toBeInTheDocument());
    expect(invokeMock.mock.calls.filter(([command]) => command === "undo_learn_pair")).toHaveLength(4);
  });

});
