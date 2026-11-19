import { vi } from "vitest";
import { createElement, type Ref } from "react";
import "@testing-library/jest-dom/vitest";

// Browser geometry and pointer APIs used by accessible portaled controls.
if (!globalThis.ResizeObserver) globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
if (typeof HTMLElement !== "undefined") {
  if (!HTMLElement.prototype.scrollIntoView) HTMLElement.prototype.scrollIntoView = vi.fn();
  if (!HTMLElement.prototype.hasPointerCapture) HTMLElement.prototype.hasPointerCapture = () => false;
  if (!HTMLElement.prototype.setPointerCapture) HTMLElement.prototype.setPointerCapture = vi.fn();
  if (!HTMLElement.prototype.releasePointerCapture) HTMLElement.prototype.releasePointerCapture = vi.fn();
}

vi.mock("thinking-orbs", () => ({
  ThinkingOrb: () => null,
}));

vi.mock("border-beam", () => ({
  BorderBeam: ({
    children,
    colorVariant,
    size,
    strength,
    brightness,
    active,
    duration,
    theme,
    className,
    style,
    ref,
  }: {
    children?: unknown;
    colorVariant?: string;
    size?: string;
    strength?: number;
    brightness?: number;
    active?: boolean;
    duration?: number;
    theme?: string;
    className?: string;
    style?: Record<string, unknown>;
    ref?: Ref<HTMLDivElement>;
  }) => createElement(
    "div",
    {
      ref,
      className,
      style,
      "data-beam": "mock",
      "data-border-beam": "",
      "data-beam-color": colorVariant ?? "",
      "data-beam-size": size ?? "",
      "data-beam-strength": strength == null ? "" : String(strength),
      "data-beam-brightness": brightness == null ? "" : String(brightness),
      "data-beam-duration": duration == null ? "" : String(duration),
      "data-beam-theme": theme ?? "",
      "data-beam-active": active === false ? "false" : "true",
    },
    children as never,
  ),
}));

vi.mock("voice-glow", () => ({
  VoiceBeam: ({
    children,
    type,
    colorVariant,
    bandColors,
    flow,
    attack,
    release,
    reach,
    level,
    theme,
    active,
    className,
    style,
    ref,
  }: {
    children?: unknown;
    type?: string;
    colorVariant?: string;
    bandColors?: { core?: string; above?: string; mid?: string; below?: string };
    flow?: number;
    attack?: number;
    release?: number;
    reach?: number;
    level?: number | (() => number);
    theme?: string;
    active?: boolean;
    className?: string;
    style?: Record<string, unknown>;
    ref?: Ref<HTMLDivElement>;
  }) => {
    const resolvedLevel = typeof level === "function" ? level() : (level ?? 0);
    return createElement(
      "div",
      {
        ref,
        className,
        style,
        "data-voice-beam": "mock",
        "data-voice-type": type ?? "default",
        "data-voice-color": colorVariant ?? "colorful",
        "data-voice-band-core": bandColors?.core ?? "",
        "data-voice-band-mid": bandColors?.mid ?? "",
        "data-voice-flow": flow == null ? "" : String(flow),
        "data-voice-attack": attack == null ? "" : String(attack),
        "data-voice-release": release == null ? "" : String(release),
        "data-voice-reach": reach == null ? "" : String(reach),
        "data-voice-theme": theme ?? "",
        "data-voice-active": active === false ? "false" : "true",
        "data-voice-level": String(resolvedLevel),
      },
      children as never,
    );
  },
}));
