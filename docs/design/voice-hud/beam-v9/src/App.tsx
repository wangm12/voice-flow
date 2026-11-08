import { useEffect, useRef, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb, type OrbState } from "thinking-orbs";
import notesIconUrl from "../assets/notes.png?inline";
import safariIconUrl from "../assets/safari.png?inline";

type Phase = "recording" | "processing";
type Theme = "dark" | "light";
type TargetApp = "notes" | "safari" | "unknown";
type BeamMode = "rotate" | "line";

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

function MicIcon() {
  return (
    <svg className="mic-icon" viewBox="0 0 20 20" aria-hidden="true">
      <rect x="7" y="2.5" width="6" height="10" rx="3" />
      <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
    </svg>
  );
}

function ActionIcon() {
  return (
    <svg className="hud-action-icon" viewBox="0 0 20 20" aria-hidden="true">
      <rect x="5" y="5" width="10" height="10" rx="1" />
    </svg>
  );
}

function BeamLayer({
  mode,
  active,
  opacity,
}: {
  mode: BeamMode;
  active: boolean;
  opacity: number;
}) {
  return (
    <BorderBeam
      className={`beam-layer beam-layer--${mode}`}
      style={{
        position: "absolute",
        inset: 0,
        zIndex: 2,
        width: 220,
        height: 48,
        opacity,
        pointerEvents: "none",
        transition: "opacity 180ms cubic-bezier(.22, 1, .36, 1)",
      }}
      size={mode === "rotate" ? "md" : "line"}
      colorVariant="mono"
      theme="dark"
      staticColors
      strength={mode === "rotate" ? 1 : 0.65}
      brightness={1.3}
      duration={mode === "rotate" ? 1.96 : 3.1}
      borderRadius={24}
      active={active}
      aria-hidden="true"
    >
      <span className="beam-layer__inner" aria-hidden="true" />
    </BorderBeam>
  );
}

