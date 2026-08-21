import { act } from "@testing-library/react";

/** Flush scheduled requestAnimationFrame callbacks used by dialog focus restore. */
export async function flushAnimationFrames(count = 1) {
  for (let i = 0; i < count; i++) {
    await act(async () => {
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => resolve());
      });
    });
  }
}
