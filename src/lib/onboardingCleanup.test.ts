import { describe, expect, it } from "vitest";
import { cleanupIntentForRoute, cleanupIntentPatch, cleanupIntentView, initialCleanupIntent } from "./onboardingCleanup";

describe("onboarding cleanup intent", () => {
  it("initializes first local setup Off without rewriting an existing route", () => {
    expect(cleanupIntentPatch(initialCleanupIntent(false, "on_device"))).toEqual({ cleanup_enabled: false });
    expect(cleanupIntentPatch(initialCleanupIntent(true, "on_device"))).toEqual({});
    expect(cleanupIntentView({ cleanup_enabled: true, cleanup_intensity: "standard" }, initialCleanupIntent(true, "on_device"))).toEqual({ intensity: "standard", pendingLocalDefault: false });
  });

  it("distinguishes a route default from an explicit independent cleanup choice", () => {
    const explicit = { kind: "explicit" as const, cleanup_enabled: true, cleanup_intensity: "light" as const };
    expect(cleanupIntentForRoute(explicit, "on_device", "on_device")).toBe(explicit);
    expect(cleanupIntentForRoute(explicit, "groq", "on_device")).toEqual({ kind: "initialize_local_off" });
    expect(cleanupIntentForRoute({ kind: "initialize_local_off" }, "on_device", "groq")).toEqual({ kind: "preserve" });
    expect(cleanupIntentPatch(explicit)).toEqual({ cleanup_enabled: true, cleanup_intensity: "light" });
  });
});
