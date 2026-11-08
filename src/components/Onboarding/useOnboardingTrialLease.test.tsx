import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useOnboardingTrialLease } from "./useOnboardingTrialLease";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
afterEach(async () => { cleanup(); await act(async () => {}); invokeMock.mockReset(); });

function Fixture({ ownerKey }: { ownerKey: string }) {
  const { lease, releaseAll, releaseFailed } = useOnboardingTrialLease(ownerKey, () => {});
  return <><output data-testid="lease">{lease?.trial_id ?? "not_ready"}</output><output>{lease?.config_generation}</output><output>{releaseFailed ? "release_unknown" : "no_release_error"}</output><button onClick={() => void releaseAll().catch(() => {})}>release</button></>;
}

describe("native onboarding lease ownership", () => {
  it("releases a late acquired old lease by its own ID before acquiring the next", async () => {
    let resolveFirst!: (response: unknown) => void;
    let firstId = "";
    invokeMock.mockImplementation(async (_command, args) => {
      const request = args as { enabled: boolean; trialId: string };
      if (request.enabled && !firstId) {
        firstId = request.trialId;
        return new Promise((resolve) => { resolveFirst = resolve; });
      }
      return { trial_id: request.trialId, config_generation: "9007199254740993", enabled: request.enabled };
    });
    const view = render(<Fixture ownerKey="step-a" />);
    await waitFor(() => expect(firstId).not.toBe(""));
    view.rerender(<Fixture ownerKey="step-b" />);
    await act(async () => { resolveFirst({ trial_id: firstId, config_generation: "9007199254740993", enabled: true }); });
    await waitFor(() => expect(screen.getByTestId("lease")).not.toHaveTextContent("not_ready"));
    const currentId = screen.getByTestId("lease").textContent;
    expect(currentId).not.toBe(firstId);
    expect(screen.getByText("9007199254740993")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("set_onboarding_test_mode", { enabled: false, trialId: firstId });
    expect(invokeMock).not.toHaveBeenCalledWith("set_onboarding_test_mode", { enabled: false, trialId: currentId });
  });

  it("retains failed release ownership and retries the matching ID", async () => {
    let releaseFailures = 1;
    invokeMock.mockImplementation(async (_command, args) => {
      const request = args as { enabled: boolean; trialId: string };
      if (!request.enabled && releaseFailures-- > 0) throw new Error("transport_unavailable");
      return { trial_id: request.trialId, config_generation: "42", enabled: request.enabled };
    });
    render(<Fixture ownerKey="step-a" />);
    await waitFor(() => expect(screen.getByTestId("lease")).not.toHaveTextContent("not_ready"));
    const acquiredId = screen.getByTestId("lease").textContent;
    fireEvent.click(screen.getByText("release"));
    await screen.findByText("release_unknown");
    fireEvent.click(screen.getByText("release"));
    await screen.findByText("no_release_error");
    const releases = invokeMock.mock.calls.filter(([, args]) => (args as { enabled: boolean }).enabled === false);
    expect(releases.map(([, args]) => (args as { trialId: string }).trialId)).toEqual([acquiredId, acquiredId]);
  });
});
