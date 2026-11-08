import { useEffect, useRef, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb, type OrbState } from "thinking-orbs";

type Phase = "recording" | "processing";
type Theme = "dark" | "light";

const orbOptions: Array<{ value: OrbState; label: string }> = [
  { value: "working", label: "working" },
  { value: "solving", label: "solving" },
  { value: "shaping", label: "shaping" },
];
const validOrbStates = new Set<OrbState>(["working", "solving", "shaping"]);
const query = new URLSearchParams(window.location.search);

function initialOrbState(): OrbState {
  const value = query.get("orbState") as OrbState | null;
  return value && validOrbStates.has(value) ? value : "working";
}

function initialPhase(): Phase {
  return query.get("phase") === "processing" ? "processing" : "recording";
}

function HudCapsule({
  phase,
  orbState,
  beamActive,
  motionAllowed,
  freshFrames,
}: {
  phase: Phase;
  orbState: OrbState;
  beamActive: boolean;
  motionAllowed: boolean;
  freshFrames: boolean;
}) {
  const processing = phase === "processing";
  const targetOrbState: OrbState = processing ? orbState : "breathing";
  const [renderedOrbState, setRenderedOrbState] = useState<OrbState>(targetOrbState);
  const [orbShifting, setOrbShifting] = useState(false);
  const renderedOrbStateRef = useRef(renderedOrbState);
  const transitionGenerationRef = useRef(0);
  renderedOrbStateRef.current = renderedOrbState;
  useEffect(() => {
    const generation = ++transitionGenerationRef.current;
    let switchTimer: number | undefined;
    let settleTimer: number | undefined;
    const settle = () => {
      if (generation !== transitionGenerationRef.current) return;
      setRenderedOrbState(targetOrbState);
      setOrbShifting(false);
    };
    if (!motionAllowed) {
      settle();
    } else if (renderedOrbStateRef.current === targetOrbState) {
      setOrbShifting(false);
    } else {
      setOrbShifting(true);
      switchTimer = window.setTimeout(() => {
        if (generation === transitionGenerationRef.current) setRenderedOrbState(targetOrbState);
      }, 80);
      settleTimer = window.setTimeout(settle, 190);
    }
    return () => {
      if (switchTimer !== undefined) window.clearTimeout(switchTimer);
      if (settleTimer !== undefined) window.clearTimeout(settleTimer);
      if (generation === transitionGenerationRef.current) transitionGenerationRef.current++;
    };
  }, [targetOrbState, motionAllowed]);
  const capsuleRadius = 24;
  return (
    <BorderBeam
      className="beam-wrap"
      style={{ width: 220, height: 48 }}
      size="line"
      colorVariant="mono"
      theme="dark"
      staticColors
      strength={1}
      brightness={1.3}
      duration={3.1}
      borderRadius={capsuleRadius}
      active={beamActive}
    >
      <div
        className={`hud-shell hud-shell--${phase}`}
        style={{ width: 220, height: 48, borderRadius: capsuleRadius }}
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        <span className="sr-only">{processing ? "处理中" : "录音中，AirPods Pro，00:08"}</span>
        <span className={`orb-dock${orbShifting ? " orb-dock--shifting" : ""}`} aria-hidden="true">
          <span className="orb-slot">
            <ThinkingOrb
              state={renderedOrbState}
              size={20}
              theme="dark"
              paused={!motionAllowed || (!processing && !freshFrames)}
              aria-hidden="true"
            />
          </span>
        </span>

        <span className="copy-dock" aria-hidden="true">
          <span className="copy-label copy-label--recording">录音中</span>
          <span className="copy-label copy-label--processing">处理中</span>
          <span className="recording-device">AirPods Pro</span>
        </span>
        <time className="recording-timer" aria-hidden={processing}>00:08</time>
      </div>
    </BorderBeam>
  );
}

