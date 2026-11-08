import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  acceptsSettlementSnapshot,
  compareSettlementRevisions,
  dictationCompletionMessage,
  DictationCompletionStatus,
  type DeliverySettlementRegistrySnapshot,
} from "./DictationCompletionStatus";
import { I18nProvider, useI18n } from "../../lib/i18n";

const { emitMock, listenMock } = vi.hoisted(() => ({ emitMock: vi.fn(), listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ emit: emitMock, listen: listenMock }));

const invokeMock = vi.mocked(invoke);
const currentId = "0123456789abcdef0123456789abcdef";
const processId = "fedcba9876543210fedcba9876543210";

function owner(overrides: Record<string, unknown> = {}) {
  return {
    transaction_id: currentId,
    terminal_outcome: {
      kind: "inserted",
      phase: "done",
      delivery_method: "paste",
      fallback_reason: null,
    },
    writer_admission_sealed: true,
    admitted_writers: 0,
    writers_closed: true,
    clipboard_restoration: "unverified",
    next_capture_readiness: "unverified",
    ...overrides,
  };
}

function snapshot(overrides: Record<string, unknown> = {}): DeliverySettlementRegistrySnapshot {
  return {
    schema: "voiceflow_delivery_settlement_v1",
    process_identity: processId,
    revision: "5",
    revision_available: true,
    current: null,
    last_terminal: owner(),
    ...overrides,
  } as DeliverySettlementRegistrySnapshot;
}