function HudCapsule({
  phase,
  orbState,
  targetApp,
  showTargetIcon,
  freshFrames,
  motionAllowed,
  rotateActive,
  lineActive,
  rotateOpacity,
  lineOpacity,
  onAction,
}: {
  phase: Phase;
  orbState: OrbState;
  targetApp: TargetApp;
  showTargetIcon: boolean;
  freshFrames: boolean;
  motionAllowed: boolean;
  rotateActive: boolean;
  lineActive: boolean;
  rotateOpacity: number;
  lineOpacity: number;
  onAction: () => void;
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

  const appLabel = targetApp === "notes" ? "备忘录" : targetApp === "safari" ? "Safari" : "未知应用";
  const orbActive = motionAllowed && (processing || (!showTargetIcon && freshFrames));
  return (
    <div className={`hud-capsule${motionAllowed ? "" : " hud-capsule--paused"}`}>
      <div className="hud-shell" role="status" aria-live="polite" aria-atomic="true">
        <span className="sr-only">
          {processing ? "处理中" : `录音中，模拟输入目标：${appLabel}，AirPods Pro，00:08`}
        </span>

        <span className={`icon-slot${orbShifting ? " icon-slot--shifting" : ""}`} aria-hidden="true">
          <span className={`app-icon-layer${processing || !showTargetIcon ? " app-icon-layer--hidden" : ""}`}>
            {targetApp === "notes" ? <img src={notesIconUrl} alt="" /> : targetApp === "safari" ? <img src={safariIconUrl} alt="" /> : <MicIcon />}
          </span>
          <span className={`orb-layer${(!processing && showTargetIcon) ? " orb-layer--hidden" : ""}`}>
            <ThinkingOrb
              state={renderedOrbState}
              size={20}
              theme="dark"
              paused={!orbActive}
              aria-hidden="true"
            />
          </span>
        </span>

        <span className="copy-dock" aria-hidden="true">
          <span className={`copy-label${processing ? " copy-label--hidden" : ""}`}>录音中</span>
          <span className={`copy-label copy-label--processing${processing ? "" : " copy-label--hidden"}`}>处理中</span>
          <span className={`recording-device${processing ? " recording-device--hidden" : ""}`}>AirPods Pro</span>
        </span>
        <time className={`recording-timer${processing ? " recording-timer--hidden" : ""}`} aria-hidden="true">00:08</time>
      </div>

      <button
        className="hud-action-button"
        type="button"
        aria-label={processing ? "取消本次听写" : "停止录音并整理"}
        title={processing ? "取消本次听写" : "停止录音并整理"}
        onKeyDown={(event) => {
          if (event.repeat && (event.key === "Enter" || event.key === " " || event.key === "Spacebar")) {
            event.preventDefault();
          }
        }}
        onClick={(event) => {
          if (event.detail > 1) return;
          onAction();
        }}
      >
        <ActionIcon />
      </button>

      <BeamLayer mode="rotate" active={rotateActive} opacity={rotateOpacity} />
      <BeamLayer mode="line" active={lineActive} opacity={lineOpacity} />
    </div>
  );
}

function App() {
  const [phase, setPhase] = useState<Phase>(initialPhase);
  const [looping, setLooping] = useState(query.get("loop") === "on");
  const [orbState, setOrbState] = useState<OrbState>(initialOrbState);
  const [targetApp, setTargetApp] = useState<TargetApp>(
    query.get("target") === "safari" ? "safari" : query.get("target") === "unknown" ? "unknown" : "notes",
  );
  const [showTargetIcon, setShowTargetIcon] = useState(query.get("targetIcon") !== "off");
  const [freshFrames, setFreshFrames] = useState(query.get("frames") !== "stale");
  const [manualReduced, setManualReduced] = useState(query.get("motion") === "reduced");
  const [systemReduced, setSystemReduced] = useState(false);
  const [theme, setTheme] = useState<Theme>(query.get("theme") === "light" ? "light" : "dark");
  const [hudShown, setHudShown] = useState(true);
  const [visible, setVisible] = useState(false);
  const [beamTransitioning, setBeamTransitioning] = useState(false);
  const [beamFromPhase, setBeamFromPhase] = useState<Phase>(initialPhase());
  const stageRef = useRef<HTMLElement>(null);
  const beamTimerRef = useRef<number | undefined>(undefined);
  const loopTimerRef = useRef<number | undefined>(undefined);
  const reducedMotion = manualReduced || systemReduced;
  const motionAllowed = hudShown && visible && !reducedMotion;

  const clearBeamTimer = () => {
    if (beamTimerRef.current !== undefined) window.clearTimeout(beamTimerRef.current);
    beamTimerRef.current = undefined;
  };
  const clearLoopTimer = () => {
    if (loopTimerRef.current !== undefined) window.clearTimeout(loopTimerRef.current);
    loopTimerRef.current = undefined;
  };

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
    if (!motionAllowed) {
      if (beamTimerRef.current !== undefined) window.clearTimeout(beamTimerRef.current);
      beamTimerRef.current = undefined;
      setBeamFromPhase(phase);
      setBeamTransitioning(false);
    }
  }, [motionAllowed, phase]);

  useEffect(() => () => {
    clearBeamTimer();
    clearLoopTimer();
  }, []);

  const transitionPhaseTo = (next: Phase) => {
    if (next === phase) return;
    clearBeamTimer();
    if (motionAllowed) {
      setBeamFromPhase(phase);
      setBeamTransitioning(true);
      beamTimerRef.current = window.setTimeout(() => {
        setBeamTransitioning(false);
        beamTimerRef.current = undefined;
      }, 180);
    } else {
      setBeamFromPhase(next);
      setBeamTransitioning(false);
    }
    setPhase(next);
  };

  useEffect(() => {
    if (!looping || !motionAllowed) return;
    loopTimerRef.current = window.setTimeout(() => {
      loopTimerRef.current = undefined;
      transitionPhaseTo(phase === "recording" ? "processing" : "recording");
    }, 3600);
    return () => {
      if (loopTimerRef.current !== undefined) window.clearTimeout(loopTimerRef.current);
      loopTimerRef.current = undefined;
    };
  }, [looping, phase, motionAllowed]);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("phase", phase);
    params.set("loop", looping ? "on" : "off");
    params.set("orbState", orbState);
    params.set("target", targetApp);
    params.set("targetIcon", showTargetIcon ? "on" : "off");
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("motion", manualReduced ? "reduced" : "full");
    params.set("theme", theme);
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // The prototype remains usable when URL updates are unavailable.
    }
  }, [phase, looping, orbState, targetApp, showTargetIcon, freshFrames, manualReduced, theme]);

  const selectPhase = (next: Phase) => {
    setHudShown(true);
    clearLoopTimer();
    setLooping(false);
    transitionPhaseTo(next);
  };
  const cancelCurrent = () => {
    if (!hudShown) return;
    clearLoopTimer();
    clearBeamTimer();
    setLooping(false);
    setBeamFromPhase(phase);
    setBeamTransitioning(false);
    setHudShown(false);
  };
  const handleHudAction = () => {
    if (phase === "recording") {
      clearLoopTimer();
      setLooping(false);
      transitionPhaseTo("processing");
    } else {
      cancelCurrent();
    }
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !hudShown) return;
      if (event.target instanceof HTMLSelectElement) return;
      event.preventDefault();
      cancelCurrent();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [phase, hudShown]);
  const rotateOpacity = phase === "recording" && freshFrames ? 1 : 0;
  const lineOpacity = phase === "processing" ? 1 : 0;
  const rotateActive = motionAllowed && freshFrames && (
    phase === "recording" || (beamTransitioning && beamFromPhase === "recording")
  );
  const lineActive = motionAllowed && (
    phase === "processing" || (beamTransitioning && beamFromPhase === "processing")
  );

  return (
    <main className={`workbench workbench--${theme}${reducedMotion ? " motion-reduced" : ""}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW / HUD MOTION REVIEW</p>
            <h1>左侧锚点固定，单行处理垂直居中</h1>
          </div>
          <p className="prototype-note">只读交互原型 · 模拟状态与目标应用，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="原型控制">
          <div className="phase-controls" role="group" aria-label="手动选择HUD状态">
            <button type="button" aria-pressed={phase === "recording"} onClick={() => selectPhase("recording")}>录音中</button>
            <button type="button" aria-pressed={phase === "processing"} onClick={() => selectPhase("processing")}>处理中</button>
          </div>
          <button className="loop-button" type="button" aria-pressed={looping} onClick={() => setLooping((current) => !current)}>
            {looping ? "暂停循环" : "循环演示"}
          </button>
          <label className="target-picker">
            <span>模拟目标应用</span>
            <select value={targetApp} onChange={(event) => setTargetApp(event.target.value as TargetApp)}>
              <option value="notes">备忘录</option>
              <option value="safari">Safari</option>
              <option value="unknown">未知应用</option>
            </select>
          </label>
          <label className="orb-picker">
            <span>处理中 Orb</span>
            <select value={orbState} onChange={(event) => setOrbState(event.target.value as OrbState)}>
              {orbOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </label>
          <label className="check-control">
            <input type="checkbox" checked={showTargetIcon} onChange={(event) => setShowTargetIcon(event.target.checked)} />
            <span>录音显示目标图标</span>
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
              <p>两态共用 220 × 48 · R24 胶囊；icon/Orb 槽、文案和第二行锚点固定。</p>
            </div>
            <span className="size-badge">220 × 48 · R24</span>
          </div>

          <section className={`live-stage${visible ? " live-stage--visible" : ""}`} ref={stageRef} aria-label="动态胶囊预览">
            {hudShown ? (
              <>
                <HudCapsule
                  phase={phase}
                  orbState={orbState}
                  targetApp={targetApp}
                  showTargetIcon={showTargetIcon}
                  freshFrames={freshFrames}
                  motionAllowed={motionAllowed}
                  rotateActive={rotateActive}
                  lineActive={lineActive}
                  rotateOpacity={rotateOpacity}
                  lineOpacity={lineOpacity}
                  onAction={handleHudAction}
                />
                <div className="stage-caption">
                  <span className="phase-indicator" />
                  <span>{phase === "recording" ? "录音中 · 环绕光" : "处理中 · 底部扫光"}{looping ? " · 循环演示" : " · 手动预览"}</span>
                </div>
              </>
            ) : (
              <button className="restart-button" type="button" onClick={() => selectPhase("recording")}>
                重新开始预览
              </button>
            )}
          </section>

          <div className="simulation-note">
            <span>环绕/扫光表示正在工作，不代表完成百分比；每相 3.6 秒为模拟演示节奏。</span>
            <span>目标应用只为模拟，不检测当前前台；AirPods Pro 与 00:08 为固定示例。</span>
          </div>
        </section>

        <footer className="page-footer">
          <span>页面隐藏或滚出预览时暂停循环与动效；返回后从完整周期重新计时。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
