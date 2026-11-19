import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { PreviewApp } from "./preview";
import type { SelectedActionPreview } from "./components/SelectedPreviewDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function makePreview(transactionId: string, finalText = `结果 ${transactionId}`, thumbnail?: string): SelectedActionPreview {
  return {
    transaction_id: transactionId,
    action_sequence: transactionId === "tx-new" ? 2 : 1,
    kind: "selected",
    operation: "rewrite",
    target_kind: "selection",
    target_label: "selected_text",
    source_text: "synthetic source",
    instruction: "synthetic instruction",
    delivery_mode: "replace_or_copy",
    delivery_notice: "copy_if_target_changed",
    selected_text: "synthetic source",
    transcript: "synthetic instruction",
    final_text: finalText,
    thumbnail,
    replace_allowed: true,
    provenance_receipt: {
      source_kind: "selected_text",
      char_count: 16,
      word_count: 2,
      screen_captured: false,
      context_source: [],
      provider_label: "Groq",
    },
  };
}

function bootstrap(preview: SelectedActionPreview) {
  return { ui_language: "zh" as const, theme: "system" as const, preview };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_action_preview_bootstrap") return bootstrap(makePreview("tx-old")) as never;
    return "replaced" as never;
  });
});

afterEach(() => {
  cleanup();
});

describe("standalone preview window", () => {
  it("keeps cancel available during a pending confirm and ignores the old reply after a new transaction loads", async () => {
    const confirm = deferred<unknown>();
    let currentBootstrap = bootstrap(makePreview("tx-old"));
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_action_preview_bootstrap") return currentBootstrap as never;
      if (command === "confirm_selected_action_preview") return confirm.promise as never;
      if (command === "cancel_selected_action_preview") return undefined as never;
      return "replaced" as never;
    });

    render(<PreviewApp />);
    expect(await screen.findByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("结果 tx-old");
    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("confirm_selected_action_preview", {
      transaction_id: "tx-old",
      final_text: "结果 tx-old",
    }));
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("cancel_selected_action_preview", {
      transaction_id: "tx-old",
    }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    currentBootstrap = bootstrap(makePreview("tx-new", "新事务结果"));
    await act(async () => {
      window.dispatchEvent(new Event("voiceflow-preview-ready"));
    });
    expect(await screen.findByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("新事务结果");

    await act(async () => {
      confirm.resolve("replaced");
      await confirm.promise;
    });
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("新事务结果");
  });

  it("clears retained thumbnail and editable preview when native bootstrap reports terminal close", async () => {
    let currentBootstrap: ReturnType<typeof bootstrap> | null = bootstrap(
      makePreview("tx-image", "synthetic output", "data:image/png;base64,AA=="),
    );
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_action_preview_bootstrap") return currentBootstrap as never;
      return undefined as never;
    });

    render(<PreviewApp />);
    expect(await screen.findByRole("img", { name: "窗口截图" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("synthetic output");

    currentBootstrap = null;
    await act(async () => {
      window.dispatchEvent(new Event("voiceflow-preview-ready"));
    });
    await waitFor(() => expect(screen.queryByRole("img", { name: "窗口截图" })).not.toBeInTheDocument());
    expect(screen.queryByRole("textbox", { name: "VoiceFlow 生成结果" })).not.toBeInTheDocument();
  });

  it("drops an older bootstrap reply after a newer native wake has requested the current transaction", async () => {
    const first = deferred<unknown>();
    const second = deferred<unknown>();
    let reads = 0;
    invokeMock.mockImplementation((command) => {
      if (command !== "get_action_preview_bootstrap") return Promise.resolve(undefined) as never;
      reads += 1;
      return (reads === 1 ? first.promise : second.promise) as never;
    });

    render(<PreviewApp />);
    await waitFor(() => expect(reads).toBe(1));
    await act(async () => {
      window.dispatchEvent(new Event("voiceflow-preview-ready"));
    });
    await waitFor(() => expect(reads).toBe(2));
    await act(async () => {
      second.resolve(bootstrap(makePreview("tx-current", "current transaction")));
      await second.promise;
    });
    expect(await screen.findByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("current transaction");

    await act(async () => {
      first.resolve(bootstrap(makePreview("tx-stale", "stale transaction")));
      await first.promise;
    });
    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toHaveValue("current transaction");
  });

  it.each([
    ["failed command", async () => { throw new Error("private native failure"); }],
    ["unknown outcome", async () => "unexpected outcome"],
  ])("shows a safe error for a %s without leaking details or replaying the action", async (_label, result) => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_action_preview_bootstrap") return bootstrap(makePreview("tx-error")) as never;
      if (command === "confirm_selected_action_preview") return result() as never;
      return undefined as never;
    });

    render(<PreviewApp />);
    await screen.findByRole("textbox", { name: "VoiceFlow 生成结果" });
    fireEvent.click(screen.getByRole("button", { name: "确认" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("无法完成这个文字操作。请检查后重试或取消。");
    expect(screen.queryByText("private native failure")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "confirm_selected_action_preview")).toHaveLength(1);
  });
});
