import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VoicePill, type VoicePillRecoveryAction } from "./VoicePill";
import { HUD_LISTENING_BEAM, HUD_THINKING_BEAM } from "./hudOrb";
import { I18nProvider } from "../../lib/i18n";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function renderPill(props: Partial<ComponentProps<typeof VoicePill>> = {}) {
  return render(
    <I18nProvider initialLanguage="en">
      <VoicePill
        state="recording"
        contextLabel={null}
        waveformLevels={[]}
        reduced
        {...props}
      />
    </I18nProvider>,
  );
}

describe("VoicePill", () => {
  it("never shows the replaced label when the delivery state is unverified", () => {
    renderPill({
      state: "unverified",
      selectedActionState: "replaced",
      deliveryMethod: "paste_unverified",
      fallbackReason: "paste_unverified",
    });

    expect(screen.queryByText(/replaced/i)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("已替换选中文本");
    expect(document.body.textContent).not.toMatch(/replaced/i);
  });

  it("keeps the 220×48 primary capsule and real recording target fixed", () => {
    renderPill({
      state: "recording",
      sessionGeneration: 31,
      stageElapsedMs: 65_000,
      audioDeviceName: "AirPods Pro",
      audioDeviceSourceLabel: "Selected input",
      audioDeviceTitle: "Selected input: AirPods Pro",
      audioCaptureStatus: "receiving_audio",
      recordingTargetIcon: "data:image/png;base64,aGVsbG8=",
    });

    expect(document.querySelector(".voice-pill")).toHaveStyle({ width: "220px", height: "48px" });
    expect(document.querySelector(".voice-pill__target-icon")).toHaveAttribute("src", "data:image/png;base64,aGVsbG8=");
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Recording");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveAttribute("title", "Selected input: AirPods Pro");
    expect(screen.getByText("01:05")).toBeInTheDocument();
  });

  it("shows the waiting-for-audio source honestly and keeps the original stop square", () => {
    renderPill({ state: "recording", audioCaptureStatus: "waiting_first_frame", sessionGeneration: 42 });

    expect(screen.getByText("Waiting for microphone")).toBeInTheDocument();
    expect(screen.getByText("No audio has arrived yet")).toBeInTheDocument();
    const stop = screen.getByRole("button", { name: "Stop and process" });
    const icon = stop.querySelector("svg");
    expect(icon).toHaveAttribute("width", "20");
    expect(icon?.querySelector("rect")).toHaveAttribute("x", "5");
    expect(icon?.querySelector("rect")).toHaveAttribute("width", "10");
    expect(icon?.querySelector("rect")).toHaveAttribute("height", "10");
  });

  it("animates the mono listening beam only while captured audio is fresh", () => {
    expect(HUD_LISTENING_BEAM).toEqual({ size: "md", colorVariant: "mono", strength: 1, duration: 1.96, brightness: 1.3 });
    const now = Date.now();
    const { rerender } = renderPill({
      state: "recording",
      audioCaptureStatus: "receiving_audio",
      waveformLevels: [0.7],
      lastAudioLevelAtMs: now - 100,
      clockNowMs: 0,
      reduced: false,
    });
    expect(document.querySelector('[data-beam-size="md"]')).toHaveAttribute("data-beam-active", "true");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="recording"
          contextLabel={null}
          waveformLevels={[0.7]}
          lastAudioLevelAtMs={Date.now() - 1_000}
          clockNowMs={0}
          reduced={false}
        />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-beam-size="md"]')).toHaveAttribute("data-beam-active", "false");
  });

  it("uses one processing label and one cancel affordance across every backend phase", () => {
    expect(HUD_THINKING_BEAM).toEqual({ size: "line", colorVariant: "mono", strength: 0.65, duration: 3.1, brightness: 1.3 });
    const phases = ["finalizing_audio", "asr", "soniox_recovery", "cascade_accurate", "cleanup", "delivery", "waiting_retry"] as const;
    const { rerender } = renderPill({ state: "processing", phase: phases[0], reduced: false, sessionGeneration: 8 });

    for (const phase of phases) {
      rerender(
        <I18nProvider initialLanguage="en">
          <VoicePill state="processing" phase={phase} phaseConfirmed contextLabel={null} waveformLevels={[]} reduced={false} sessionGeneration={8} />
        </I18nProvider>,
      );
      expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Processing");
      expect(screen.getByRole("button", { name: "Cancel dictation" })).toBeInTheDocument();
      expect(document.querySelector("[data-orb-state='working']")).toHaveAttribute("data-orb-speed", "1.00");
      expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "true");
      expect(document.querySelector(".voice-pill__elapsed")).not.toBeInTheDocument();
      expect(document.querySelector(".voice-pill__subtitle")).not.toBeInTheDocument();
    }
  });

  it("pauses motion for reduced-motion users", () => {
    renderPill({ state: "processing", phase: "asr", reduced: true });
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "false");
  });

  it("provides a real generation-bound microphone settings action in the detail tray", () => {
    const onDetailsAction = vi.fn();
    renderPill({
      state: "error",
      fallbackReason: "microphone_required",
      detailsOpen: true,
      sessionGeneration: 55,
      onDetailsAction,
    });

    expect(document.querySelector(".voice-pill")).toHaveStyle({ width: "220px", height: "48px" });
    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Microphone permission is needed");
    fireEvent.click(screen.getByRole("button", { name: "Open Microphone settings" }));
    expect(onDetailsAction).toHaveBeenCalledWith("microphone", 55);
  });

  it.each(["preparing_rewrite", "preview_ready"])("keeps unverified %s text actions honest in details and announcements", (selectedActionState) => {
    const onDetailsAction = vi.fn();
    renderPill({
      state: "unverified",
      selectedActionState,
      fallbackReason: "paste_unverified",
      detailsOpen: true,
      sessionGeneration: 56,
      onDetailsAction,
    });

    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Input attempted");
    expect(document.querySelector(".voice-pill__details-copy")).not.toHaveTextContent(/saved|copied|pasted/i);
    expect(document.querySelector(".voice-pill__details-copy")).toHaveTextContent("Check the input field first");
    expect(document.querySelector(".voice-pill__details-copy")).not.toHaveTextContent(/history/i);
    expect(screen.queryByRole("button", { name: /history/i })).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("Check the input field first");
    expect(document.querySelector(".voice-pill__subtitle")).not.toHaveTextContent(/preparing rewrite/i);
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).not.toMatch(/history|preparing rewrite/i);
    expect(screen.getByRole("status")).not.toHaveTextContent(/history|preparing rewrite/i);
    expect(onDetailsAction).not.toHaveBeenCalled();
  });

  it("keeps the History recovery action for ordinary unverified dictation", () => {
    const onDetailsAction = vi.fn();
    renderPill({
      state: "unverified",
      fallbackReason: "paste_unverified",
      detailsOpen: true,
      sessionGeneration: 57,
      onDetailsAction,
    });

    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Input attempted");
    expect(screen.getByRole("button", { name: "Check History" })).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__details-copy")).toHaveTextContent("History");
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).toContain("History");
    fireEvent.click(screen.getByRole("button", { name: "Check History" }));
    expect(onDetailsAction).toHaveBeenCalledWith("history", 57);
  });

  it("announces critical cleanup alongside an unverified paste warning", () => {
    renderPill({
      state: "unverified",
      fallbackReason: "paste_unverified",
      deliveryMethod: "paste_unverified",
      cleanupStatus: "critical_memory_pressure",
      detailsOpen: true,
    });

    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Input attempted");
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("paste was not retried");
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("System memory pressure is critical");
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).toContain("System memory pressure is critical");
  });

  it("announces the neutral local-memory notice alongside an unverified paste warning", () => {
    renderPill({
      state: "unverified",
      fallbackReason: "paste_unverified",
      deliveryMethod: "paste_unverified",
      cleanupStatus: "local_memory_unavailable",
      detailsOpen: true,
    });

    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Input attempted");
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("paste was not retried");
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("The local memory budget is insufficient");
    expect(document.querySelector(".voice-pill")?.getAttribute("aria-label")).toContain("The local memory budget is insufficient");
    expect(document.querySelector(".voice-pill__details-text")).not.toHaveTextContent("pressure is critical");
  });

  it("does not imply saved text from a generic delivery failure", () => {
    renderPill({ state: "error", fallbackReason: "delivery_failed", detailsOpen: true });
    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Text was not entered");
    expect(document.querySelector(".voice-pill__details-copy")).not.toHaveTextContent(/saved/i);
  });

  it("shows the confirmed outcome in the subtitle instead of truncating it into the cause title", () => {
    const { rerender } = renderPill({
      state: "copied",
      deliveryMethod: "clipboard",
      fallbackReason: "target_changed",
    });
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("The input target changed");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("Text copied");
    expect(document.querySelector(".voice-pill__subtitle")).not.toHaveTextContent("The target changed");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="history" contextLabel={null} waveformLevels={[]} reduced deliveryMethod="history" fallbackReason="target_changed" />
      </I18nProvider>,
    );
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("The input target changed");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("Saved to History");
  });

  it("shows recovery actions only with a concrete recovery payload", () => {
    const onAction = vi.fn();
    const recoveryAction: VoicePillRecoveryAction = {
      title: "Recording saved",
      message: "The recording was saved to History.",
      actionLabel: "View History",
      busy: false,
      error: null,
      onAction,
      onDismiss: vi.fn(),
    };
    renderPill({ state: "error", recoveryAction, detailsOpen: true });
    fireEvent.click(screen.getByRole("button", { name: "View History" }));
    expect(onAction).toHaveBeenCalledOnce();
    expect(document.querySelector(".voice-pill-stack")).toHaveStyle({ width: "320px", height: "116px" });
  });

  it("lets the primary arrow open and close a recovery tray", () => {
    const onDetailsOpenChange = vi.fn();
    const recoveryAction: VoicePillRecoveryAction = {
      title: "Recording saved",
      message: "The recording was saved to History.",
      actionLabel: "View History",
      busy: false,
      error: null,
      onAction: vi.fn(),
      onDismiss: vi.fn(),
    };
    const { rerender } = renderPill({ state: "error", recoveryAction, onDetailsOpenChange });

    expect(screen.queryByText("The recording was saved to History.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "View details" }));
    expect(onDetailsOpenChange).toHaveBeenCalledWith(true);

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="error" contextLabel={null} waveformLevels={[]} reduced detailsOpen recoveryAction={recoveryAction} onDetailsOpenChange={onDetailsOpenChange} />
      </I18nProvider>,
    );
    expect(screen.getByText("The recording was saved to History.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close details" }));
    expect(onDetailsOpenChange).toHaveBeenCalledWith(false);
  });

  it("keeps a folded recovery ticket reachable while the HUD state is idle", () => {
    const onDetailsOpenChange = vi.fn();
    const recoveryAction: VoicePillRecoveryAction = {
      title: "Recording saved",
      message: "The recording was saved to History.",
      actionLabel: "View History",
      busy: false,
      error: null,
      onAction: vi.fn(),
      onDismiss: vi.fn(),
    };
    renderPill({ state: "idle", recoveryAction, onDetailsOpenChange });

    expect(document.querySelector(".voice-pill-stack")).not.toHaveAttribute("aria-hidden", "true");
    fireEvent.click(screen.getByRole("button", { name: "View details" }));
    expect(onDetailsOpenChange).toHaveBeenCalledWith(true);
  });

  it("shows a transport failure before a generic microphone subtitle", () => {
    renderPill({
      state: "recording",
      audioCaptureStatus: "no_recent_audio",
      detailsOpen: true,
      actionError: "Could not stop recording. Try again.",
    });

    expect(document.querySelector(".voice-pill__details-title")).toHaveTextContent("Microphone is not responding");
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("Could not stop recording. Try again.");
    expect(document.querySelector(".voice-pill__details-text")).not.toHaveTextContent("Check the connection and try again");
  });

  it("does not offer an idle-only destination while recording", () => {
    renderPill({
      state: "recording",
      audioCaptureStatus: "no_recent_audio",
      detailsOpen: true,
      actionError: "Could not stop recording. Try again.",
    });

    expect(document.querySelector(".voice-pill__details-action")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__details-text")).toHaveTextContent("Could not stop recording. Try again.");
  });

  it("hides immediately when the owning state is dismissed or confirmed", () => {
    const { rerender } = renderPill({ state: "error", sessionGeneration: 9 });
    expect(document.querySelector(".voice-pill-stack")).not.toHaveAttribute("aria-hidden", "true");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="idle" contextLabel={null} waveformLevels={[]} reduced hideImmediately sessionGeneration={9} />
      </I18nProvider>,
    );
    expect(document.querySelector(".voice-pill-stack")).toHaveAttribute("aria-hidden", "true");
    expect(document.querySelector(".voice-pill-stack")).toHaveClass("voice-pill-stack--hidden-now");
  });

  it("announces recovery action failures without relying on visual detail text", () => {
    renderPill({ state: "error", detailsOpen: true, detailsActionError: "Could not open History. Try again." });
    expect(screen.getByRole("status")).toHaveTextContent("Could not open History. Try again.");
  });
});
