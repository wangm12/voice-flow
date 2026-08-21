import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Toggle } from "./Toggle";

afterEach(() => {
  cleanup();
});

describe("Toggle", () => {
  it("exposes switch semantics and emits the next value", () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="自动识别" />);

    const toggle = screen.getByRole("switch", { name: "自动识别" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    fireEvent.click(toggle);

    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("uses the success track and white thumb when enabled", () => {
    render(<Toggle checked onChange={vi.fn()} label="自动识别" />);

    const toggle = screen.getByRole("switch", { name: "自动识别" });
    expect(toggle.firstElementChild).toHaveClass("bg-success");
    expect(toggle.lastElementChild).toHaveClass("bg-success-foreground", "translate-x-5");
  });

  it("uses the semantic card color for the unchecked thumb", () => {
    render(<Toggle checked={false} onChange={vi.fn()} label="自动识别" />);

    const toggle = screen.getByRole("switch", { name: "自动识别" });
    expect(toggle.lastElementChild).toHaveClass("bg-card");
    expect(toggle.lastElementChild).not.toHaveClass("bg-zinc-300");
  });

  it("does not emit changes while disabled", () => {
    const onChange = vi.fn();
    render(<Toggle checked onChange={onChange} label="自动识别" disabled />);

    fireEvent.click(screen.getByRole("switch", { name: "自动识别" }));

    expect(onChange).not.toHaveBeenCalled();
  });
});
