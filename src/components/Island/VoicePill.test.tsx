import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VoicePill } from "./VoicePill";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("VoicePill", () => {
  it("shows a compact target label while recording", () => {
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

    expect(screen.getByText("Chrome Canary")).toBeInTheDocument();
    expect(screen.queryByText(/General/)).not.toBeInTheDocument();
  });

  it("keeps the target label visible across the starting-to-recording transition", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <VoicePill
        state="starting"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    rerender(
      <VoicePill
        state="recording"
        contextLabel="Chrome Canary · General"
        selectedActionState={null}
        waveformLevels={[]}
        progress={0}
        reduced
      />,
    );

    const caption = container.querySelector(".voice-pill-caption");
    expect(caption).not.toHaveClass("voice-pill-caption--empty");

    act(() => vi.advanceTimersByTime(1400));
    expect(caption).toHaveClass("voice-pill-caption--empty");
  });
});
