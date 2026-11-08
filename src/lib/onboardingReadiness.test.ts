import { describe, expect, it } from "vitest";
import { currentReadiness, invalidatePermissionReadiness, parseOnboardingPermissions, isNativeGeneration, matchesTrialReceipt, readinessFact } from "./onboardingReadiness";

describe("onboarding source receipts", () => {
  it("keeps credential, runtime and app result claims independent", () => {
    const facts = [readinessFact("credential_configured", "passed", "get_settings", "7", "setup-a"), readinessFact("runtime_capability", "passed", "list_on_device_models", "7", "setup-a"), readinessFact("in_app_trial", "passed", "dictation://onboarding-result", "7", "setup-a")];
    expect(currentReadiness(facts, "7", "setup-a").map((fact) => fact.stage)).toEqual(["credential_configured", "runtime_capability", "in_app_trial"]);
    expect(facts.some((fact) => fact.stage === "provider_probe" || fact.stage === "model_loaded" || fact.stage === "external_delivery")).toBe(false);
  });

  it("invalidates receipts after config changes or reopening setup", () => {
    const facts = [readinessFact("microphone_frames", "passed", "microphone-check://state:check-a", "7", "setup-a")];
    expect(currentReadiness(facts, "8", "setup-a")).toEqual([]);
    expect(currentReadiness(facts, "7", "setup-b")).toEqual([]);
  });

  it("preserves large native generations and rejects lossy numbers", () => {
    expect(isNativeGeneration("18446744073709551615")).toBe(true);
    expect(isNativeGeneration("18446744073709551616")).toBe(false);
    expect(isNativeGeneration("01")).toBe(false);
    expect(isNativeGeneration(42)).toBe(false);
  });

  it("requires a current native trial identity and generation", () => {
    const lease = { trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "8" };
    expect(matchesTrialReceipt({ final_text: "old" }, lease)).toBe(false);
    expect(matchesTrialReceipt({ trial_id: "22222222-2222-4222-8222-222222222222", config_generation: "8", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true, final_text: "old" }, lease)).toBe(false);
    expect(matchesTrialReceipt({ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "7", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true, final_text: "old" }, lease)).toBe(false);
    expect(matchesTrialReceipt({ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "8", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true, final_text: "current" }, lease)).toBe(true);
    expect(matchesTrialReceipt({ trial_id: "11111111-1111-4111-8111-111111111111", config_generation: "8", recording_generation: "7", trigger_source: "native_hotkey", trial_current: true, final_text: "current" }, null)).toBe(false);
  });
  it("preserves original receipts but makes permission-invalidated current presentation unknown", () => {
    const original = [readinessFact("credential_configured", "passed", "get_settings", "7", "s"), readinessFact("hotkey_registered", "passed", "actual_registration", "7", "s"), readinessFact("model_loaded", "passed", "actual_load", "7", "s"), readinessFact("in_app_trial", "passed", "actual_receipt", "7", "s")];
    const facts = invalidatePermissionReadiness(original, { source: "check_permissions", reason: "permission_changed" });
    expect(facts[3].state).toBe("passed"); expect(facts[3].source).toBe("actual_receipt");
    expect(facts[3].permission_invalidation).toEqual({ source: "check_permissions", reason: "permission_changed" });
    expect(currentReadiness(facts, "7", "s").map(({ stage, state }) => [stage, state])).toEqual([["credential_configured", "passed"], ["hotkey_registered", "passed"], ["model_loaded", "passed"], ["in_app_trial", "unknown"]]);
    const regrant = invalidatePermissionReadiness(facts, { source: "check_permissions", reason: "permission_changed" });
    expect(currentReadiness(regrant, "7", "s")[3].state).toBe("unknown");
    expect(original[3].permission_invalidation).toBeUndefined();
  });

  it.each([
    { input: undefined, reason: "unavailable" },
    { input: { microphone: true }, reason: "malformed" },
    { input: { microphone: true, microphone_status: "authorized", accessibility: true, screen_recording: "yes" }, reason: "malformed" },
    { input: { microphone: false, microphone_status: "unknown", accessibility: true }, reason: "unknown_status" },
    { input: { microphone: true, microphone_status: "denied", accessibility: true }, reason: "conflicting_status" },
  ])("keeps permission readback uncertainty and source fields: $reason", ({ input, reason }) => {
    const observation = parseOnboardingPermissions(input);
    expect(observation).toEqual({ state: "unknown", permissions: null, source_value: input, reason });
  });

});