function App() {
  const [phase, setPhase] = useState<Phase>(initialPhase);
  const [looping, setLooping] = useState(query.get("loop") !== "off");
  const [orbState, setOrbState] = useState<OrbState>(initialOrbState);
  const [freshFrames, setFreshFrames] = useState(query.get("frames") !== "stale");
  const [manualReduced, setManualReduced] = useState(query.get("motion") === "reduced");
  const [systemReduced, setSystemReduced] = useState(false);
  const [theme, setTheme] = useState<Theme>(query.get("theme") === "light" ? "light" : "dark");
  const [visible, setVisible] = useState(false);
  const stageRef = useRef<HTMLElement>(null);
  const reducedMotion = manualReduced || systemReduced;

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setSystemReduced(media.matches);
    update();
    media.addEventListener?.("change", update);
    return () => media.removeEventListener?.("change", update);
  }, []);

  useEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    let intersects = false;
    const updateVisibility = () => {
      setVisible(intersects && document.visibilityState === "visible");
    };
    const observer = "IntersectionObserver" in window
      ? new IntersectionObserver((entries) => {
          intersects = entries.some((entry) => entry.isIntersecting);
          updateVisibility();
        })
      : null;
    if (observer) observer.observe(element);
    else {
      intersects = true;
      updateVisibility();
    }
    const onVisibility = () => updateVisibility();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      observer?.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  useEffect(() => {
    if (!looping || !visible || reducedMotion) return;
    const timer = window.setTimeout(() => {
      setPhase((current) => current === "recording" ? "processing" : "recording");
    }, 3600);
    return () => window.clearTimeout(timer);
  }, [looping, phase, visible, reducedMotion]);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("phase", phase);
    params.set("loop", looping ? "on" : "off");
    params.set("orbState", orbState);
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("motion", manualReduced ? "reduced" : "full");
    params.set("theme", theme);
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // The prototype remains usable when URL updates are unavailable.
    }
  }, [phase, looping, orbState, freshFrames, manualReduced, theme]);

  const selectPhase = (next: Phase) => {
    setLooping(false);
    setPhase(next);
  };
  const motionAllowed = visible && !reducedMotion;
  const beamActive = motionAllowed && (phase === "processing" || freshFrames);

  return (
    <main className={`workbench workbench--${theme}${reducedMotion ? " motion-reduced" : ""}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW / HUD MOTION REVIEW</p>
            <h1>同一尺寸，平滑切换状态</h1>
          </div>
          <p className="prototype-note">只读交互原型 · 模拟状态与音频帧，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="原型控制">
          <div className="phase-controls" role="group" aria-label="手动选择HUD状态">
            <button type="button" aria-pressed={phase === "recording"} onClick={() => selectPhase("recording")}>录音中</button>
            <button type="button" aria-pressed={phase === "processing"} onClick={() => selectPhase("processing")}>处理中</button>
          </div>
          <button
            className="loop-button"
            type="button"
            aria-pressed={looping}
            onClick={() => setLooping((current) => !current)}
          >
            {looping ? "暂停循环" : "循环演示"}
          </button>
          <label className="orb-picker">
            <span>处理中 Orb</span>
            <select value={orbState} onChange={(event) => setOrbState(event.target.value as OrbState)}>
              {orbOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </label>
          <label className="check-control">
            <input type="checkbox" checked={freshFrames} onChange={(event) => setFreshFrames(event.target.checked)} />
            <span>模拟新音频帧</span>
          </label>
          <label className="check-control">
            <input type="checkbox" checked={manualReduced} onChange={(event) => setManualReduced(event.target.checked)} />
            <span>减少动态效果</span>
          </label>
          <div className="theme-control" role="group" aria-label="页面背景">
            <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button>
            <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button>
          </div>
        </section>

        <section className="preview-section" aria-labelledby="preview-title">
          <div className="preview-heading">
            <div>
              <h2 id="preview-title">单个 1× 实际尺寸预览</h2>
              <p>录音与处理中均为 220 × 48 · R24；原生 Mono Beam 持续使用同一胶囊。</p>
            </div>
            <span className="size-badge">220 × 48 · R24</span>
          </div>

          <section className={`live-stage${visible ? " live-stage--visible" : ""}`} ref={stageRef} aria-label="动态胶囊预览">
            <HudCapsule
              phase={phase}
              orbState={orbState}
              beamActive={beamActive}
              motionAllowed={motionAllowed}
              freshFrames={freshFrames}
            />
            <div className="stage-caption">
              <span className="phase-indicator" />
              <span>{phase === "recording" ? "录音中" : "处理中"}{looping ? " · 循环演示" : " · 手动预览"}</span>
            </div>
          </section>

          <div className="simulation-note">
            <span>循环每相 3.6 秒；这是原型演示节奏，不代表实际处理耗时。</span>
            <span>AirPods Pro 与 00:08 为固定示例；处理中不显示设备或计时。</span>
          </div>
        </section>

        <footer className="page-footer">
          <span>离开页面或滚动离开预览时，循环与动效会暂停；返回后重新计时。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
