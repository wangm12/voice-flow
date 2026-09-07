import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { VoicePill } from "./VoicePill";
import { HUD_LABEL_FADE_MS, HUD_LABEL_OUT_MS, HUD_ORB_FADE_MS } from "./hudOrb";
import { CONTEXT_LABEL_VISIBLE_MS } from "./voicePillTokens";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  invokeMock.mockReset();
});

describe("VoicePill", () => {
  it("keeps in-progress words off the HUD while recording", () => {
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

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语");
    expect(document.querySelector(".voice-pill-caption")?.textContent ?? "").not.toContain("你好世界");
    expect(screen.getByRole("status").getAttribute("aria-label") ?? "").not.toContain("你好世界");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveClass("voice-pill-caption--partial");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveStyle({ maxWidth: "360px" });
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

  it("does not bring the dismissed app caption back when thinking restates the same context", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <VoicePill
        state="recording"
        contextLabel="Cursor · 代码"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · 代码");
    act(() => {
      vi.advanceTimersByTime(CONTEXT_LABEL_VISIBLE_MS);
    });
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();

    rerender(
      <VoicePill
        state="processing"
        phase="asr"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.4}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();

    rerender(
      <VoicePill
        state="processing"
        phase="cleanup"
        contextLabel="Cursor · 代码"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.65}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();

    rerender(
      <VoicePill
        state="idle"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    rerender(
      <VoicePill
        state="recording"
        contextLabel="Cursor · 代码"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · 代码");
  });

  it("renders 精确重打中 when the cascade accurate phase is in flight", () => {
    render(
      <VoicePill
        state="processing"
        phase="cascade_accurate"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.5}
        reduced
      />,
    );
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("精确重打中");
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
    expect(document.querySelector(".voice-pill__orb")?.getAttribute("data-orb-state")).toBe("shaping");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Thinking…");
    expect(document.querySelector(".voice-pill__dots--thinking")).not.toBeInTheDocument();
    expect(document.querySelector("[data-beam-size='line']")?.getAttribute("data-beam-color")).toBe("colorful");
    expect(document.querySelector("[data-beam-size='line']")?.getAttribute("data-beam-size")).toBe("line");
    expect(document.querySelector("[data-beam-size='line']")?.getAttribute("data-beam-strength")).toBe("0.7");
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

  it("does not leave prefetch words after the template caption dismisses", () => {
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

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("WeChat · 口语");
    act(() => {
      vi.advanceTimersByTime(CONTEXT_LABEL_VISIBLE_MS);
    });
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
    expect(screen.queryByText("你好世界")).not.toBeInTheDocument();
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

  it("hides the orb when input delivery cannot be verified", () => {
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Copied");
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Saved");
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
    expect(document.querySelector(".voice-pill__orb--dim")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__orb")?.getAttribute("data-orb-state")).toBe("breathing");
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Error");
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
    expect(document.querySelector(".voice-pill__center-state--active .voice-pill__orb")?.getAttribute("data-orb-state")).toBe("breathing");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Listening…");
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
    expect(document.querySelector(".voice-pill__orb")?.getAttribute("data-orb-state")).toBe("breathing");
    expect(document.querySelector(".voice-pill__orb")?.getAttribute("data-orb-paused")).toBe("true");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Thinking…");
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
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

  it("does not show cancel or send controls on the pill", () => {
    render(
      <VoicePill
        state="recording"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    expect(screen.queryByRole("button", { name: "取消录音" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "停止录音" })).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__orb")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Listening…");
    expect(document.querySelector("[data-beam-size='md']")?.getAttribute("data-beam-color")).toBe("colorful");
    expect(document.querySelector("[data-beam-size='md']")?.getAttribute("data-beam-size")).toBe("md");
    expect(document.querySelector("[data-beam-size='md']")?.getAttribute("data-beam-active")).toBe("false");
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Done");
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
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill--orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Copied");
  });

  it("fades the thinking orb out when paste falls back to the clipboard", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <VoicePill
        state="processing"
        phase="delivery"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );

    expect(document.querySelector(".voice-pill__orb")?.getAttribute("data-orb-state")).toBe("shaping");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Thinking…");

    rerender(
      <VoicePill
        state="copied"
        contextLabel={null}
        fallbackReason="paste_failed"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );

    expect(document.querySelector(".voice-pill--orb")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill--labeled")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-state--active")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__indicator--overlay")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__orb")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill-caption")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label--out")).toHaveTextContent("Thinking…");
    expect(document.querySelector(".voice-pill__label--in")).toHaveTextContent("Copied");

    act(() => {
      vi.advanceTimersByTime(HUD_LABEL_OUT_MS);
    });
    expect(document.querySelector(".voice-pill__label--out")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label--in")).toHaveTextContent("Copied");
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("已复制到剪贴板，请手动粘贴");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--reveal");

    act(() => {
      vi.advanceTimersByTime(HUD_LABEL_FADE_MS - HUD_LABEL_OUT_MS);
    });
    expect(document.querySelector(".voice-pill--labeled")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Copied");
    expect(document.querySelector(".voice-pill__label--in")).not.toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(Math.max(0, HUD_ORB_FADE_MS - HUD_LABEL_FADE_MS));
    });
    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill--orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill--labeled")).toBeInTheDocument();
  });

  it("does not flash the copied caption under a thinking pill", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <VoicePill
        state="processing"
        phase="delivery"
        contextLabel="Cursor · Code"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );

    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · Code");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveClass("voice-pill-caption--warning");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Thinking…");

    rerender(
      <VoicePill
        state="unverified"
        contextLabel="Cursor · Code"
        fallbackReason="paste_unverified"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );

    expect(document.querySelector(".voice-pill__label--out")).toHaveTextContent("Thinking…");
    expect(document.querySelector(".voice-pill--unverified")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("Cursor · Code");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--exit");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveClass("voice-pill-caption--warning");
    expect(document.querySelector(".voice-pill-caption")).not.toHaveTextContent("已复制，请按 ⌘V");

    act(() => {
      vi.advanceTimersByTime(HUD_LABEL_OUT_MS);
    });
    expect(document.querySelector(".voice-pill-caption")).toHaveTextContent("已复制，请按 ⌘V");
    expect(document.querySelector(".voice-pill-caption")).toHaveClass("voice-pill-caption--reveal");
    expect(document.querySelector(".voice-pill--unverified")).toBeInTheDocument();
  });

  it("hides the orb on done instead of keeping a thinking orbit", () => {
    render(
      <VoicePill
        state="done"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );

    expect(document.querySelector(".voice-pill__orb")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Done");
  });

  it("keeps the colorful md beam while listening and switches to a colorful line beam while thinking", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <VoicePill
        state="recording"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced={false}
      />,
    );
    const listenBeam = document.querySelector("[data-beam-size='md']");
    const thinkBeam = document.querySelector("[data-beam-size='line']");
    expect(listenBeam).toBeInstanceOf(HTMLElement);
    expect(thinkBeam).toBeInstanceOf(HTMLElement);
    expect(listenBeam?.getAttribute("data-beam-color")).toBe("colorful");
    expect(listenBeam?.getAttribute("data-beam-size")).toBe("md");
    expect(listenBeam?.getAttribute("data-beam-strength")).toBe("1");
    expect(listenBeam?.getAttribute("data-beam-duration")).toBe("3.2");
    expect(listenBeam?.getAttribute("data-beam-active")).toBe("true");
    expect(thinkBeam?.getAttribute("data-beam-active")).toBe("false");
    expect(listenBeam).toHaveStyle({ position: "absolute", width: "100%", height: "100%" });
    expect(document.querySelector(".voice-pill__beam-ghost")).toBeInTheDocument();
    expect((thinkBeam as HTMLElement).style.getPropertyValue("--beam-x-mock")).toBe("");

    rerender(
      <VoicePill
        state="processing"
        phase="asr"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.4}
        reduced={false}
      />,
    );
    expect(document.querySelectorAll("[data-border-beam]")).toHaveLength(2);
    expect(listenBeam?.getAttribute("data-beam-active")).toBe("false");
    expect(thinkBeam?.getAttribute("data-beam-color")).toBe("colorful");
    expect(thinkBeam?.getAttribute("data-beam-size")).toBe("line");
    expect(thinkBeam?.getAttribute("data-beam-strength")).toBe("0.7");
    expect(thinkBeam?.getAttribute("data-beam-active")).toBe("true");
    expect((thinkBeam as HTMLElement).style.getPropertyValue("--beam-x-mock")).toBe("0.0600");
    expect(document.querySelector(".voice-pill__progress--visible")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__label--out")).toHaveTextContent("Listening…");
    expect(document.querySelector(".voice-pill__label--in")).toHaveTextContent("Thinking…");

    act(() => {
      vi.advanceTimersByTime(500);
    });
    const mid = Number((thinkBeam as HTMLElement).style.getPropertyValue("--beam-x-mock"));
    expect(mid).toBeGreaterThan(0.06);
    expect(mid).toBeLessThan(0.44);

    rerender(
      <VoicePill
        state="copied"
        contextLabel={null}
        fallbackReason="paste_failed"
        selectedActionState={null}
        waveformLevels={[]}
        progress={1}
        reduced={false}
      />,
    );
    expect(document.querySelector(".voice-pill__beam-layer--exit")).toBeInTheDocument();
    expect(thinkBeam?.getAttribute("data-beam-active")).toBe("true");
    expect(Number((thinkBeam as HTMLElement).style.getPropertyValue("--beam-x-mock"))).toBeGreaterThanOrEqual(mid);

    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(thinkBeam?.getAttribute("data-beam-active")).toBe("false");

    rerender(
      <VoicePill
        state="rate_limited"
        retryAfterSecs={8}
        phase="waiting_retry"
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        progress={0.4}
        reduced={false}
      />,
    );
    expect(thinkBeam?.getAttribute("data-beam-active")).toBe("true");
    expect((thinkBeam as HTMLElement).style.getPropertyValue("--beam-x-mock")).not.toBe("");
  });
});
