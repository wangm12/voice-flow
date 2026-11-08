import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ParticleText } from "./ParticleText";

type MockResizeObserver = { callback: ResizeObserverCallback; disconnect: ReturnType<typeof vi.fn> };

const observers: MockResizeObserver[] = [];
const mediaQueries: Array<{ query: string; listeners: Set<(event: MediaQueryListEvent) => void> }> = [];
let dpr = 1;
let rafId = 0;
const rafCallbacks = new Map<number, FrameRequestCallback>();

function makeContext() {
  return {
    beginPath: vi.fn(), arc: vi.fn((_x: number, _y: number, _radius: number, _startAngle: number, _endAngle: number, _counterclockwise?: boolean) => undefined), fill: vi.fn(), clearRect: vi.fn(), setTransform: vi.fn(),
    measureText: vi.fn(() => ({ width: 20, actualBoundingBoxLeft: 0, actualBoundingBoxRight: 20, actualBoundingBoxAscent: 10, actualBoundingBoxDescent: 2 })),
    fillText: vi.fn(), getImageData: vi.fn((_x: number, _y: number, width: number, height: number) => {
      const data = new Uint8ClampedArray(width * height * 4);
      for (let alpha = 3; alpha < data.length; alpha += 4) data[alpha] = 255;
      return { data, width, height };
    }),
    globalAlpha: 1, fillStyle: "", font: "", textBaseline: "",
  };
}

let contexts: ReturnType<typeof makeContext>[];

beforeEach(() => {
  vi.useFakeTimers();
  observers.length = 0;
  mediaQueries.length = 0;
  contexts = [];
  rafCallbacks.clear();
  rafId = 0;
  dpr = 1;
  vi.stubGlobal("ResizeObserver", class {
    callback: ResizeObserverCallback;
    disconnect = vi.fn();
    constructor(callback: ResizeObserverCallback) { this.callback = callback; observers.push(this); }
    observe() {}
  });
  vi.stubGlobal("MutationObserver", class { observe() {} disconnect = vi.fn(); });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { const id = ++rafId; rafCallbacks.set(id, callback); return id; });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => { rafCallbacks.delete(id); });
  Object.defineProperty(window, "devicePixelRatio", { configurable: true, get: () => dpr });
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 120, height: 40, top: 0, left: 0, right: 120, bottom: 40, x: 0, y: 0, toJSON: () => ({}) } as DOMRect);
  vi.spyOn(window, "getComputedStyle").mockReturnValue({ fontSize: "16px", fontWeight: "400", fontFamily: "sans-serif", color: "rgb(1, 2, 3)" } as CSSStyleDeclaration);
  vi.spyOn(document, "createElement").mockImplementation(((tag: string) => {
    const element = document.createElementNS("http://www.w3.org/1999/xhtml", tag) as HTMLElement;
    if (tag === "canvas") {
      const context = makeContext(); contexts.push(context);
      vi.spyOn(element as HTMLCanvasElement, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
    }
    return element;
  }) as typeof document.createElement);
  vi.stubGlobal("matchMedia", (query: string) => {
    const item = { query, listeners: new Set<(event: MediaQueryListEvent) => void>() };
    mediaQueries.push(item);
    return {
      matches: false, media: query, onchange: null,
      addEventListener: (_: string, listener: (event: MediaQueryListEvent) => void) => item.listeners.add(listener),
      removeEventListener: (_: string, listener: (event: MediaQueryListEvent) => void) => item.listeners.delete(listener),
      addListener: vi.fn(), removeListener: vi.fn(), dispatchEvent: vi.fn(),
    } as unknown as MediaQueryList;
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function settleSample() {
  await act(async () => { await Promise.resolve(); await Promise.resolve(); });
}

describe("ParticleText canvas resampling", () => {
  it("keeps the pending first reveal through initial ResizeObserver delivery, then resamples statically", async () => {
    const view = render(<ParticleText text="Voice" delay={50} />);
    const canvas = view.container.querySelector("canvas")!;
    await settleSample();
    expect(canvas.width).toBe(120);
    expect(canvas.height).toBe(40);

    act(() => observers[0].callback([], {} as ResizeObserver));
    await act(async () => { const [id, callback] = [...rafCallbacks.entries()][0]; rafCallbacks.delete(id); callback(16); await Promise.resolve(); await Promise.resolve(); });
    dpr = 2;
    const initialDprQuery = mediaQueries.find((item) => item.query === "(resolution: 1dppx)")!;
    act(() => [...initialDprQuery.listeners].forEach((listener) => listener({ matches: true } as MediaQueryListEvent)));
    await act(async () => { const [id, callback] = [...rafCallbacks.entries()][0]; rafCallbacks.delete(id); callback(32); await Promise.resolve(); await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(canvas.width).toBe(240);
    expect(canvas.height).toBe(80);
    expect(rafCallbacks.size).toBe(1);
    const [firstFrameId, firstFrame] = [...rafCallbacks.entries()][0];
    rafCallbacks.delete(firstFrameId);
    act(() => firstFrame(performance.now()));
    const firstRevealArcs = contexts[0].arc.mock.calls;
    expect(firstRevealArcs.length).toBeGreaterThan(0);
    expect(firstRevealArcs.some(([x, y]) => x < 0 || x > 120 || y < 0 || y > 40)).toBe(true);

    const [finishFrameId, finishFrame] = [...rafCallbacks.entries()][0];
    rafCallbacks.delete(finishFrameId);
    act(() => finishFrame(performance.now() + 1_000));
    expect(rafCallbacks.size).toBe(0);
    const completedArcCount = contexts[0].arc.mock.calls.length;

    dpr = 1;
    const secondDprQuery = mediaQueries.find((item) => item.query === "(resolution: 2dppx)")!;
    act(() => [...secondDprQuery.listeners].forEach((listener) => listener({ matches: true } as MediaQueryListEvent)));
    await act(async () => { const [id, callback] = [...rafCallbacks.entries()][0]; rafCallbacks.delete(id); callback(32); await Promise.resolve(); await Promise.resolve(); });
    expect(canvas.width).toBe(120);
    expect(canvas.height).toBe(40);
    expect(contexts[0].arc.mock.calls.length).toBeGreaterThan(completedArcCount);
    expect(contexts[0].arc.mock.calls.slice(completedArcCount).every(([x, y]) => x >= 0 && x <= 120 && y >= 0 && y <= 40)).toBe(true);
    expect(mediaQueries.some((item) => item.query === "(resolution: 1dppx)")).toBe(true);
    expect(mediaQueries.filter((item) => item.query === "(resolution: 1dppx)").pop()?.listeners.size).toBe(1);
  });

  it("disconnects observers and removes DPR listeners on unmount", async () => {
    const view = render(<ParticleText text="Voice" />);
    await settleSample();
    const dprQuery = mediaQueries.find((item) => item.query.startsWith("(resolution:"))!;
    expect(dprQuery.listeners.size).toBe(1);
    view.unmount();
    expect(observers[0].disconnect).toHaveBeenCalledOnce();
    expect(dprQuery.listeners.size).toBe(0);
    expect(rafCallbacks.size).toBe(0);
  });
});