describe("persistent dictation completion status", () => {
  beforeEach(() => {
    emitMock.mockReset();
    emitMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    invokeMock.mockReset();
    listenMock.mockReset();
    emitMock.mockReset();
  });

  it("keeps the human status outside the hidden pill and says insertion is not restoration or readiness", async () => {
    invokeMock.mockResolvedValue(snapshot());
    listenMock.mockImplementation(async (_event: string, _handler: (event: { payload: unknown }) => void) => vi.fn());
    render(
      <I18nProvider initialLanguage="zh">
        <div>
          <div aria-hidden="true"><span>pill</span></div>
          <DictationCompletionStatus />
        </div>
      </I18nProvider>,
    );

    const status = await screen.findByRole("status", { name: "VoiceFlow 听写状态" });
    expect(status).toHaveAttribute("id", "voiceflow-dictation-settlement-status");
    expect(status).toHaveAttribute("aria-live", "polite");
    expect(status).toHaveAttribute("aria-atomic", "true");
    await waitFor(() => expect(status).toHaveTextContent("文字已输入"));
    expect(status).toHaveTextContent("剪贴板恢复情况");
    expect(status).toHaveTextContent("下一次录音是否可用均未确认");
    expect(status).not.toHaveTextContent(currentId);
    expect(status).not.toHaveTextContent("writer_admission_sealed");
    expect(status.closest('[aria-hidden="true"]')).toBeNull();
  });

  it("does not announce an outcome until its sealed writer owner has closed", () => {
    const active = snapshot({
      current: owner({
        admitted_writers: 1,
        writer_admission_sealed: true,
        writers_closed: false,
      }),
      last_terminal: null,
    });
    expect(dictationCompletionMessage(active, (source) => source)).toBe("VoiceFlow 正在进行本次听写。");
  });

  it("uses accurate English progress wording for an active owner", async () => {
    const active = snapshot({
      current: owner({
        admitted_writers: 1,
        writer_admission_sealed: true,
        writers_closed: false,
      }),
      last_terminal: null,
    });
    invokeMock.mockResolvedValue(active);
    listenMock.mockImplementation(async (_event: string, _handler: (event: { payload: unknown }) => void) => vi.fn());
    render(
      <I18nProvider initialLanguage="en">
        <DictationCompletionStatus />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByRole("status", { name: "VoiceFlow dictation status" }))
      .toHaveTextContent("VoiceFlow dictation is in progress."));
  });

  it("synchronizes native status language changes without another snapshot request", async () => {
    invokeMock.mockResolvedValue(snapshot());
    listenMock.mockImplementation(async (_event: string, _handler: (event: { payload: unknown }) => void) => vi.fn());
    emitMock.mockResolvedValue(undefined);

    function ChangeLanguage() {
      const { setLanguage } = useI18n();
      return <button type="button" onClick={() => setLanguage("zh")}>Use Chinese</button>;
    }

    render(
      <I18nProvider initialLanguage="en">
        <ChangeLanguage />
        <DictationCompletionStatus />
      </I18nProvider>,
    );
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("dictation://settlement-language", "en"));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole("button", { name: "Use Chinese" }));
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("dictation://settlement-language", "zh"));
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("subscribes before the snapshot getter and ignores stale or duplicate revisions", async () => {
    let handler: ((event: { payload: unknown }) => void) | null = null;
    listenMock.mockImplementation(async (_event: string, callback: (event: { payload: unknown }) => void) => {
      handler = callback;
      return vi.fn();
    });
    const initial = snapshot({ revision: "18446744073709551615" });
    invokeMock.mockResolvedValue(initial);
    render(
      <I18nProvider initialLanguage="zh">
        <DictationCompletionStatus />
      </I18nProvider>,
    );
    await waitFor(() => expect(handler).not.toBeNull());
    expect(invokeMock).toHaveBeenCalledWith("get_delivery_settlement_snapshot");

    const next = snapshot({
      revision: "18446744073709551616",
      current: owner({ transaction_id: "abcdef0123456789abcdef0123456789", writers_closed: false, terminal_outcome: null }),
      last_terminal: owner(),
    });
    expect(compareSettlementRevisions(next.revision, initial.revision)).toBe(1);
    expect(acceptsSettlementSnapshot(initial, next)).toBe(true);
    expect(acceptsSettlementSnapshot(next, initial)).toBe(false);
    expect(acceptsSettlementSnapshot(next, next)).toBe(false);

    act(() => handler?.({ payload: next }));
    expect(screen.getByRole("status", { name: "VoiceFlow 听写状态" })).toHaveTextContent("正在进行本次听写");
    act(() => handler?.({ payload: initial }));
    expect(screen.getByRole("status", { name: "VoiceFlow 听写状态" })).toHaveTextContent("正在进行本次听写");
  });

  it("freezes the public state after revision exhaustion and rejects another process", () => {
    const lastAvailable = snapshot({ revision: "18446744073709551615" });
    const exhausted = snapshot({
      revision: "18446744073709551615",
      revision_available: false,
      current: owner({ terminal_outcome: null, writer_admission_sealed: false, writers_closed: false }),
      last_terminal: owner(),
    });
    const changedProcess = snapshot({ process_identity: "00112233445566778899aabbccddeeff", revision: "18446744073709551616" });

    expect(acceptsSettlementSnapshot(lastAvailable, exhausted)).toBe(true);
    expect(acceptsSettlementSnapshot(exhausted, snapshot({ revision: "18446744073709551616" }))).toBe(false);
    expect(acceptsSettlementSnapshot(lastAvailable, changedProcess)).toBe(false);
    expect(dictationCompletionMessage(exhausted, (source) => source)).toBe(
      "听写状态身份无法确认，请检查 VoiceFlow。",
    );
  });

  it("fails closed when the process or transaction identity is unavailable", () => {
    const unavailable = snapshot({
      process_identity: null,
      current: owner({ transaction_id: null }),
      last_terminal: null,
    });
    expect(dictationCompletionMessage(unavailable, (source) => source)).toBe(
      "听写状态身份无法确认，请检查 VoiceFlow。",
    );

    const terminalWithoutIdentity = snapshot({
      current: owner({ transaction_id: null }),
      last_terminal: null,
    });
    expect(dictationCompletionMessage(terminalWithoutIdentity, (source) => source)).toBe(
      "听写状态身份无法确认，请检查 VoiceFlow。",
    );
  });
});
