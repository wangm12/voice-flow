/** Setup intent; saved cleanup configuration remains owned by the settings API. */
import type { Settings } from "../types/settings";

export type CleanupIntensity = NonNullable<Settings["cleanup_intensity"]>;
export const cleanupIntensities: readonly CleanupIntensity[] = ["auto", "off", "light", "standard", "heavy"];
export function isCleanupIntensity(value: unknown): value is CleanupIntensity {
  return typeof value === "string" && cleanupIntensities.includes(value as CleanupIntensity);
}
export type OnboardingCleanupIntent =
  | { kind: "preserve" }
  | { kind: "initialize_local_off" }
  | { kind: "explicit"; cleanup_enabled: boolean; cleanup_intensity: CleanupIntensity };

export function initialCleanupIntent(onboarded: boolean, provider: string): OnboardingCleanupIntent {
  return !onboarded && provider === "on_device" ? { kind: "initialize_local_off" } : { kind: "preserve" };
}

export function cleanupIntentForRoute(intent: OnboardingCleanupIntent, previous: string, next: string): OnboardingCleanupIntent {
  if (previous === next) return intent;
  if (next === "on_device") return { kind: "initialize_local_off" };
  return intent.kind === "initialize_local_off" ? { kind: "preserve" } : intent;
}

export function cleanupIntentPatch(intent: OnboardingCleanupIntent): { cleanup_enabled?: boolean; cleanup_intensity?: CleanupIntensity } {
  if (intent.kind === "preserve") return {};
  if (intent.kind === "initialize_local_off") return { cleanup_enabled: false };
  return { cleanup_enabled: intent.cleanup_enabled, cleanup_intensity: intent.cleanup_intensity };
}

export function cleanupIntentView(settings: { cleanup_enabled?: unknown; cleanup_intensity?: unknown }, intent: OnboardingCleanupIntent) {
  const values = { ...settings, ...cleanupIntentPatch(intent) };
  const intensity = isCleanupIntensity(values.cleanup_intensity) ? values.cleanup_intensity : "auto";
  return { intensity: values.cleanup_enabled === false ? "off" as const : intensity, pendingLocalDefault: intent.kind === "initialize_local_off" };
}
