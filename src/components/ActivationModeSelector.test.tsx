import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivationModeSelector } from "./ActivationModeSelector";
afterEach(cleanup);
describe("ActivationModeSelector", () => {
  it.each(["Fn", "CmdOrControl+Alt+Space"])("offers two enabled modes for %s", (hotkey) => {
    render(<ActivationModeSelector value="tap" hotkey={hotkey} onChange={() => undefined} />);
    expect(screen.getByRole("radiogroup", { name: "录音方式" })).toBeInTheDocument();
    expect(screen.getAllByRole("radio")).toHaveLength(2);
    expect(screen.getByRole("radio", { name: "点按切换" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "按住说话" })).toBeEnabled();
    expect(screen.queryByText("双击开始")).not.toBeInTheDocument();
  });
  it("submits pure hold and explains the current binding", () => {
    const onChange = vi.fn();
    const { rerender } = render(<ActivationModeSelector value="tap" hotkey="Fn" onChange={onChange} />);
    fireEvent.click(screen.getByRole("radio", { name: "按住说话" }));
    expect(onChange).toHaveBeenCalledWith("hold_to_talk");
    rerender(<ActivationModeSelector value="hold_to_talk" hotkey="CmdOrControl+Alt+Space" onChange={onChange} />);
    expect(screen.getByText(/按住 .*Space 说话，松开后结束并转成文字/)).toBeVisible();
    expect(screen.getByText("Esc 取消。")).toBeVisible();
  });
  it("disables both modes while recording or saving", () => {
    render(<ActivationModeSelector value="tap" hotkey="Fn" disabled onChange={vi.fn()} />);
    for (const radio of screen.getAllByRole("radio")) expect(radio).toBeDisabled();
  });
});
