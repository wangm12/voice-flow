import { useEffect, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb } from "thinking-orbs";

type HudState = "preparing" | "recording" | "transcribing" | "cleanup" | "progress" | "no-audio" | "unverified" | "entered";
type Theme = "light" | "dark";

const stateOptions: Array<{ id: HudState; label: string }> = [
  { id: "preparing", label: "准备麦克风" },
  { id: "recording", label: "录音中" },
  { id: "transcribing", label: "正在识别" },
  { id: "cleanup", label: "正在整理" },
  { id: "progress", label: "处理中 · 阶段未知" },
  { id: "no-audio", label: "未收到音频" },
  { id: "unverified", label: "输入未确认" },
  { id: "entered", label: "已输入并确认" },
];

const initialQuery = new URLSearchParams(window.location.search);
const knownStates = new Set(stateOptions.map((item) => item.id));

function initialState(): HudState {
  const requested = initialQuery.get("state") as HudState | null;
  return requested && knownStates.has(requested) ? requested : "recording";
}

function MicIcon() {
  return (
    <svg className="mic-icon" viewBox="0 0 20 20" aria-hidden="true">
      <rect x="7" y="2.5" width="6" height="10" rx="3" />
      <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
    </svg>
  );
}

function HudCapsule({
  state,
  active = false,
  orbAnimation = true,
}: {
  state: HudState;
  active?: boolean;
  orbAnimation?: boolean;
}) {
  const isRecording = state === "recording";
  const isProcessing = state === "transcribing" || state === "cleanup" || state === "progress";
  const isWarning = state === "no-audio" || state === "unverified";
  const height = isWarning ? 72 : 56;
  const beamSize = isProcessing ? "line" : "md";
  const label = state === "preparing" ? "正在准备麦克风"
    : state === "recording" ? "录音中"
      : state === "transcribing" ? "正在识别"
        : state === "cleanup" ? "正在整理"
          : state === "no-audio" ? "未收到音频"
            : state === "unverified" ? "已尝试输入"
              : "";
  const secondary = state === "no-audio" ? "没有生成可用文字"
    : state === "unverified" ? "检查目标文本"
      : "";

  return (
    <BorderBeam
      className="beam-wrap"
      style={{ width: 264, height }}
      size={beamSize}
      colorVariant="mono"
      theme="dark"
      staticColors
      strength={1}
      brightness={1.3}
      duration={isProcessing ? 3.1 : 1.96}
      borderRadius={16}
      active={active}
    >
      <div className={`hud-shell${isWarning ? " hud-shell--warning" : ""}`} style={{ width: 264, height }}>
        {isRecording ? (
          <>
            <div className="hud-recording-row">
              <div className="hud-label-group">
                {orbAnimation ? (
                  <span className="orb-slot" aria-hidden="true">
                    <ThinkingOrb state="breathing" size={64} theme="dark" speed={1.35} paused={!active} aria-hidden="true" />
                  </span>
                ) : <MicIcon />}
                <span className="hud-label">录音中</span>
              </div>
              <time className="hud-timer">00:08</time>
            </div>
            <div className="hud-device">AirPods Pro</div>
          </>
        ) : isProcessing ? (
          <div className="hud-centered hud-centered--processing">
            {label && <span className="hud-label">{label}</span>}
          </div>
        ) : isWarning ? (
          <div className="hud-warning-copy">
            <span className="hud-label">{label}</span>
            <span className="hud-secondary">{secondary}</span>
          </div>
        ) : (
          <div className="hud-centered"><span className="hud-label">{label}</span></div>
        )}
      </div>
    </BorderBeam>
  );
}

function HudPreview({
  state,
  scale = 1,
  active = false,
  orbAnimation = true,
}: {
  state: HudState;
  scale?: number;
  active?: boolean;
  orbAnimation?: boolean;
}) {
  if (state === "entered") return null;
  const height = state === "no-audio" || state === "unverified" ? 72 : 56;

  return (
    <div className="hud-preview" style={{ width: 264 * scale, height: height * scale }}>
      <div className="hud-preview__scale" style={{ width: 264, height, transform: `scale(${scale})` }}>
        <HudCapsule state={state} active={active} orbAnimation={orbAnimation} />
      </div>
    </div>
  );
}

function StateSpecimen({ label, state, orbAnimation = true }: { label: string; state: HudState; orbAnimation?: boolean }) {
  return (
    <div className="specimen">
      <div className="specimen-label">{label}</div>
      <HudPreview state={state} orbAnimation={orbAnimation} />
    </div>
  );
}

