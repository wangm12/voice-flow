import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
import { VoicePill } from "./VoicePill";
import { CONTEXT_LABEL_VISIBLE_MS } from "./voicePillTokens";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  invokeMock.mockReset();
});

describe("VoicePill", () => {
  it("shows in-progress words while recording", () => {
    render(
      <VoicePill
        state="recording"
        contextLabel="WeChat · 口语"
        partialText="你好世界"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语 · 你好世界");
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("你好世界");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--partial");
    expect(document.querySelector(".voice-pill-caption")).toHaveStyle({ maxWidth: "360px" });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("keeps the compact caption width when recording without in-progress words", () => {
    render(
      <VoicePill
        state="recording"
        contextLabel="WeChat · 口语"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    const caption = document.querySelector(".voice-pill-caption");
    expect(caption).toHaveTextContent("WeChat · 口语");
    expect(caption).not.toHaveClass("voice-pill-caption--partial");
    expect(caption).not.toHaveStyle({ maxWidth: "360px" });
  });

  it("keeps error captions ahead of in-progress words", () => {
    render(
      <VoicePill
        state="error"
        contextLabel="WeChat · 口语"
        partialText="should not appear"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("语音输入失败，请重试");
    expect(screen.queryByText("should not appear")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("shows the context label while recording", () => {
    render(
      <VoicePill
        state="recording"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(screen.getByText("Chrome Canary · General")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Chrome Canary · General");
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("Chrome Canary · General");
  });

  it("shows the context label during processing", () => {
    render(
      <VoicePill
        state="processing"
        phase="cleanup"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Chrome Canary · General");
  });

  it("dismisses the template caption after a couple of seconds", () => {
    vi.useFakeTimers();
    render(
      <VoicePill
        state="recording"
        contextLabel="Chrome · 通用"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Chrome · 通用");
    act(() => {
      vi.advanceTimersByTime(CONTEXT_LABEL_VISIBLE_MS - 1);
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Chrome · 通用");
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
  });

  it("keeps in-progress words after the template caption dismisses", () => {
    vi.useFakeTimers();
    render(
      <VoicePill
        state="recording"
        contextLabel="WeChat · 口语"
        partialText="你好世界"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语 · 你好世界");
    act(() => {
      vi.advanceTimersByTime(CONTEXT_LABEL_VISIBLE_MS);
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("你好世界");
    expect(document.querySelector(".voice-pill-caption")?.textContent ?? "").not.toContain("WeChat · 口语");
  });

  it("shows the context label while starting", () => {
    render(
      <VoicePill
        state="starting"
        contextLabel="WeChat · 口语"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语");
  });

  it("uses an icon-only warning state when input delivery cannot be verified", () => {
    render(
      <VoicePill
        state="unverified"
        contextLabel="Cursor · Code"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "已复制，请按 ⌘V");
    expect(screen.queryByText("已复制，请按 ⌘V")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill--unverified")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-state--active .voice-pill__caution-icon")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-state--active .voice-pill__status-icon")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "撤销插入" })).not.toBeInTheDocument();
  });

  it("uses an archive indicator for history-only delivery", () => {
    render(
      <VoicePill
        state="history"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "已保存到历史");
    expect(screen.getByRole("status").querySelector("svg")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "撤销插入" })).not.toBeInTheDocument();
  });

  it("keeps degraded and unverified HUD states visible during the backend dwell", () => {
    const { rerender } = render(
      <VoicePill
        state="degraded"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--exit");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("部分结果已保存");

    rerender(
      <VoicePill
        state="unverified"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--exit");
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
  });

  it("renders exception captions only for limited, rate-limited, degraded, and error states", () => {
    const { rerender } = render(
      <VoicePill
        state="recording_limited"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("已达上限 · 按热键结束");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Chrome Canary · General");
    expect(document.querySelector(".voice-pill__wave--dim")).toBeInTheDocument();
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("已达上限");

    rerender(
      <VoicePill
        state="rate_limited"
        retryAfterSecs={12}
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.4}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("处理时间比平时长");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("12");

    rerender(
      <VoicePill
        state="error"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("语音输入失败，请重试");
    expect(screen.queryByText("Chrome Canary · General")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill--error")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__status-icon")).toBeInTheDocument();
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("语音输入失败，请重试");
  });

  it("ticks the rate-limit caption down once per second", () => {
    vi.useFakeTimers();
    render(
      <VoicePill
        state="rate_limited"
        retryAfterSecs={12}
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.4}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("12");
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("11");
  });

  it("shows selected-action listening copy during recording without hiding the waveform", () => {
    render(
      <VoicePill
        state="recording"
        contextLabel={null}
        selectedActionState="listening"
        waveformLevels={[0.2, 0.4, 0.3]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("正在听取操作");
    expect(document.querySelector(".voice-pill__center-state--active .voice-pill__wave")).toBeInTheDocument();
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("正在听取操作");
  });

  it("unhides the stack for idle selected-action guidance", () => {
    render(
      <VoicePill
        state="idle"
        contextLabel={null}
        selectedActionState="waiting_for_selection"
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--hidden");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("等待选中文本");
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("等待选中文本");
  });

  it("hides the stack when idle without any caption", () => {
    render(
      <VoicePill
        state="idle"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden");
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
  });

  it("hides undo when a completed insert did not arm a transaction", () => {
    render(
      <VoicePill
        state="done"
        contextLabel={null}
        selectedActionState={null}
        undoAvailable={false}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(screen.queryByRole("button", { name: "撤销插入" })).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("does not treat a non-success undo result as success", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    invokeMock.mockResolvedValue("not_available");
    render(
      <VoicePill
        state="done"
        contextLabel={null}
        selectedActionState={null}
        undoAvailable
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "撤销插入" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("undo_last_delivery");
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it("exposes undo delivery as an English accessible action", async () => {
    invokeMock.mockResolvedValue("success");
    render(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="done"
          contextLabel={null}
          selectedActionState={null}
          undoAvailable
          waveformLevels={[]}
          progress={1}
          reduced
        />
      </I18nProvider>,
    );

    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "Completed");
    const undo = screen.getByRole("button", { name: "Undo insertion" });
    fireEvent.click(undo);
    await act(async () => {
      await Promise.resolve();
    });
    expect(invokeMock).toHaveBeenCalledWith("undo_last_delivery");
  });

  it("keeps selected-action guidance in the caption chip", () => {
    render(
      <VoicePill
        state="idle"
        contextLabel={null}
        selectedActionState="accessibility_required"
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("需要辅助功能权限");
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("需要辅助功能权限");
  });

  it("surfaces delivery fallback reasons in the caption chip", () => {
    render(
      <VoicePill
        state="copied"
        contextLabel={null}
        fallbackReason="target_changed"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("输入目标已变化，文字已复制到剪贴板，请手动粘贴");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--wide");
    expect(document.querySelector(".voice-pill-caption")).toHaveStyle({ maxWidth: "360px" });
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").toContain("输入目标已变化");
  });

  it("keeps terminal selected-action captions visible without exit animation", () => {
    render(
      <VoicePill
        state="copied"
        contextLabel={null}
        selectedActionState="copied_instead"
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("目标变化，结果已复制");
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--exit");
  });

  it("shows completed replacement copy on done without exit animation", () => {
    render(
      <VoicePill
        state="done"
        contextLabel={null}
        selectedActionState="replaced"
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("已替换选中文本");
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--exit");
  });

  it("keeps ordinary copied state readable instead of exiting immediately", () => {
    render(
      <VoicePill
        state="copied"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent(
      "已复制到剪贴板，请手动粘贴",
    );
    expect(document.querySelector(".voice-pill-stack")).not.toHaveClass("voice-pill-stack--exit");
  });
});
