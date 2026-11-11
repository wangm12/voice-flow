import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../types/settings";
import { useSettingsPersistence } from "./useSettingsPersistence";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const initialSettings = {
  schema_version: 1,
  api_key_configured: false,
  api_key_hint: null,
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "openai/gpt-oss-20b",
  language: "auto",
  ui_language: "zh",
  theme: "system",
  dictionary: [],
  chunk_threshold_secs: 25,
  chunk_length_secs: 35,
  long_output_mode: "paste",
  delivery_policy: "auto",
  keep_audio_days: 7,
  keep_history_days: 90,
  onboarded: true,
  cleanup_enabled: true,
  show_tray_icon: true,
  hotkey: "CmdOrControl+Alt+Space",
  activation_mode: "tap",
  context_enabled: true,
  browser_access_enabled: false,
  context_mappings: [],
  writing_modes: [],
  snippets: [],
  output_mode: "auto",
  translation_target_language: "en",
  input_device: "",
} as Settings;

function usePersistenceHarness() {
  const [settings, setSettings] = useState<Settings | null>(initialSettings);
  const persistence = useSettingsPersistence({ setSettings, formatError: String });
  return { settings, persistence };
}

describe("useSettingsPersistence", () => {
  afterEach(() => {
    vi.useRealTimers();
    invokeMock.mockReset();
  });

  it("does not requeue an older rejected microphone choice over a newer queued choice", async () => {
    const firstWrite = deferred<void>();
    const secondWrite = deferred<void>();
    invokeMock
      .mockImplementationOnce(() => firstWrite.promise)
      .mockImplementationOnce(() => secondWrite.promise);
    const { result } = renderHook(() => usePersistenceHarness());

    act(() => result.current.persistence.save({ input_device: "Mic A" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(result.current.persistence.unsavedFields).toContain("input_device");

    act(() => result.current.persistence.save({ input_device: "Mic B" }));
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(invokeMock).toHaveBeenCalledTimes(1); // B waits behind A.

    await act(async () => {
      firstWrite.reject(new Error("first write rejected"));
      await firstWrite.promise.catch(() => undefined);
    });
    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    expect(invokeMock).toHaveBeenLastCalledWith("update_settings_patch", {
      patch: { input_device: "Mic B" },
    });

    await act(async () => secondWrite.resolve());
    await waitFor(() => expect(result.current.persistence.unsavedFields).toEqual([]));
    expect(result.current.settings?.input_device).toBe("Mic B");
    expect(result.current.persistence.saveError).toBeNull();

    act(() => result.current.persistence.retryPendingSave());
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("keeps a committed edit when an older settings read resolves later", async () => {
    invokeMock.mockResolvedValue(undefined);
    const { result } = renderHook(() => usePersistenceHarness());
    const oldReadRevision = result.current.persistence.captureSettingsRevision();

    act(() => result.current.persistence.save({ input_device: "MacBook Pro Microphone" }));
    result.current.persistence.applySettingsSnapshot(initialSettings, oldReadRevision);
    expect(result.current.settings?.input_device).toBe("MacBook Pro Microphone");

    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(result.current.persistence.unsavedFields).toEqual([]));
    act(() => result.current.persistence.applySettingsSnapshot(initialSettings, oldReadRevision));
    expect(result.current.settings?.input_device).toBe("MacBook Pro Microphone");
  });

  it("persists an immediate preference through the queue without flushing unrelated edits", async () => {
    const immediateWrite = deferred<void>();
    const debouncedWrite = deferred<void>();
    invokeMock
      .mockImplementationOnce(() => immediateWrite.promise)
      .mockImplementationOnce(() => debouncedWrite.promise);
    const { result } = renderHook(() => usePersistenceHarness());

    act(() => {
      result.current.persistence.save({ input_device: "Mic A" });
      result.current.persistence.save({ hud_appearance: "liquid_glass" }, { immediate: true });
    });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(1));
    expect(invokeMock).toHaveBeenNthCalledWith(1, "update_settings_patch", {
      patch: { hud_appearance: "liquid_glass" },
    });
    expect(result.current.settings?.hud_appearance).toBe("liquid_glass");
    expect(result.current.persistence.unsavedFields).toEqual(expect.arrayContaining(["input_device", "hud_appearance"]));

    await act(async () => immediateWrite.resolve());
    await waitFor(() => expect(result.current.persistence.unsavedFields).toEqual(["input_device"]));
    await new Promise((resolve) => setTimeout(resolve, 350));

    expect(invokeMock).toHaveBeenNthCalledWith(2, "update_settings_patch", {
      patch: { input_device: "Mic A" },
    });
    await act(async () => debouncedWrite.resolve());
    await waitFor(() => expect(result.current.persistence.unsavedFields).toEqual([]));
  });
});
