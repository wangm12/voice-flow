import { describe, expect, it } from "vitest";
import { recommendOnboardingRoute } from "./onboardingRouteRecommendations";

describe("onboarding route recommendations", () => {
  it("recommends Parakeet for European regions even in automatic language mode", () => {
    expect(recommendOnboardingRoute("auto", "en-GB")).toEqual({
      kind: "european_local",
      systemRegion: "GB",
      selectedLanguage: "auto",
    });
  });

  it("lets an explicitly selected European language drive the local route suggestion", () => {
    expect(recommendOnboardingRoute("fr", "en-US")?.kind).toBe("european_local");
  });

  it("recommends Qwen and a multilingual cloud route for Asian regions", () => {
    expect(recommendOnboardingRoute("auto", "en-SG")).toEqual({
      kind: "asia_multilingual",
      systemRegion: "SG",
      selectedLanguage: "auto",
    });
  });

  it("uses an explicitly selected Asian language when the system region is elsewhere", () => {
    expect(recommendOnboardingRoute("ja", "en-US")?.kind).toBe("asia_multilingual");
  });

  it("does not guess a region for a locale without an explicit region", () => {
    expect(recommendOnboardingRoute("auto", "en")).toBeNull();
  });

  it("does not let an incompatible fixed language inherit a regional recommendation", () => {
    expect(recommendOnboardingRoute("ar", "de-DE")).toBeNull();
  });
});
