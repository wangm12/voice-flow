import { fireEvent, screen, waitFor, within } from "@testing-library/react";

/** Exercise the same dropdown and value callback as a user picking an option. */
export function selectOption(trigger: HTMLElement, value: string) {
  fireEvent.click(trigger);
  const list = document.getElementById(trigger.getAttribute("aria-controls") ?? "");
  if (!list) throw new Error("Dropdown did not open");
  const option = within(list).getAllByRole("option", { hidden: true }).find((item) => item.getAttribute("data-value") === value);
  if (!option) throw new Error(`Dropdown option not found: ${value}`);
  fireEvent.click(option);
}

export async function openSelect(trigger: HTMLElement) {
  await waitFor(() => { if (trigger.hasAttribute("disabled")) throw new Error("Dropdown is still disabled"); });
  fireEvent.click(trigger);
  return await screen.findByRole("listbox");
}

export function closeSelect(list: HTMLElement) {
  fireEvent.keyDown(list, { key: "Escape" });
}
