import { act, cleanup, render, screen } from "@testing-library/react";
import type { Dispatch, SetStateAction } from "react";
import { afterEach, expect, it } from "vitest";
import { SessionDraftProvider, useSessionDraftState } from "./sessionDraft";

afterEach(cleanup);

it("keeps a late save acknowledgment after navigation in the window draft", () => {
  let update: Dispatch<SetStateAction<string>>;
  function Form() {
    const [draft, setDraft] = useSessionDraftState("form", "unsaved");
    update = setDraft;
    return <output>{draft || "Saved"}</output>;
  }
  const view = render(<SessionDraftProvider><Form /></SessionDraftProvider>);
  view.rerender(<SessionDraftProvider><span>Other page</span></SessionDraftProvider>);
  act(() => update(""));
  view.rerender(<SessionDraftProvider><Form /></SessionDraftProvider>);
  expect(screen.getByText("Saved")).toBeVisible();
});

it("does not let an old save acknowledgment erase a newer page instance's draft", () => {
  let update: Dispatch<SetStateAction<string>>;
  function Form() {
    const [draft, setDraft] = useSessionDraftState("form", "initial");
    update = setDraft;
    return <output>{draft}</output>;
  }
  const view = render(<SessionDraftProvider><Form /></SessionDraftProvider>);
  const oldUpdate = update!;
  view.rerender(<SessionDraftProvider><span>Other page</span></SessionDraftProvider>);
  view.rerender(<SessionDraftProvider><Form /></SessionDraftProvider>);
  act(() => update("newer edit"));
  view.rerender(<SessionDraftProvider><span>Other page</span></SessionDraftProvider>);
  act(() => oldUpdate(""));
  view.rerender(<SessionDraftProvider><Form /></SessionDraftProvider>);
  expect(screen.getByText("newer edit")).toBeVisible();
});
