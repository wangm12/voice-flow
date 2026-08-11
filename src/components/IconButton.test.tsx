import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { IconButton } from "./IconButton";

describe("IconButton", () => {
  it("exposes the action name to assistive technology and hover tooltip", () => {
    render(<IconButton label="复制" icon={<span aria-hidden="true">⧉</span>} />);

    const button = screen.getByRole("button", { name: "复制" });
    expect(button).toHaveAttribute("title", "复制");
    expect(button).toHaveAttribute("data-tooltip", "复制");
  });
});
