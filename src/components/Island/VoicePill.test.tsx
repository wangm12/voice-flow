import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VoicePill, type VoicePillRecoveryAction } from "./VoicePill";
import { DEFAULT_HUD_VISUALS, HUD_VOICE_BEAM, resolveVoiceBeamConfig } from "./hudOrb";
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

  it("uses a headphone glyph for custom AirPods names in the input-device subtitle", () => {
    renderPill({
      state: "recording",
      audioDeviceName: "Not ur airpods",
      audioDeviceSourceLabel: "Selected input",
      audioDeviceTitle: "Selected input: Not ur airpods",
      audioCaptureStatus: "receiving_audio",
    });

    expect(document.querySelector(".voice-pill__indicator svg")).toHaveAttribute("data-glyph", "mic");
    expect(document.querySelector(".voice-pill__subtitle svg")).toHaveAttribute("data-glyph", "headphones");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("Not ur airpods");
  });

  it.each(["no_audio_yet", "no_recent_audio", "device_error", "unavailable", "no_speech"] as const)(
    "keeps %s in the leading microphone failure cue ahead of a target avatar",
    (audioCaptureStatus) => {
      renderPill({
        state: "recording",
        audioCaptureStatus,
        recordingTargetIcon: "data:image/png;base64,aGVsbG8=",
      });

      expect(document.querySelector(".voice-pill__target-icon")).not.toBeInTheDocument();
      expect(document.querySelectorAll('[data-glyph="mic-off"]')).toHaveLength(1);
      expect(document.querySelector(".voice-pill__subtitle svg")).not.toBeInTheDocument();
    },
  );

  it("keeps startup waiting quiet under 250ms and surfaces waiting_first_frame after 250ms", () => {
    const { rerender } = renderPill({
      state: "recording",
      audioCaptureStatus: "waiting_first_frame",
      stageElapsedMs: 120,
      audioDeviceName: "AirPods Pro",
    });

    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Recording");
    expect(document.querySelector(".voice-pill__subtitle")).toHaveTextContent("AirPods Pro");
    expect(screen.queryByText("Waiting for microphone")).not.toBeInTheDocument();

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="recording"
          audioCaptureStatus="waiting_first_frame"
          stageElapsedMs={250}
          audioDeviceName="AirPods Pro"
          contextLabel={null}
          waveformLevels={[]}
          reduced
        />
      </I18nProvider>,
    );

    expect(document.querySelector(".voice-pill__target-icon")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__indicator svg")).toHaveAttribute("data-glyph", "mic");
    expect(document.querySelector(".voice-pill__subtitle svg")).not.toBeInTheDocument();
    expect(screen.getByText("Waiting for microphone")).toBeInTheDocument();
    expect(screen.getByText("No audio has arrived yet")).toBeInTheDocument();
  });

  it("keeps starting quiet under 250ms and reveals delayed wait notice at 250ms", () => {
    const { rerender } = renderPill({
      state: "starting",
      audioCaptureStatus: "waiting_first_frame",
      stageElapsedMs: 0,
    });

    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Recording");
    expect(screen.queryByText("Waiting for microphone")).not.toBeInTheDocument();

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="starting"
          audioCaptureStatus="waiting_first_frame"
          stageElapsedMs={250}
          contextLabel={null}
          waveformLevels={[]}
          reduced
        />
      </I18nProvider>,
    );

    expect(document.querySelector(".voice-pill__indicator svg")).toHaveAttribute("data-glyph", "mic");
    expect(document.querySelector(".voice-pill__subtitle svg")).not.toBeInTheDocument();
    expect(screen.getByText("Waiting for microphone")).toBeInTheDocument();
    expect(screen.getByText("No audio has arrived yet")).toBeInTheDocument();
  });

  it("preserves the target app avatar while waiting for the first frame", () => {
    renderPill({
      state: "recording",
      audioCaptureStatus: "waiting_first_frame",
      stageElapsedMs: 300,
      recordingTargetIcon: "data:image/png;base64,aGVsbG8=",
    });

    expect(document.querySelector(".voice-pill__target-icon")).toHaveAttribute("src", "data:image/png;base64,aGVsbG8=");
    expect(document.querySelector(".voice-pill__indicator svg")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__subtitle svg")).not.toBeInTheDocument();
    expect(screen.getByText("Waiting for microphone")).toBeInTheDocument();
    expect(screen.getByText("No audio has arrived yet")).toBeInTheDocument();
  });

  it("shows the waiting-for-audio source honestly after 250ms and keeps the original stop square", () => {
    renderPill({
      state: "recording",
      audioCaptureStatus: "waiting_first_frame",
      stageElapsedMs: 250,
      sessionGeneration: 42,
    });

    expect(screen.getByText("Waiting for microphone")).toBeInTheDocument();
    expect(screen.getByText("No audio has arrived yet")).toBeInTheDocument();
    const stop = screen.getByRole("button", { name: "Stop and process" });
    const icon = stop.querySelector("svg");
    expect(icon).toHaveAttribute("width", "20");
    expect(icon?.querySelector("rect")).toHaveAttribute("x", "5");
    expect(icon?.querySelector("rect")).toHaveAttribute("width", "10");
    expect(icon?.querySelector("rect")).toHaveAttribute("height", "10");
  });

  it("drives the colorful pill voice beam only while captured audio is fresh and mic input is healthy", () => {
    expect(HUD_VOICE_BEAM).toEqual(resolveVoiceBeamConfig(DEFAULT_HUD_VISUALS));
    const now = Date.now();
    const { rerender } = renderPill({
      state: "recording",
      audioCaptureStatus: "receiving_audio",
      waveformLevels: [0.7],
      lastAudioLevelAtMs: now - 100,
      clockNowMs: 0,
      beamTheme: "auto",
      reduced: false,
    });
    const voiceBeam = document.querySelector('[data-voice-beam="mock"]');
    expect(voiceBeam).toHaveAttribute("data-voice-active", "true");
    expect(voiceBeam).toHaveAttribute("data-voice-type", "pill");
    expect(voiceBeam).toHaveAttribute("data-voice-color", "colorful");
    expect(voiceBeam).toHaveAttribute("data-voice-theme", "auto");
    expect(Number(voiceBeam?.getAttribute("data-voice-level"))).toBeGreaterThan(0);

    // Microphone error suppresses the voice glow even if an earlier audio sample was recent.
    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="recording"
          audioCaptureStatus="device_error"
          contextLabel={null}
          waveformLevels={[0.7]}
          lastAudioLevelAtMs={now - 100}
          clockNowMs={0}
          reduced={false}
        />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "false");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-level", "0");

    // Stale audio (> 750ms) also turns the voice glow off and clears the level.
    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="recording"
          audioCaptureStatus="receiving_audio"
          contextLabel={null}
          waveformLevels={[0.7]}
          lastAudioLevelAtMs={Date.now() - 1_000}
          clockNowMs={0}
          reduced={false}
        />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "false");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-level", "0");

    // Simulated Demo Voice and custom studio preset drive VoiceBeam even when real mic samples are empty.
    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="recording"
          audioCaptureStatus="receiving_audio"
          contextLabel={null}
          waveformLevels={[]}
          lastAudioLevelAtMs={null}
          previewDemoLevel={0.75}
          visuals={{ colorVariant: "forest", voiceStyle: "prism", voiceIntensity: 1.25 }}
          clockNowMs={0}
          reduced={false}
        />
      </I18nProvider>,
    );
    const prismBeam = document.querySelector('[data-voice-beam="mock"]');
    const pill = document.querySelector(".voice-pill");
    const waveLayer = document.querySelector(".voice-pill > .voice-pill__voice-beam-layer");
    const copyBlock = document.querySelector(".voice-pill__copy");
    expect(waveLayer).not.toBeNull();
    expect(pill).not.toBeNull();
    expect(copyBlock).not.toBeNull();
    expect(waveLayer?.compareDocumentPosition(copyBlock!) ?? 0).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(prismBeam).toHaveAttribute("data-voice-active", "true");
    expect(prismBeam).toHaveAttribute("data-voice-color", "forest");
    expect(prismBeam).toHaveAttribute("data-voice-band-core", "#e6fff2");
    expect(prismBeam).toHaveAttribute("data-voice-band-mid", "#2ecc71");
    expect(prismBeam).toHaveAttribute("data-voice-flow", "84");
    expect(Number(prismBeam?.getAttribute("data-voice-level"))).toBeGreaterThan(0.5);
  });

  it("applies processing style and hue rotation for non-native BorderBeam color palettes independently from voice color", () => {
    renderPill({
      state: "processing",
      phase: "cleanup",
      visuals: { colorVariant: "mono", processingColorVariant: "forest", processingStyle: "brilliant_sweep" },
      reduced: false,
      sessionGeneration: 9,
    });
    const layer = document.querySelector<HTMLElement>(".voice-pill__processing-beam-layer");
    const lineBeam = document.querySelector('[data-beam-size="line"]');
    expect(lineBeam).toHaveAttribute("data-beam-color", "ocean");
    expect(lineBeam).toHaveAttribute("data-beam-duration", "1.45");
    expect(lineBeam).toHaveAttribute("data-beam-brightness", "1.55");
    expect(layer?.style.filter).toBe("hue-rotate(-95deg)");
  });

  it("collapses normal processing sub-phases to Processing and runs the colorful line beam", () => {
    const phases = [
      ["finalizing_audio", "Processing", "true"],
      ["asr", "Processing", "true"],
      ["soniox_recovery", "Processing", "true"],
      ["cascade_accurate", "Processing", "true"],
      ["cleanup", "Processing", "true"],
      ["delivery", "Processing", "true"],
      ["waiting_retry", "Rate limit retry", "false"],
    ] as const;
    const { rerender } = renderPill({ state: "processing", phase: phases[0][0], reduced: false, sessionGeneration: 8 });
    const lineBeam = document.querySelector('[data-beam-size="line"]');
    expect(lineBeam).toHaveAttribute("data-beam-color", "colorful");
    expect(lineBeam).toHaveAttribute("data-beam-duration", "2.4");
    expect(lineBeam).toHaveAttribute("data-beam-theme", "dark");
    expect(lineBeam).toHaveAttribute("data-beam-active", "true");

    for (const [phase, confirmedLabel, expectedBeamActive] of phases) {
      rerender(
        <I18nProvider initialLanguage="en">
          <VoicePill state="processing" phase={phase} contextLabel={null} waveformLevels={[]} reduced={false} sessionGeneration={8} />
        </I18nProvider>,
      );
      expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Processing");
      expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "true");

      rerender(
        <I18nProvider initialLanguage="en">
          <VoicePill state="processing" phase={phase} phaseConfirmed contextLabel={null} waveformLevels={[]} reduced={false} sessionGeneration={8} />
        </I18nProvider>,
      );
      expect(document.querySelector(".voice-pill__label")).toHaveTextContent(confirmedLabel);
      expect(screen.getByRole("button", { name: "Cancel dictation" })).toBeInTheDocument();
      expect(document.querySelector("[data-orb-state='working']")).toHaveAttribute("data-orb-speed", "1.00");
      expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", expectedBeamActive);
      expect(document.querySelector(".voice-pill__elapsed")).not.toBeInTheDocument();
      expect(document.querySelector(".voice-pill__subtitle")).not.toBeInTheDocument();
    }
  });

  it("pauses motion and hides active beams for reduced-motion users while honoring beamTheme", () => {
    const { rerender } = renderPill({
      state: "recording",
      audioCaptureStatus: "receiving_audio",
      waveformLevels: [0.8],
      lastAudioLevelAtMs: Date.now(),
      beamTheme: "auto",
      reduced: true,
      sessionGeneration: 8,
    });
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "false");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-theme", "auto");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-level", "0");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="processing" phase="asr" beamTheme="auto" contextLabel={null} waveformLevels={[]} reduced sessionGeneration={9} />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "false");
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-theme", "auto");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="rate_limited" phase="waiting_retry" phaseConfirmed beamTheme="light" contextLabel={null} waveformLevels={[]} reduced={false} sessionGeneration={9} />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "false");
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-theme", "light");
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

  it("uses a completion check cue in the done capsule and its details", () => {
    renderPill({ state: "done", detailsOpen: true });

    expect(document.querySelector(".voice-pill__indicator svg")).toHaveAttribute("data-glyph", "check");
    expect(document.querySelector(".voice-pill__details-icon svg")).toHaveAttribute("data-glyph", "check");
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
    expect(document.querySelector(".voice-pill__secondary--details")).toHaveClass("voice-pill__secondary--has-error");
  });

  it("renders recovery error inside the recovery copy column beside the message", () => {
    const recoveryAction: VoicePillRecoveryAction = {
      title: "Recording saved",
      message: "The recording was saved to History.",
      actionLabel: "View History",
      busy: false,
      error: "Could not save right now.",
      onAction: vi.fn(),
      onDismiss: vi.fn(),
    };
    renderPill({ state: "error", recoveryAction, detailsOpen: true });

    const copy = document.querySelector(".voice-pill__recovery-copy");
    expect(copy?.querySelector(".voice-pill__recovery-message")).toHaveTextContent("The recording was saved to History.");
    expect(copy?.querySelector(".voice-pill__recovery-error")).toHaveTextContent("Could not save right now.");
    expect(document.querySelector(".voice-pill__secondary--recovery")).toHaveClass("voice-pill__secondary--has-error");
  });

  it("crossfades the working orb out across processing -> done when motion is enabled", () => {
    vi.useFakeTimers();
    const { rerender } = renderPill({ state: "processing", phase: "cleanup", phaseConfirmed: true, reduced: false });
    expect(document.querySelector("[data-orb-state='working']")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-state--active [data-orb-state='working']")).toBeInTheDocument();

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="done" contextLabel={null} waveformLevels={[]} reduced={false} />
      </I18nProvider>,
    );
    expect(document.querySelector("[data-orb-state='working']")).toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-state--active [data-orb-state='working']")).not.toBeInTheDocument();
    expect(document.querySelector(".voice-pill__center-cue [data-glyph='check']")).toBeInTheDocument();

    act(() => { vi.advanceTimersByTime(220); });
    expect(document.querySelector("[data-orb-state='working']")).not.toBeInTheDocument();
  });

  it("updates intra-processing transitions steadily without opacity-hole crossfade while toggling line beam on rate limits", () => {
    vi.useFakeTimers();
    const { rerender } = renderPill({ state: "recording", reduced: false, sessionGeneration: 10 });
    expect(document.querySelectorAll(".voice-pill__label")).toHaveLength(1);
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "false");

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill state="processing" phase="finalizing_audio" phaseConfirmed={false} contextLabel={null} waveformLevels={[]} reduced={false} sessionGeneration={10} />
      </I18nProvider>,
    );
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "false");
    expect(document.querySelector(".voice-pill__voice-beam-layer")).toHaveClass("voice-pill__voice-beam-layer--exit");
    expect(document.querySelectorAll(".voice-pill__label")).toHaveLength(1);
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Processing");
    expect(document.querySelector(".voice-pill__label")).toHaveClass("voice-pill__label--in");
    expect(document.querySelector(".voice-pill__label--out")).not.toBeInTheDocument();
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "true");
    expect(document.querySelector(".voice-pill__processing-beam-layer")).not.toHaveClass("voice-pill__processing-beam-layer--exit");

    act(() => { vi.advanceTimersByTime(220); });
    const processingLabelNode = document.querySelector(".voice-pill__label");
    expect(processingLabelNode).not.toHaveClass("voice-pill__label--in");

    for (const [phase, chunkProgress] of [
      ["asr", null],
      ["asr", { done: 2, total: 4 }],
      ["cleanup", null],
    ] as const) {
      rerender(
        <I18nProvider initialLanguage="en">
          <VoicePill
            state="processing"
            phase={phase}
            phaseConfirmed
            chunkProgress={chunkProgress}
            contextLabel={null}
            waveformLevels={[]}
            reduced={false}
            sessionGeneration={10}
          />
        </I18nProvider>,
      );
      expect(document.querySelectorAll(".voice-pill__label")).toHaveLength(1);
      expect(document.querySelector(".voice-pill__label")).toBe(processingLabelNode);
      expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Processing");
      expect(document.querySelector(".voice-pill__label")).not.toHaveClass("voice-pill__label--in");
      expect(document.querySelector(".voice-pill__label--out")).not.toBeInTheDocument();
      expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "true");
    }

    rerender(
      <I18nProvider initialLanguage="en">
        <VoicePill
          state="rate_limited"
          phase="waiting_retry"
          phaseConfirmed
          contextLabel={null}
          waveformLevels={[]}
          reduced={false}
          sessionGeneration={10}
        />
      </I18nProvider>,
    );
    expect(document.querySelector(".voice-pill__label")).toHaveTextContent("Rate limit retry");
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-active", "false");
    expect(document.querySelector(".voice-pill__processing-beam-layer")).toHaveClass("voice-pill__processing-beam-layer--exit");
  });
});
