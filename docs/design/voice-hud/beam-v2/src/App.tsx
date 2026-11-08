import { useEffect, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb, type OrbState } from "thinking-orbs";

type Theme = "light" | "dark";
type BeamPreset = "line" | "md";
type HudKind = "recording" | "processing";

const orbOptions: Array<{ value: OrbState; label: string }> = [
  { value: "working", label: "working · 轨道" },
  { value: "solving", label: "solving · 重组" },
  { value: "shaping", label: "shaping · 塑形" },
];

const allowedOrbStates = new Set<OrbState>(["working", "solving", "shaping"]);
const initialQuery = new URLSearchParams(window.location.search);

function initialOrbState(): OrbState {
  const requested = initialQuery.get("orbState") as OrbState | null;
  return requested && allowedOrbStates.has(requested) ? requested : "working";
}

function MicIcon() {
  return (
    <svg className="mic-icon" viewBox="0 0 20 20" aria-hidden="true">
      <rect x="7" y="2.5" width="6" height="10" rx="3" />
      <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
    </svg>
  );
}

function OrbSlot({ state, active }: { state: OrbState; active: boolean }) {
  return (
    <span className="orb-slot" aria-hidden="true">
      <ThinkingOrb state={state} size={20} theme="dark" paused={!active} aria-hidden="true" />
    </span>
  );
}