function App() {
  const [state, setState] = useState<HudState>(initialState);
  const [theme, setTheme] = useState<Theme>(initialQuery.get("theme") === "light" ? "light" : "dark");
  const [scale, setScale] = useState(initialQuery.get("scale") === "1" ? 1 : 2);
  const [reducedMotion, setReducedMotion] = useState(initialQuery.get("motion") === "reduced");
  const [freshFrames, setFreshFrames] = useState(initialQuery.get("frames") !== "stale");
  const [orbAnimation, setOrbAnimation] = useState(initialQuery.get("orb") !== "off");

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("state", state);
    params.set("theme", theme);
    params.set("scale", String(scale));
    params.set("motion", reducedMotion ? "reduced" : "full");
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("orb", orbAnimation ? "on" : "off");
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // Controls still work when this file is opened without a writable URL.
    }
  }, [state, theme, scale, reducedMotion, freshFrames, orbAnimation]);

  const mainActive = !reducedMotion && (
    (state === "recording" && freshFrames)
    || state === "transcribing"
    || state === "cleanup"
    || state === "progress"
  );
  const selectedLabel = stateOptions.find((item) => item.id === state)?.label ?? "录音中";

  return (
    <main className={`workbench workbench--${theme}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW / HUD REVIEW</p>
            <h1>录音状态 HUD</h1>
          </div>
          <p className="prototype-note">交互原型 · 模拟状态与音频帧，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="原型控制">
          <label className="state-picker">
            <span>模拟状态</span>
            <select value={state} onChange={(event) => setState(event.target.value as HudState)}>
              {stateOptions.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
            </select>
          </label>

          <div className="control-group">
            <span className="control-label">背景</span>
            <div className="segmented-control" role="group" aria-label="背景明暗">
              <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button>
              <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button>
            </div>
          </div>

          <div className="control-group">
            <span className="control-label">放大</span>
            <div className="segmented-control" role="group" aria-label="预览倍率">
              <button type="button" aria-pressed={scale === 1} onClick={() => setScale(1)}>1×</button>
              <button type="button" aria-pressed={scale === 2} onClick={() => setScale(2)}>2×</button>
            </div>
          </div>

          <label className="check-control">
            <input type="checkbox" checked={reducedMotion} onChange={(event) => setReducedMotion(event.target.checked)} />
            <span>减少动态效果</span>
          </label>

          {state === "recording" && (
            <>
              <label className="check-control">
                <input type="checkbox" checked={orbAnimation} onChange={(event) => setOrbAnimation(event.target.checked)} />
                <span>录音点阵动画</span>
              </label>
              <label className="check-control">
                <input type="checkbox" checked={freshFrames} onChange={(event) => setFreshFrames(event.target.checked)} />
                <span>模拟新音频帧</span>
              </label>
            </>
          )}
        </section>

        {state === "entered" && (
          <p className="hidden-state-note">已输入并确认 · HUD 已隐藏</p>
        )}

        <section className="main-preview-section" aria-labelledby="preview-title">
          <div className="section-heading">
            <div>
              <h2 id="preview-title">Border Beam · Mono</h2>
              <p>{selectedLabel} · 保留完整 Beam 容器；录音点阵可单独开关</p>
            </div>
            {scale === 2 && <span className="scale-hint">放大区域可横向查看</span>}
          </div>

          {state === "entered" ? (
            <div className="hidden-preview-space">确认输入后胶囊不显示；此说明位于组件外。</div>
          ) : (
            <div className="hero-stage">
              <div className="hero-loupe">
                <div className="sample-label"><span>{scale === 2 ? "2× 细节查看" : "1× 动态预览"}</span><span>放大呈现 · 非产品尺寸</span></div>
                <div className="loupe-scroll">
                  <HudPreview state={state} scale={scale} active={mainActive} orbAnimation={orbAnimation} />
                </div>
              </div>
              <aside className="actual-size">
                <div className="sample-label"><span>1× 实际尺寸</span><span>静止参照</span></div>
                <HudPreview state={state} orbAnimation={orbAnimation} />
                <p>264 × {state === "no-audio" || state === "unverified" ? "72" : "56"} px</p>
              </aside>
            </div>
          )}
        </section>

        <section className="specimen-section" aria-labelledby="specimen-title">
          <div className="section-heading section-heading--specimens">
            <div>
              <h2 id="specimen-title">状态样本</h2>
              <p>同一容器、同一边界；样本静止，便于比较状态文案与内容位置。</p>
            </div>
          </div>
          <div className="specimen-grid">
              <StateSpecimen label={`录音中 · ${orbAnimation ? "点阵动画" : "静态麦克风"} · 模拟设备与时长`} state="recording" orbAnimation={orbAnimation} />
            <StateSpecimen label="语音识别 · 底边光束" state="transcribing" />
            <StateSpecimen label="文本整理 · 底边光束" state="cleanup" />
            <StateSpecimen label="输入未确认 · 静态提示" state="unverified" />
          </div>
        </section>

        <footer className="page-footer">
          <span>AirPods Pro 与 00:08 为固定模拟示例；关闭新音频帧时录音动效暂停。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
