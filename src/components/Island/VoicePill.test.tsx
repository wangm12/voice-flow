import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VoicePill } from "./VoicePill";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("VoicePill", () => {
  it("keeps the HUD text-free while recording", () => {
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

    expect(screen.queryByText("Chrome Canary")).not.toBeInTheDocument();
    expect(screen.queryByText(/General/)).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "录音中");
  });

  it("does not render a visible caption during processing", () => {
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
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
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

    expect(screen.getByRole("status")).toHaveAttribute("aria-label", "已尝试写入，请确认输入框内容");
    expect(screen.queryByText("已尝试写入，请确认输入框内容")).not.toBeInTheDocument();
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
    expect(document.querySelector(".voice-pill__wave--dim")).toBeInTheDocument();
    expect(screen.queryByText("Chrome Canary")).not.toBeInTheDocument();
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
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("语音输入失败，请重试");
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
});
