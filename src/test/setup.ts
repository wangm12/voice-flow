import { vi } from "vitest";
import { createElement, type Ref } from "react";
import "@testing-library/jest-dom/vitest";

vi.mock("thinking-orbs", () => ({
  ThinkingOrb: () => null,
}));

vi.mock("border-beam", () => ({
  BorderBeam: ({
    children,
    colorVariant,
    size,
    strength,
    active,
    duration,
    className,
    style,
    ref,
  }: {
    children?: unknown;
    colorVariant?: string;
    size?: string;
    strength?: number;
    active?: boolean;
    duration?: number;
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
      "data-beam-duration": duration == null ? "" : String(duration),
      "data-beam-active": active === false ? "false" : "true",
    },
    children as never,
  ),
}));

