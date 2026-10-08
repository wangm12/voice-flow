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

  it("keeps a neutral checked track and a white thumb independent of action labels", () => {
    render(<Toggle checked onChange={vi.fn()} label="自动识别" />);

    const toggle = screen.getByRole("switch", { name: "自动识别" });
    expect(toggle.firstElementChild).toHaveClass("bg-toggle-checked");
    expect(toggle.lastElementChild).toHaveClass("bg-toggle-thumb", "translate-x-5");
  });

  it("keeps the unchecked thumb visible on the neutral track", () => {
    render(<Toggle checked={false} onChange={vi.fn()} label="自动识别" />);

    const toggle = screen.getByRole("switch", { name: "自动识别" });
    expect(toggle.lastElementChild).toHaveClass("bg-toggle-idle-thumb");
    expect(toggle.lastElementChild).not.toHaveClass("bg-zinc-300");
  });

  it("does not emit changes while disabled", () => {
    const onChange = vi.fn();
    render(<Toggle checked onChange={onChange} label="自动识别" disabled />);

    fireEvent.click(screen.getByRole("switch", { name: "自动识别" }));

    expect(onChange).not.toHaveBeenCalled();
  });
});