function HudCapsule({
  kind,
  beamPreset,
  active,
  orbState,
  recordingOrb,
}: {
  kind: HudKind;
  beamPreset: BeamPreset;
  active: boolean;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  return (
    <BorderBeam
      className="beam-wrap"
      style={{ width: 264, height: 56 }}
      size={beamPreset}
      colorVariant="mono"
      theme="dark"
      staticColors
      strength={1}
      brightness={1.3}
      duration={beamPreset === "line" ? 3.1 : 1.96}
      borderRadius={16}
      active={active}
    >
      <div className={`hud-shell hud-shell--${kind}`}>
        {kind === "recording" ? (
          <div className="recording-grid">
            <span className="recording-icon" aria-hidden="true">
              {recordingOrb ? <OrbSlot state="breathing" active={active} /> : <MicIcon />}
            </span>
            <span className="recording-label">录音中</span>
            <time className="recording-timer">00:08</time>
            <span className="recording-device">AirPods Pro</span>
          </div>
        ) : (
          <div className="processing-group">
            <OrbSlot state={orbState} active={active} />
            <span className="processing-label">处理中</span>
          </div>
        )}
      </div>
    </BorderBeam>
  );
}

function HudPreview({
  kind,
  scale,
  active,
  beamPreset,
  orbState,
  recordingOrb,
}: {
  kind: HudKind;
  scale: number;
  active: boolean;
  beamPreset: BeamPreset;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  return (
    <div className="hud-preview" style={{ width: 264 * scale, height: 56 * scale }}>
      <div className="hud-preview__scale" style={{ width: 264, height: 56, transform: `scale(${scale})` }}>
        <HudCapsule
          kind={kind}
          beamPreset={beamPreset}
          active={active}
          orbState={orbState}
          recordingOrb={recordingOrb}
        />
      </div>
    </div>
  );
}

function PreviewPair({
  kind,
  active,
  beamPreset,
  orbState,
  recordingOrb,
}: {
  kind: HudKind;
  active: boolean;
  beamPreset: BeamPreset;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  const label = kind === "recording" ? "录音中" : "处理中";
  const detail = kind === "recording"
    ? `${recordingOrb ? "原录音点阵" : "静态麦克风"} · 模拟设备与时长`
    : "处理动效可切换 · 不显示阶段或设备信息";

  return (
    <section className={`preview-section preview-section--${kind}`} aria-labelledby={`${kind}-title`}>
      <header className="preview-section__header">
        <div>
          <p className="section-index">{kind === "recording" ? "01" : "02"} / LIVE PREVIEW</p>
          <h2 id={`${kind}-title`}>{label}</h2>
          <p className="section-description">{detail}</p>
        </div>
      </header>
      <div className="preview-pair">
        <div className="preview-detail">
          <div className="sample-label"><span>2× 动态细节</span><span>放大呈现</span></div>
          <div className="loupe-scroll">
            <HudPreview
              kind={kind}
              scale={2}
              active={active}
              beamPreset={beamPreset}
              orbState={orbState}
              recordingOrb={recordingOrb}
            />
          </div>
        </div>
        <aside className="preview-actual">
          <div className="sample-label"><span>1× 实际尺寸</span><span>静止参照</span></div>
          <HudPreview
            kind={kind}
            scale={1}
            active={false}
            beamPreset={beamPreset}
            orbState={orbState}
            recordingOrb={recordingOrb}
          />
          <p>264 × 56 px</p>
        </aside>
      </div>
    </section>
  );
}

function App() {
  const [theme, setTheme] = useState<Theme>(initialQuery.get("theme") === "light" ? "light" : "dark");
  const [beamPreset, setBeamPreset] = useState<BeamPreset>(initialQuery.get("beam") === "md" ? "md" : "line");
  const [orbState, setOrbState] = useState<OrbState>(initialOrbState);
  const [reducedMotion, setReducedMotion] = useState(initialQuery.get("motion") === "reduced");
  const [freshFrames, setFreshFrames] = useState(initialQuery.get("frames") !== "stale");
  const [recordingOrb, setRecordingOrb] = useState(initialQuery.get("recordingOrb") !== "off");

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("theme", theme);
    params.set("beam", beamPreset);
    params.set("orbState", orbState);
    params.set("motion", reducedMotion ? "reduced" : "full");
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("recordingOrb", recordingOrb ? "on" : "off");
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // The controls still work if the preview URL is read-only.
    }
  }, [theme, beamPreset, orbState, reducedMotion, freshFrames, recordingOrb]);

  const recordingActive = !reducedMotion && freshFrames;
  const processingActive = !reducedMotion;

  return (
    <main className={`workbench workbench--${theme}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW / HUD REVIEW</p>
            <h1>录音与处理动效</h1>
          </div>
          <p className="prototype-note">交互原型 · 模拟状态与音频帧，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="原型控制">
          <div className="control-group">
            <span className="control-label">背景</span>
            <div className="segmented-control" role="group" aria-label="背景明暗">
              <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button>
              <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button>
            </div>
          </div>

          <div className="control-group">
            <span className="control-label">边框动效</span>
            <div className="segmented-control" role="group" aria-label="Beam边框模式">
              <button type="button" aria-pressed={beamPreset === "line"} onClick={() => setBeamPreset("line")}>底部光束</button>
              <button type="button" aria-pressed={beamPreset === "md"} onClick={() => setBeamPreset("md")}>环绕边框</button>
            </div>
          </div>

          <label className="orb-picker">
            <span>处理动效</span>
            <select value={orbState} onChange={(event) => setOrbState(event.target.value as OrbState)}>
              {orbOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </label>

          <label className="check-control">
            <input type="checkbox" checked={recordingOrb} onChange={(event) => setRecordingOrb(event.target.checked)} />
            <span>录音点阵动画</span>
          </label>

          <label className="check-control">
            <input type="checkbox" checked={freshFrames} onChange={(event) => setFreshFrames(event.target.checked)} />
            <span>模拟新音频帧</span>
          </label>

          <label className="check-control">
            <input type="checkbox" checked={reducedMotion} onChange={(event) => setReducedMotion(event.target.checked)} />
            <span>减少动态效果</span>
          </label>
        </section>

        <div className="preview-stack">
          <PreviewPair
            kind="recording"
            active={recordingActive}
            beamPreset={beamPreset}
            orbState="breathing"
            recordingOrb={recordingOrb}
          />
          <PreviewPair
            kind="processing"
            active={processingActive}
            beamPreset={beamPreset}
            orbState={orbState}
            recordingOrb={recordingOrb}
          />
        </div>

        <footer className="page-footer">
          <span>AirPods Pro 与 00:08 为固定模拟示例；无新音频帧时仅录音动效暂停。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
