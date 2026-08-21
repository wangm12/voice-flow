import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivationModeSelector } from "./ActivationModeSelector";

afterEach(() => {
  cleanup();
});

describe("ActivationModeSelector", () => {
  it("labels the radiogroup for screen readers", () => {
    render(<ActivationModeSelector value="tap" onChange={() => undefined} />);
    expect(screen.getByRole("radiogroup", { name: "激活方式" })).toBeInTheDocument();
  });

  it("explains why modifier-only hotkeys lock double-tap", () => {
    render(<ActivationModeSelector value="double_tap" onChange={() => undefined} modifierOnly />);
    expect(screen.getByText(/功能键只能使用双击/)).toBeInTheDocument();
  });

  it("lets combo keys select hybrid", () => {
    const onChange = vi.fn();
    render(<ActivationModeSelector value="tap" onChange={onChange} />);
    const hybrid = screen.getByRole("radio", { name: /短按切换，按住说话/ });
    expect(hybrid).toBeEnabled();
    fireEvent.click(hybrid);
    expect(onChange).toHaveBeenCalledWith("hybrid");
  });

  it("does not let modifier-only hotkeys select hybrid", () => {
    render(<ActivationModeSelector value="double_tap" onChange={() => undefined} modifierOnly />);
    expect(screen.getByRole("radio", { name: /短按切换，按住说话/ })).toBeDisabled();
    expect(screen.getByRole("radio", { name: /按一下切换/ })).toBeDisabled();
    expect(screen.getByRole("radio", { name: /双击开始/ })).toBeChecked();
  });
});
