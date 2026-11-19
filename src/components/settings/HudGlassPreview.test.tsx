import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HudGlassPreview } from "./HudGlassPreview";

function mockMatchMedia(reducedTransparency: boolean) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: query === "(prefers-reduced-transparency: reduce)" ? reducedTransparency : false,
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  });
}

describe("HudGlassPreview", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("mounts the single .vf-glasswork-surface with matching data-kind and omits it when refract is off", () => {
    mockMatchMedia(false);
    const { container, rerender } = render(
      <HudGlassPreview backdrop="dark" refract />,
    );

    const root = container.querySelector(".vf-hud-glasswork-preview");
    const material = container.querySelector("[data-glasswork-material]");
    expect(root).toHaveAttribute("data-backdrop", "dark");
    expect(material).toHaveClass("vf-hud-glasswork-material", "vf-glasswork-surface");
    expect(material).toHaveAttribute("data-kind", "dark");

    rerender(<HudGlassPreview backdrop="light" refract={false} />);
    expect(root).toHaveAttribute("data-backdrop", "light");
    expect(root).toHaveAttribute("data-glass-mode", "off");
    expect(container.querySelector("[data-glasswork-material]")).toBeNull();
  });

  it("applies reduced-transparency attributes for the solid 96% fallback", () => {
    mockMatchMedia(true);
    const { container } = render(
      <HudGlassPreview backdrop="light" refract />,
    );

    const root = container.querySelector(".vf-hud-glasswork-preview");
    const material = container.querySelector("[data-glasswork-material]");
    expect(root).toHaveAttribute("data-glass-reduced-transparency", "true");
    expect(material).toHaveAttribute("data-reduced-transparency", "true");
    expect(material).not.toHaveAttribute("data-refract", "webgl");
  });
});
