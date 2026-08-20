import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ActivationModeSelector } from "./ActivationModeSelector";

afterEach(() => {
  cleanup();
});

describe("ActivationModeSelector", () => {
  it("explains why modifier-only hotkeys lock double-tap", () => {
    render(<ActivationModeSelector value="double_tap" onChange={() => undefined} modifierOnly />);
    expect(screen.getByText(/功能键只能使用双击/)).toBeInTheDocument();
  });
});
