import { describe, expect, it } from "vitest";
import {
  acceptsSessionGeneration,
  hudContextFromEvent,
  hudPartialAfterState,
  hudPartialFromEvent,
  hudProgressForState,
  normalizeChunkProgress,
  selectedActionStateForDictation,
} from "./IslandWindow";

describe("IslandWindow session generations", () => {
  it("accepts the current or a newer generation", () => {
    expect(acceptsSessionGeneration(4, 4)).toBe(true);
    expect(acceptsSessionGeneration(4, 5)).toBe(true);
  });

  it("rejects stale and legacy events without a generation", () => {
    expect(acceptsSessionGeneration(4, 3)).toBe(false);
    expect(acceptsSessionGeneration(4)).toBe(false);
  });

  it("keeps chunk progress bounded and accepts updates as chunks complete", () => {
    expect(normalizeChunkProgress(0, 8)).toEqual({ completed: 0, total: 8 });
    expect(normalizeChunkProgress(3, 8)).toEqual({ completed: 3, total: 8 });
    expect(normalizeChunkProgress(9, 8)).toBeNull();
    expect(normalizeChunkProgress(undefined, 8)).toBeNull();
  });

  it("clears terminal selected-action copy when a normal dictation starts", () => {
    expect(selectedActionStateForDictation("replaced", "starting")).toBeNull();
    expect(selectedActionStateForDictation("copied_instead", "recording")).toBeNull();
    expect(selectedActionStateForDictation("replaced", "idle")).toBeNull();
  });

  it("retains selected-action guidance through its active dictation phases", () => {
    expect(selectedActionStateForDictation("waiting_for_selection", "idle")).toBe("waiting_for_selection");
    expect(selectedActionStateForDictation("listening", "recording")).toBe("listening");
    expect(selectedActionStateForDictation("preparing_rewrite", "processing")).toBe("preparing_rewrite");
  });

  it("accepts HUD-only partial text for the current session", () => {
    expect(hudPartialFromEvent(4, 4, "recording", "  hello world  ")).toBe("hello world");
    expect(hudPartialFromEvent(4, 3, "recording", "stale")).toBeUndefined();
    expect(hudPartialFromEvent(4, 4, "idle", "too late")).toBeUndefined();
  });

  it("keeps thinking progress across a rate-limit wait instead of snapping to zero", () => {
    expect(hudProgressForState("rate_limited", "waiting_retry", 0.65)).toBe(0.65);
    expect(hudProgressForState("processing", "cleanup", 0.2)).toBe(0.65);
    expect(hudProgressForState("processing", "cascade_accurate", 0.35)).toBe(0.5);
    expect(hudProgressForState("copied", "idle", 0.65)).toBe(1);
    expect(hudProgressForState("recording", "idle", 0.65)).toBe(0);
  });

  it("keeps the active-app context when a thinking event omits it", () => {
    const current = {
      contextApp: "Cursor",
      contextStyle: "prompt_or_code",
      contextLabel: "Cursor · Code",
      cleanupIntensity: "heavy" as const,
    };
    expect(hudContextFromEvent("processing", current, {})).toEqual(current);
    expect(hudContextFromEvent("processing", current, { context_app: "Cursor", context_style: "prompt_or_code" })).toEqual({
      contextApp: "Cursor",
      contextStyle: "prompt_or_code",
      contextLabel: "Cursor · Code",
      cleanupIntensity: "heavy",
    });
    expect(hudContextFromEvent("processing", current, { cleanup_intensity: "light" })).toEqual({
      ...current,
      cleanupIntensity: "light",
    });
    expect(hudContextFromEvent("idle", current, { context_app: "Cursor" })).toEqual({
      contextApp: null,
      contextStyle: null,
      contextLabel: null,
      cleanupIntensity: null,
    });
  });

  it("clears HUD partials on idle or a newer session generation", () => {
    expect(hudPartialAfterState(4, 4, "idle", "hello")).toBeNull();
    expect(hudPartialAfterState(4, 5, "recording", "hello")).toBeNull();
    expect(hudPartialAfterState(4, 4, "processing", "hello")).toBe("hello");
    expect(hudPartialAfterState(4, 3, "idle", "hello")).toBe("hello");
  });
});
