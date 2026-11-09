import { describe, expect, it } from "vitest";
import {
  acceptsSessionGeneration,
  hudContextFromEvent,
  hudContextSourceFromEvent,
  hudPartialAfterState,
  hudPartialFromEvent,
  hudTranslationFromEvent,
  selectedActionStateForDictation,
} from "./IslandWindow";

describe("IslandWindow session generations", () => {
  it("keeps the session target while processing and clears it on completion or a new recording", () => {
    expect(hudTranslationFromEvent("recording", null, "ja")).toBe("ja");
    expect(hudTranslationFromEvent("processing", "ja")).toBe("ja");
    expect(hudTranslationFromEvent("starting", "ja")).toBeNull();
    expect(hudTranslationFromEvent("recording", "ja", null)).toBeNull();
    for (const state of ["idle", "error", "copied", "degraded", "done", "unverified"]) {
      expect(hudTranslationFromEvent(state, "ja", "ja")).toBeNull();
    }
  });
  it("accepts the current or a newer generation", () => {
    expect(acceptsSessionGeneration(4, 4)).toBe(true);
    expect(acceptsSessionGeneration(4, 5)).toBe(true);
  });

  it("rejects stale and legacy events without a generation", () => {
    expect(acceptsSessionGeneration(4, 3)).toBe(false);
    expect(acceptsSessionGeneration(4)).toBe(false);
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

  it("shows only the safe actual-used context source label and clears it on idle", () => {
    expect(hudContextSourceFromEvent("processing", null, {
      context_source: { source: "ocr", label: "On-device OCR", matched_rule_label: "Private Gmail rule" },
    })).toBe("ocr");
    expect(hudContextSourceFromEvent("processing", "ocr", {})).toBe("ocr");
    expect(hudContextSourceFromEvent("idle", "ocr", {
      context_source: { source: "ax", label: "AX text", matched_rule_label: "Private Gmail rule" },
    })).toBeNull();
    expect(hudContextSourceFromEvent("processing", null, {
      context_source: { source: "none", label: "None", matched_rule_label: "Private Gmail rule" },
    })).toBe("none");
    expect(hudContextSourceFromEvent("recording", "ax", {}, true)).toBeNull();
    expect(hudContextSourceFromEvent("processing", "ax", {}, true)).toBeNull();
    expect(hudContextSourceFromEvent("recording", "ax", {
      context_source: { source: "ax", label: "AX text" },
    })).toBeNull();
    expect(hudContextSourceFromEvent("processing", null, {
      context_source: { source: "toString", label: "unsafe", matched_rule_label: "unsafe" } as never,
    })).toBeNull();
  });

  it("clears HUD partials on idle or a newer session generation", () => {
    expect(hudPartialAfterState(4, 4, "idle", "hello")).toBeNull();
    expect(hudPartialAfterState(4, 5, "recording", "hello")).toBeNull();
    expect(hudPartialAfterState(4, 4, "processing", "hello")).toBe("hello");
    expect(hudPartialAfterState(4, 3, "idle", "hello")).toBe("hello");
  });
});
