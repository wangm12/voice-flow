import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { demoTiming } from "./demoTimeline";

export function useDemoPlayback(duration: number, resetKey: string) {
  const stageRef = useRef<HTMLDivElement>(null);
  const initialReducedMotion = useRef(
    window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  const elapsedRef = useRef(
    initialReducedMotion.current ? demoTiming.complete : 0,
  );
  const [elapsed, setElapsed] = useState(elapsedRef.current);
  const [paused, setPaused] = useState(initialReducedMotion.current);
  const [reducedMotion, setReducedMotion] = useState(
    initialReducedMotion.current,
  );
  const [inView, setInView] = useState(false);
  const [pageVisible, setPageVisible] = useState(!document.hidden);
  const [version, setVersion] = useState(0);
  const previousResetKey = useRef(resetKey);

  useLayoutEffect(() => {
    if (previousResetKey.current === resetKey) return;
    previousResetKey.current = resetKey;
    elapsedRef.current = reducedMotion ? demoTiming.complete : 0;
    setElapsed(elapsedRef.current);
    setVersion((previous) => previous + 1);
  }, [resetKey, reducedMotion]);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const observer = new IntersectionObserver(
      ([entry]) =>
        setInView(entry.isIntersecting && entry.intersectionRatio >= 0.18),
      { threshold: [0, 0.18] },
    );
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    function onMotionChange() {
      setReducedMotion(media.matches);
      setPaused(media.matches);
      elapsedRef.current = media.matches ? demoTiming.complete : 0;
      setElapsed(elapsedRef.current);
      setVersion((previous) => previous + 1);
    }
    function onVisibilityChange() {
      setPageVisible(!document.hidden);
    }
    media.addEventListener("change", onMotionChange);
    document.addEventListener("visibilitychange", onVisibilityChange);
    onVisibilityChange();
    // A preference can change between the first render and listener setup.
    if (media.matches !== initialReducedMotion.current) onMotionChange();
    return () => {
      media.removeEventListener("change", onMotionChange);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, []);

  const running = !paused && inView && pageVisible;

  useEffect(() => {
    if (!running) return;
    let frameId: number;
    let previousTime: number | undefined;
    let lastRender = 0;
    function tick(time: number) {
      if (previousTime !== undefined) elapsedRef.current += time - previousTime;
      previousTime = time;

      if (reducedMotion && elapsedRef.current >= demoTiming.complete) {
        elapsedRef.current = demoTiming.complete;
        setElapsed(elapsedRef.current);
        setPaused(true);
        return;
      }

      const wrapped = elapsedRef.current >= duration;
      if (wrapped) elapsedRef.current %= duration;
      if (wrapped || time - lastRender >= 50) {
        setElapsed(elapsedRef.current);
        lastRender = time;
      }
      frameId = window.requestAnimationFrame(tick);
    }
    frameId = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frameId);
  }, [running, duration, reducedMotion, version]);

  function replay() {
    elapsedRef.current = 0;
    setElapsed(0);
    setPaused(false);
    setVersion((previous) => previous + 1);
  }

  function toggle() {
    if (reducedMotion && paused && elapsedRef.current >= demoTiming.complete) {
      replay();
    } else {
      setPaused((previous) => !previous);
    }
  }

  function reset() {
    elapsedRef.current = reducedMotion ? demoTiming.complete : 0;
    setElapsed(elapsedRef.current);
    setVersion((previous) => previous + 1);
  }

  return {
    stageRef,
    elapsed,
    paused,
    running,
    reducedMotion,
    toggle,
    replay,
    reset,
  };
}
