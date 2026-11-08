import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { flushAnimationFrames } from "../test/flushRaf";
import { getFocusableElements } from "./focusTrap";

afterEach(cleanup);

function Pair({ lower = true, upper = true, onLower = () => undefined, onUpper = () => undefined }: { lower?: boolean; upper?: boolean; onLower?: () => void; onUpper?: () => void }) {
  return <main><div data-testid="background"><button>Settings</button></div>
    <ConfirmDialog open={lower} title="Lower" description="Lower modal" confirmLabel="Confirm lower" cancelLabel="Close lower" onConfirm={onLower} onCancel={onLower} />
    <ConfirmDialog open={upper} title="Upper" description="Upper modal" confirmLabel="Confirm upper" cancelLabel="Close upper" onConfirm={onUpper} onCancel={onUpper} />
  </main>;
}

describe("modal coordination", () => {
  it("exposes and dismisses only the top modal", async () => {
    const lower = vi.fn(), upper = vi.fn();
    render(<Pair onLower={lower} onUpper={upper} />);
    await flushAnimationFrames(1);
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog", { name: "Upper" })).toHaveFocus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(upper).toHaveBeenCalledOnce();
    expect(lower).not.toHaveBeenCalled();
  });

  it("keeps the background isolated when a lower modal closes first and restores it after the last", async () => {
    const view = render(<Pair />);
    const background = screen.getByTestId("background");
    expect(background.inert).toBe(true);
    const upperAction = screen.getByRole("button", { name: "Confirm upper" });
    upperAction.focus();
    view.rerender(<Pair lower={false} />);
    await flushAnimationFrames(1);
    expect(background.inert).toBe(true);
    expect(background).toHaveAttribute("aria-hidden", "true");
    expect(upperAction).toHaveFocus();
    view.rerender(<Pair lower={false} upper={false} />);
    await flushAnimationFrames(1);
    expect(background.inert).toBe(false);
    expect(background).not.toHaveAttribute("aria-hidden");
    expect(screen.getByRole("button", { name: "Settings" })).toBeVisible();
  });

  it("reactivates the underlying modal when the top closes", async () => {
    const view = render(<Pair />);
    view.rerender(<Pair upper={false} />);
    await flushAnimationFrames(1);
    expect(screen.getByRole("dialog", { name: "Lower" })).toHaveFocus();
    expect(screen.getByTestId("background").inert).toBe(true);
  });

  it("returns to the next surviving row after deleting the opener", async () => {
    function List() {
      const [rows, setRows] = useState([1, 2]);
      const [deleting, setDeleting] = useState<number | null>(null);
      return <main><div>{rows.map((id) => <div key={id} data-focus-row><button onClick={() => setDeleting(id)}>Delete {id}</button></div>)}</div>
        <ConfirmDialog open={deleting !== null} title="Delete" description="Delete row?" confirmLabel="Delete now" cancelLabel="Cancel" onCancel={() => setDeleting(null)} onConfirm={() => { setRows((current) => current.filter((id) => id !== deleting)); setDeleting(null); }} />
      </main>;
    }
    render(<List />);
    const opener = screen.getByRole("button", { name: "Delete 1" });
    opener.focus(); fireEvent.click(opener);
    await flushAnimationFrames(1);
    fireEvent.click(screen.getByRole("button", { name: "Delete now" }));
    await flushAnimationFrames(1);
    expect(screen.getByRole("button", { name: "Delete 2" })).toHaveFocus();
  });

  it("does not put collapsed or hidden fields into the Tab loop", () => {
    const { container } = render(<div><details><summary>Details</summary><input aria-label="Collapsed" /></details><div hidden><button>Hidden</button></div><button>Visible</button></div>);
    const elements = getFocusableElements(container);
    expect(elements).not.toContain(screen.getByLabelText("Collapsed"));
    expect(elements).toContain(screen.getByRole("button", { name: "Visible" }));
  });
});
