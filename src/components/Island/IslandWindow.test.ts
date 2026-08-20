import { describe, expect, it } from "vitest";
import { acceptsSessionGeneration } from "./IslandWindow";

describe("IslandWindow session generations", () => {
  it("accepts the current or a newer generation", () => {
    expect(acceptsSessionGeneration(4, 4)).toBe(true);
    expect(acceptsSessionGeneration(4, 5)).toBe(true);
  });

  it("rejects stale and legacy events without a generation", () => {
    expect(acceptsSessionGeneration(4, 3)).toBe(false);
    expect(acceptsSessionGeneration(4)).toBe(false);
  });
});
