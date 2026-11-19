import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../../lib/i18n";
import type { OnDeviceModelStatus } from "../../types/settings";
import { EngineConfigStep } from "./EngineConfigStep";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(cleanup);

const localModels: OnDeviceModelStatus[] = [
  {
    id: "qwen3-asr-0.6b", label: "Qwen3-ASR 0.6B", bytes: 1_500_000_000,
    sha256: null, state: "missing", inference_ready: false, downloaded_bytes: 0,
    error: null, error_category: null, revision: "qwen-rev", file_count: 0,
    platform_supported: true, runtime_status: "runtime_ready", loaded: false,
    download_available: true, language_note: "Auto or fixed Chinese/English.",
  },
  {
    id: "parakeet-tdt-0.6b-v3-8bit", label: "Parakeet TDT v3", bytes: 900_000_000,
    sha256: null, state: "missing", inference_ready: false, downloaded_bytes: 0,
    error: null, error_category: null, revision: "parakeet-rev", file_count: 0,
    platform_supported: true, runtime_status: "runtime_ready", loaded: false,
    download_available: true, language_note: "Auto only; the model card lists 25 European languages. The runtime does not report detected language.",
  },
];

function renderStep(language: string, locale: string, strictOfflineEnabled = false) {
  Object.defineProperty(navigator, "languages", { configurable: true, value: [locale] });
  Object.defineProperty(navigator, "language", { configurable: true, value: locale });
  const onAsrProviderChange = vi.fn();
  const onAsrModelChange = vi.fn();
  const onLanguageChange = vi.fn();
  render(
    <I18nProvider initialLanguage="zh">
      <EngineConfigStep
        asrProvider="groq"
        onAsrProviderChange={onAsrProviderChange}
        dashscopeRegion="beijing"
        onDashscopeRegionChange={vi.fn()}
        onDeviceReady={false}
        onDeviceModels={localModels}
        onDeviceActionId={null}
        onDeviceActionError={null}
        onDeviceAction={vi.fn()}
        onRefreshOnDeviceModels={vi.fn()}
        strictOfflineBlocked={false}
        strictOfflineEnabled={strictOfflineEnabled}
        asrModel="whisper-large-v3-turbo"
        onAsrModelChange={onAsrModelChange}
        language={language}
        onLanguageChange={onLanguageChange}
        configuredAsrKey={true}
        keyValue=""
        onKeyChange={vi.fn()}
        valid={null}
        validating={false}
        onValidate={vi.fn()}
      />
    </I18nProvider>,
  );
  return { onAsrProviderChange, onAsrModelChange, onLanguageChange };
}

describe("EngineConfigStep route recommendations", () => {
  it("offers the Parakeet automatic-language route for a European region", () => {
    const callbacks = renderStep("auto", "en-GB");
    fireEvent.click(screen.getByRole("button", { name: "选择 Parakeet TDT v3 自动检测路线" }));
    expect(callbacks.onAsrProviderChange).toHaveBeenCalledWith("on_device");
    expect(callbacks.onAsrModelChange).toHaveBeenCalledWith("parakeet-tdt-0.6b-v3-8bit");
    expect(callbacks.onLanguageChange).not.toHaveBeenCalled();
  });

  it("offers Qwen local and Soniox cloud routes for an Asian language", () => {
    const callbacks = renderStep("ja", "en-US");
    fireEvent.click(screen.getByRole("button", { name: "选择 Qwen3-ASR 本机路线" }));
    expect(callbacks.onAsrProviderChange).toHaveBeenCalledWith("on_device");
    expect(callbacks.onAsrModelChange).toHaveBeenCalledWith("qwen3-asr-0.6b");
    expect(callbacks.onLanguageChange).toHaveBeenCalledWith("auto");

    const recommendation = screen.getByRole("region", { name: "地区和语言路线建议" });
    fireEvent.click(within(recommendation).getByRole("button", { name: "选择 Soniox 多语言云端路线" }));
    expect(callbacks.onAsrProviderChange).toHaveBeenLastCalledWith("soniox");
    expect(callbacks.onAsrModelChange).toHaveBeenLastCalledWith("stt-rt-v5");
  });

  it("keeps the cloud recommendation disabled in strict offline mode even when local ASR is selected", () => {
    renderStep("auto", "en-SG", true);
    const recommendation = screen.getByRole("region", { name: "地区和语言路线建议" });
    expect(within(recommendation).getByRole("button", { name: "选择 Soniox 多语言云端路线" })).toBeDisabled();
    expect(screen.getByRole("note")).toHaveTextContent("严格离线模式会阻止 Soniox 云端路线。");
  });
});
