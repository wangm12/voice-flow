import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Autocomplete } from "./Autocomplete";

afterEach(cleanup);
const options = [{ value: "model-a", label: "Model A" }, { value: "model-b", label: "Model B" }];

function Example() {
  const [value, setValue] = useState("");
  return <Autocomplete aria-label="Model" value={value} onValueChange={setValue} options={options} />;
}

describe("Autocomplete", () => {
  it("preserves arbitrary text instead of coercing it to a preset", () => {
    render(<Example />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "my-custom-model" } });
    expect(screen.getByRole("combobox")).toHaveValue("my-custom-model");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("chooses a suggestion with the keyboard while retaining input focus", async () => {
    render(<Example />);
    const input = screen.getByRole("combobox");
    input.focus();
    await screen.findByRole("listbox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(input).toHaveValue("model-a"));
    expect(input).toHaveFocus();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("dismisses suggestions with Escape without changing the draft", async () => {
    const change = vi.fn();
    render(<Autocomplete aria-label="Model" value="model" onValueChange={change} options={options} />);
    const input = screen.getByRole("combobox");
    input.focus();
    await screen.findByRole("listbox");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(input).toHaveValue("model");
    expect(change).not.toHaveBeenCalled();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("closes when disabled without committing a choice", async () => {
    const change = vi.fn();
    const { rerender } = render(<Autocomplete aria-label="Model" value="" onValueChange={change} options={options} />);
    screen.getByRole("combobox").focus();
    await screen.findByRole("listbox");
    rerender(<Autocomplete aria-label="Model" value="" disabled onValueChange={change} options={options} />);
    await waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
    expect(screen.getByRole("combobox")).toBeDisabled();
    expect(change).not.toHaveBeenCalled();
  });
});
