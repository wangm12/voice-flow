import { describe, expect, it } from "vitest";
import {
  applyLineBeamProgress,
  driveLineBeamWithJs,
  lineBeamFrame,
  releaseLineBeamDrive,
  stepSmoothThinkingProgress,
  THINKING_HOLD,
} from "./hudLineBeam";

describe("lineBeamFrame", () => {
  it("starts at the library's left-edge keyframe", () => {
    expect(lineBeamFrame(0)).toEqual({ x: 0.06, w: 0.5 });
  });

  it("hits the mid-pill keyframe at half progress", () => {
    expect(lineBeamFrame(0.5)).toEqual({ x: 0.5, w: 1.5 });
  });

  it("ends at the library's right-edge keyframe", () => {
    expect(lineBeamFrame(1)).toEqual({ x: 0.94, w: 0.5 });
  });

  it("interpolates between authored travel stops", () => {
    expect(lineBeamFrame(0.25).x).toBeCloseTo(0.3, 5);
    expect(lineBeamFrame(0.25).w).toBeCloseTo(1.2, 5);
  });
});

describe("stepSmoothThinkingProgress", () => {
  it("does not snap to a sparse pipeline tick in one frame", () => {
    expect(stepSmoothThinkingProgress(0, 0.45, 1 / 60)).toBeLessThan(0.05);
  });

  it("keeps moving while the reported target stalls", () => {
    let visual = 0.05;
    for (let i = 0; i < 60; i += 1) {
      visual = stepSmoothThinkingProgress(visual, 0.05, 1 / 60);
    }
    expect(visual).toBeGreaterThan(0.1);
    expect(visual).toBeLessThanOrEqual(THINKING_HOLD);
  });

  it("never goes backwards", () => {
    expect(stepSmoothThinkingProgress(0.4, 0.1, 0.1)).toBeGreaterThanOrEqual(0.4);
  });

  it("reaches the end after completion is reported", () => {
    let visual = 0.85;
    for (let i = 0; i < 90; i += 1) {
      visual = stepSmoothThinkingProgress(visual, 1, 1 / 60);
    }
    expect(visual).toBeGreaterThan(0.99);
  });
});

describe("applyLineBeamProgress", () => {
  it("writes the line custom properties onto the beam host", () => {
    const el = document.createElement("div");
    el.setAttribute("data-beam", "mock");
    applyLineBeamProgress(el, 0.5);
    expect(el.style.getPropertyValue("--beam-x-mock")).toBe("0.5000");
    expect(el.style.getPropertyValue("--beam-w-mock")).toBe("1.5000");
    expect(el.style.getPropertyValue("--beam-edge-mock")).toBe("1");
  });

  it("skips redundant writes when the travel frame has not moved", () => {
    const el = document.createElement("div");
    el.setAttribute("data-beam", "mock");
    applyLineBeamProgress(el, 0.5);
    el.style.setProperty("--beam-edge-mock", "changed");
    applyLineBeamProgress(el, 0.5);
    expect(el.style.getPropertyValue("--beam-edge-mock")).toBe("changed");
  });

  it("clears the override when progress scrubbing stops", () => {
    const el = document.createElement("div");
    el.setAttribute("data-beam", "mock");
    applyLineBeamProgress(el, 0.2);
    applyLineBeamProgress(el, null);
    expect(el.style.getPropertyValue("--beam-x-mock")).toBe("");
    expect(el.style.getPropertyValue("--beam-edge-mock")).toBe("");
  });
});

describe("driveLineBeamWithJs", () => {
  it("drops library travel so JS owns --beam-x", () => {
    const el = document.createElement("div");
    el.setAttribute("data-beam", "abc");
    driveLineBeamWithJs(el);
    expect(el.style.animation).toContain("beam-breathe-abc");
    expect(el.style.animation).not.toContain("beam-travel");
    expect(el.style.animation).not.toContain("beam-edge-fade");
    const first = el.style.animation;
    driveLineBeamWithJs(el);
    expect(el.style.animation).toBe(first);
    releaseLineBeamDrive(el);
    expect(el.style.animation).toBe("");
  });
});
