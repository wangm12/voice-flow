import { useEffect, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb, type OrbState } from "thinking-orbs";

type Theme = "light" | "dark";
type HudKind = "recording" | "processing";
type Geometry = { width: number; height: number; radius: number; compact: boolean };

const processingOrbOptions: Array<{ value: OrbState; label: string }> = [
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

function getGeometry(kind: HudKind, compact: boolean): Geometry {
  if (!compact) return { width: 264, height: 56, radius: 16, compact: false };
  return kind === "recording"
    ? { width: 220, height: 48, radius: 24, compact: true }
    : { width: 152, height: 40, radius: 20, compact: true };
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
  geometry,
  active,
  orbState,
  recordingOrb,
}: {
  kind: HudKind;
  geometry: Geometry;
  active: boolean;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  const { width, height, radius, compact } = geometry;
  return (
    <BorderBeam
      className="beam-wrap"
      style={{ width, height }}
      size="line"
      colorVariant="mono"
      theme="dark"
      staticColors
      strength={1}
      brightness={1.3}
      duration={3.1}
      borderRadius={radius}
      active={active}
    >
      <div
        className={`hud-shell hud-shell--${kind}${compact ? " hud-shell--compact" : ""}`}
        style={{ width, height, borderRadius: radius }}
      >
        {kind === "recording" ? (
          <div className={`recording-grid${compact ? " recording-grid--compact" : ""}`}>
            <span className="recording-icon" aria-hidden="true">
              {recordingOrb ? <OrbSlot state="breathing" active={active} /> : <MicIcon />}
            </span>
            <span className="recording-label">录音中</span>
            <time className="recording-timer">00:08</time>
            <span className="recording-device">AirPods Pro</span>
          </div>
        ) : (
          <div className={`processing-group${compact ? " processing-group--compact" : ""}`}>
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
  compact,
  scale = 1,
  active,
  orbState,
  recordingOrb,
}: {
  kind: HudKind;
  compact: boolean;
  scale?: number;
  active: boolean;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  const geometry = getGeometry(kind, compact);
  return (
    <div className="hud-preview" style={{ width: geometry.width * scale, height: geometry.height * scale }}>
      <div
        className="hud-preview__scale"
        style={{ width: geometry.width, height: geometry.height, transform: `scale(${scale})` }}
      >
        <HudCapsule
          kind={kind}
          geometry={geometry}
          active={active}
          orbState={orbState}
          recordingOrb={recordingOrb}
        />
      </div>
    </div>
  );
}

function ComparisonCell({ kind, compact, active, orbState, recordingOrb }: {
  kind: HudKind;
  compact: boolean;
  active: boolean;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  const geometry = getGeometry(kind, compact);
  return (
    <div className={`comparison-cell${compact ? " comparison-cell--proposal" : ""}`}>
      <span className="cell-dimensions">{geometry.width} × {geometry.height} · R{geometry.radius}</span>
      <HudPreview
        kind={kind}
        compact={compact}
        active={active}
        orbState={orbState}
        recordingOrb={recordingOrb}
      />
    </div>
  );
}

function ComparisonRow({ kind, active, orbState, recordingOrb }: {
  kind: HudKind;
  active: boolean;
  orbState: OrbState;
  recordingOrb: boolean;
}) {
  const label = kind === "recording" ? "录音中" : "处理中";
  const detail = kind === "recording" ? "设备与计时" : "Orb + Beam";
  return (
    <div className={`comparison-row comparison-row--${kind}`}>
      <div className="row-label">
        <strong>{label}</strong>
        <span>{detail}</span>
      </div>
      <ComparisonCell kind={kind} compact={false} active={active} orbState={orbState} recordingOrb={recordingOrb} />
      <ComparisonCell kind={kind} compact active={active} orbState={orbState} recordingOrb={recordingOrb} />
    </div>
  );
}

function DetailPair({ orbState, recordingOrb, recordingActive, processingActive }: {
  orbState: OrbState;
  recordingOrb: boolean;
  recordingActive: boolean;
  processingActive: boolean;
}) {
  return (
    <section className="detail-section" aria-label="建议尺寸的两倍细节">
      <div className="detail-heading">
        <span>紧凑提案 · 2× 局部细节</span>
        <span>下方放大只用于观察边界，不代表实际尺寸</span>
      </div>
      <div className="detail-grid">
        <div className="detail-item">
          <span className="detail-label">录音中 · 220 × 48</span>
          <HudPreview kind="recording" compact scale={2} active={recordingActive} orbState="breathing" recordingOrb={recordingOrb} />
        </div>
        <div className="detail-item">
          <span className="detail-label">处理中 · 152 × 40</span>
          <HudPreview kind="processing" compact scale={2} active={processingActive} orbState={orbState} recordingOrb={recordingOrb} />
        </div>
      </div>
    </section>
  );
}

function App() {
  const [theme, setTheme] = useState<Theme>(initialQuery.get("theme") === "light" ? "light" : "dark");
  const [orbState, setOrbState] = useState<OrbState>(initialOrbState);
  const [recordingOrb, setRecordingOrb] = useState(initialQuery.get("recordingOrb") !== "off");
  const [freshFrames, setFreshFrames] = useState(initialQuery.get("frames") !== "stale");
  const [reducedMotion, setReducedMotion] = useState(initialQuery.get("motion") === "reduced");
  const [showDetails, setShowDetails] = useState(initialQuery.get("detail") === "2");

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("theme", theme);
    params.set("orbState", orbState);
    params.set("recordingOrb", recordingOrb ? "on" : "off");
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("motion", reducedMotion ? "reduced" : "full");
    params.set("detail", showDetails ? "2" : "off");
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // Local controls remain usable if URL updates are restricted.
    }
  }, [theme, orbState, recordingOrb, freshFrames, reducedMotion, showDetails]);

  const recordingActive = !reducedMotion && freshFrames;
  const processingActive = !reducedMotion;

  return (
    <main className={`workbench workbench--${theme}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW / PILL SIZE REVIEW</p>
            <h1>录音胶囊尺寸对照</h1>
          </div>
          <p className="prototype-note">只读交互原型 · 模拟状态与音频帧，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="原型控制">
          <div className="control-group">
            <span className="control-label">背景</span>
            <div className="segmented-control" role="group" aria-label="背景明暗">
              <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button>
              <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button>
            </div>
          </div>

          <label className="orb-picker">
            <span>处理动效</span>
            <select value={orbState} onChange={(event) => setOrbState(event.target.value as OrbState)}>
              {processingOrbOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
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
          <label className="check-control">
            <input type="checkbox" checked={showDetails} onChange={(event) => setShowDetails(event.target.checked)} />
            <span>显示 2× 细节</span>
          </label>
        </section>

        <section className="comparison-section" aria-labelledby="comparison-title">
          <div className="section-heading">
            <div>
              <h2 id="comparison-title">同一动效，比较实际尺寸</h2>
              <p>两列均为 1×；推荐胶囊采用更窄、更高圆角的 pill 比例。</p>
            </div>
            <span className="recommendation-tag">紧凑提案</span>
          </div>
          <div className="comparison-scroll">
            <div className="comparison-grid">
              <div className="comparison-head comparison-head--blank" aria-hidden="true" />
              <div className="comparison-head">
                <strong>现有尺寸参考</strong>
                <span>264 × 56 · R16</span>
              </div>
              <div className="comparison-head comparison-head--proposal">
                <strong>VoiceFlow 紧凑提案</strong>
                <span>参考 pill 形态；非 Typeless 尺寸测量</span>
              </div>
              <ComparisonRow kind="recording" active={recordingActive} orbState={orbState} recordingOrb={recordingOrb} />
              <ComparisonRow kind="processing" active={processingActive} orbState={orbState} recordingOrb={recordingOrb} />
            </div>
          </div>
          {showDetails && (
            <DetailPair
              orbState={orbState}
              recordingOrb={recordingOrb}
              recordingActive={recordingActive}
              processingActive={processingActive}
            />
          )}
        </section>

        <footer className="page-footer">
          <span>AirPods Pro 与 00:08 为固定示例；新尺寸仅是 VoiceFlow 提案。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
