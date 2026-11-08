import { type ReactNode, useEffect, useMemo, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb } from "thinking-orbs";

type HudState = "preparing" | "recording" | "transcribing" | "cleanup" | "progress" | "no-audio" | "unverified" | "entered";
type Scheme = "A" | "B" | "C";
type View = "compare" | "states";

const stateOptions: Array<{ id: HudState; label: string }> = [
  { id: "preparing", label: "准备麦克风" },
  { id: "recording", label: "录音中" },
  { id: "transcribing", label: "正在识别" },
  { id: "cleanup", label: "正在整理" },
  { id: "no-audio", label: "未收到音频" },
  { id: "unverified", label: "输入未确认" },
  { id: "entered", label: "已输入并确认" },
];

const schemes: Array<{ id: Scheme; name: string; descriptor: string; reason: string }> = [
  {
    id: "A",
    name: "内核优先",
    descriptor: "录音点阵保留在中心",
    reason: "文字与录音点阵分工清楚；动态只留给真实的 ThinkingOrb。",
  },
  {
    id: "B",
    name: "边缘优先",
    descriptor: "内容安静，边缘移动",
    reason: "录音时用一束弱灰光沿单条边界移动，内部保持静态。",
  },
  {
    id: "C",
    name: "极简状态条",
    descriptor: "压缩成单行反馈",
    reason: "占用更低，设备与录音时长仍能快速扫读。",
  },
];

const initialQuery = new URLSearchParams(window.location.search);
const knownStates = new Set(stateOptions.map((item) => item.id));

function initialState(): HudState {
  const queryState = initialQuery.get("state") as HudState | null;
  return queryState && knownStates.has(queryState) ? queryState : "recording";
}

function initialView(): View {
  return initialQuery.get("view") === "states" ? "states" : "compare";
}

function MicIcon({ tiny = false }: { tiny?: boolean }) {
  return (
    <svg className={tiny ? "mic-icon mic-icon--tiny" : "mic-icon"} viewBox="0 0 20 20" aria-hidden="true">
      <rect x="7" y="2.5" width="6" height="10" rx="3" />
      <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />
    </svg>
  );
}

function HudSurface({
  scheme,
  state,
  staticPreview = false,
  reducedMotion,
  freshFrames,
  scale = 1,
}: {
  scheme: Scheme;
  state: HudState;
  staticPreview?: boolean;
  reducedMotion: boolean;
  freshFrames: boolean;
  scale?: number;
}) {
  const isCompact = scheme === "C";
  const isRecording = state === "recording";
  const isProcess = state === "transcribing" || state === "cleanup" || state === "progress";
  const isTerminal = state === "no-audio" || state === "unverified";
  const motionAllowed = !staticPreview && !reducedMotion && (!isRecording || freshFrames);
  const visualState = state === "preparing" ? "正在准备麦克风"
    : state === "recording" ? "录音中"
      : state === "transcribing" ? "正在识别"
        : state === "cleanup" ? "正在整理"
        : state === "progress" ? ""
          : state === "no-audio" ? "未收到音频"
            : state === "unverified" ? "已尝试输入"
              : "";
  const showHud = state !== "entered";
  const compactTerminal = isCompact && isTerminal;
  const width = isCompact && !compactTerminal ? 208 : 264;
  const height = compactTerminal ? 72 : isCompact ? 40 : 56;
  const showOrb = scheme === "A" && isRecording;
  const showMic = isCompact && (isRecording || state === "preparing");
  const showDevice = isRecording;

  if (!showHud) return null;

  const content = (
    <div className={`hud-shell hud-shell--${scheme.toLowerCase()}${isProcess ? " hud-shell--process" : ""}${isTerminal ? " hud-shell--terminal" : ""}`}>
      <div className={`hud-main${state === "preparing" ? " hud-main--centered" : ""}`}>
        <div className="hud-content">
          {showOrb && (
            <span className="orb-slot" aria-hidden="true">
              <ThinkingOrb
                state="breathing"
                size={64}
                theme="dark"
                speed={1.35}
                paused={!motionAllowed}
                aria-hidden="true"
              />
            </span>
          )}
          {showMic && <MicIcon />}
          <span className={`hud-label${isCompact ? " hud-label--compact" : ""}`}>{visualState}</span>
          {isCompact && isRecording && <span className="hud-device--compact">AirPods Pro</span>}
        </div>
        {isRecording && <time className="hud-timer">00:08</time>}
      </div>
      {showDevice && !isCompact && <div className="hud-device">AirPods Pro</div>}
      {isTerminal && (
        <div className="hud-caption">
          {state === "no-audio" ? "检查麦克风后重试" : "检查目标文本"}
        </div>
      )}
      {isProcess && (
        <span className={`hud-progress${motionAllowed ? " hud-progress--moving" : ""}`} aria-hidden="true">
          <span />
        </span>
      )}
    </div>
  );

  const stageStyle = {
    width: `${width * scale}px`,
    height: `${height * scale}px`,
  };
  const innerStyle = {
    width: `${width}px`,
    height: `${height}px`,
    transform: `scale(${scale})`,
  };

  return (
    <div className={`hud-stage hud-stage--${scheme.toLowerCase()}`} style={stageStyle}>
      <div className="hud-scale" style={innerStyle}>
        {scheme === "B" ? (
          <BorderBeam
            className={`hud-beam-frame${motionAllowed && isRecording ? " hud-beam-frame--active" : ""}`}
            size="md"
            colorVariant="mono"
            theme="dark"
            staticColors
            strength={0.25}
            brightness={0.72}
            saturation={0}
            duration={4.2}
            borderRadius={20}
            active={motionAllowed && isRecording}
          >
            {content}
          </BorderBeam>
        ) : content}
      </div>
    </div>
  );
}

function StateExample({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="state-example">
      <span className="state-example__label">{label}</span>
      {children}
    </div>
  );
}

function StateGallery({ reducedMotion, freshFrames }: { reducedMotion: boolean; freshFrames: boolean }) {
  const [recoveryOpen, setRecoveryOpen] = useState(true);
  const [learningOpen, setLearningOpen] = useState(true);
  const [recoveryFeedback, setRecoveryFeedback] = useState("");
  const [learningFeedback, setLearningFeedback] = useState("");

  const reset = () => {
    setRecoveryOpen(true);
    setLearningOpen(true);
    setRecoveryFeedback("");
    setLearningFeedback("");
  };

  return (
    <>
      <div className="gallery-toolbar">
        <span>胶囊内容直接展示在对应状态名下；控件仅执行本地原型动作。</span>
        <button type="button" className="text-button" onClick={reset}>重置原型动作</button>
      </div>
      <div className="state-gallery" aria-label="A 方案状态预览">
        <StateExample label="麦克风准备">
          <HudSurface scheme="A" state="preparing" reducedMotion={reducedMotion} freshFrames={freshFrames} />
        </StateExample>
        <StateExample label="录音 · 模拟音频帧">
          <HudSurface scheme="A" state="recording" reducedMotion={reducedMotion} freshFrames={freshFrames} />
        </StateExample>
        <StateExample label="语音识别">
          <HudSurface scheme="A" state="transcribing" reducedMotion={reducedMotion} freshFrames={freshFrames} />
        </StateExample>
        <StateExample label="文本整理">
          <HudSurface scheme="A" state="cleanup" reducedMotion={reducedMotion} freshFrames={freshFrames} />
        </StateExample>
        <StateExample label="阶段未知 · 无状态文案">
          <HudSurface scheme="A" state="progress" reducedMotion={reducedMotion} freshFrames={freshFrames} />
        </StateExample>
        <StateExample label="错误">
          <div className="hud-shell hud-shell--warning">
            <div className="warning-title">语音未完成</div>
            <div className="warning-copy">本次没有生成可用文字</div>
          </div>
        </StateExample>
        <StateExample label="输入未确认">
          <div className="hud-shell hud-shell--warning hud-shell--unverified">
            <div className="warning-title">已尝试输入</div>
            <div className="warning-copy">检查目标文本</div>
          </div>
        </StateExample>
        <StateExample label="音频恢复">
          {recoveryOpen ? (
            <div className="recovery-shell">
              <div className="recovery-heading">
                <strong>音频已保留</strong>
                <button type="button" className="close-control" aria-label="关闭恢复提示" onClick={() => { setRecoveryOpen(false); setRecoveryFeedback("原型动作：关闭恢复提示"); }}>×</button>
              </div>
              <p>保存到历史记录</p>
              <button type="button" className="primary-control" onClick={() => setRecoveryFeedback("原型动作：保存并查看")}>保存并查看</button>
            </div>
          ) : <p className="dismissed-sample">恢复提示已关闭 · 原型动作</p>}
          {recoveryFeedback && <span className="action-feedback">{recoveryFeedback}</span>}
        </StateExample>
        <StateExample label="个人词典学习">
          {learningOpen ? (
            <div className="learning-shell">
              <span>个人词典已更新</span>
              <button type="button" className="undo-control" onClick={() => setLearningFeedback("原型动作：撤销学习")}>撤销</button>
              <button type="button" className="close-control" aria-label="关闭学习提示" onClick={() => { setLearningOpen(false); setLearningFeedback("原型动作：关闭学习提示"); }}>×</button>
            </div>
          ) : <p className="dismissed-sample">学习提示已关闭 · 原型动作</p>}
          {learningFeedback && <span className="action-feedback">{learningFeedback}</span>}
        </StateExample>
      </div>
      <p className="gallery-note">恢复和学习按钮只显示“原型动作”，不会写入历史记录、个人词典或用户数据。</p>
    </>
  );
}

function App() {
  const [state, setState] = useState<HudState>(initialState);
  const [view, setView] = useState<View>(initialView);
  const [scheme, setScheme] = useState<Scheme>((initialQuery.get("scheme") as Scheme) || "A");
  const [theme, setTheme] = useState<"light" | "dark">(initialQuery.get("theme") === "dark" ? "dark" : "light");
  const [reducedMotion, setReducedMotion] = useState(initialQuery.get("motion") === "reduced");
  const [freshFrames, setFreshFrames] = useState(initialQuery.get("frames") !== "stale");
  const [scale, setScale] = useState(initialQuery.get("scale") === "2" ? 2 : 1);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    params.set("view", view);
    params.set("state", state);
    params.set("scheme", scheme);
    params.set("theme", theme);
    params.set("motion", reducedMotion ? "reduced" : "full");
    params.set("frames", freshFrames ? "fresh" : "stale");
    params.set("scale", String(scale));
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    } catch {
      // A file:// preview can restrict history updates; controls still work locally.
    }
  }, [view, state, scheme, theme, reducedMotion, freshFrames, scale]);

  const selectedStateLabel = useMemo(
    () => stateOptions.find((item) => item.id === state)?.label ?? "录音中",
    [state],
  );

  return (
    <main className={`workbench workbench--${theme}`}>
      <div className="page-shell">
        <header className="page-header">
          <div>
            <p className="eyebrow">VOICEFLOW · HUD REVIEW</p>
            <h1>录音状态反馈</h1>
          </div>
          <p className="prototype-note">交互原型 · 模拟状态与音量，不连接麦克风或模型</p>
        </header>

        <section className="control-rail" aria-label="预览控制">
          <label className="state-picker">
            <span>模拟状态</span>
            <select value={state} onChange={(event) => setState(event.target.value as HudState)}>
              {stateOptions.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
            </select>
          </label>

          <div className="rail-divider" />

          <fieldset className="segmented-control">
            <legend>背景</legend>
            <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button>
            <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button>
          </fieldset>

          <fieldset className="segmented-control">
            <legend>预览倍率</legend>
            <button type="button" aria-pressed={scale === 1} onClick={() => setScale(1)}>1×</button>
            <button type="button" aria-pressed={scale === 2} onClick={() => setScale(2)}>2×</button>
          </fieldset>

          <label className="check-control">
            <input type="checkbox" checked={reducedMotion} onChange={(event) => setReducedMotion(event.target.checked)} />
            <span>减少动态效果</span>
          </label>

          <label className="check-control check-control--audio">
            <input type="checkbox" checked={freshFrames} onChange={(event) => setFreshFrames(event.target.checked)} disabled={state !== "recording"} />
            <span>模拟新音频帧</span>
          </label>
          <span className="control-footnote">录音示例时关闭此项，点阵与边缘动效会暂停</span>
        </section>

        {view === "compare" ? (
          <>
            <section className={`comparison${scale === 2 ? " comparison--zoomed" : ""}`} aria-labelledby="compare-title">
              <div className="section-heading">
                <div>
                  <h2 id="compare-title">三种录音 HUD</h2>
                  <p>当前统一状态：{selectedStateLabel}。每列上方为交互预览，下方为静止参考。</p>
                  {scale === 2 && <p className="zoom-note">2× 放大后可横向滚动比较其余方案。</p>}
                </div>
                <button type="button" className="text-button" onClick={() => { setView("states"); setScheme("A"); }}>
                  查看 A 的完整状态
                </button>
              </div>

              {state === "entered" && (
                <p className="hidden-state-note">已输入并确认，HUD 已隐藏。此状态保留在原型外部作为说明。</p>
              )}

              <div className="comparison-grid">
                {schemes.map((item) => (
                  <article className={`compare-column compare-column--${item.id.toLowerCase()}`} key={item.id}>
                    <div className="scheme-heading">
                      <span className="scheme-letter">{item.id}</span>
                      <div>
                        <h3>{item.name}{item.id === "A" && <span className="recommendation">建议方向</span>}</h3>
                        <p>{item.descriptor}</p>
                      </div>
                    </div>

                    <div className="preview-block">
                      <div className="preview-caption"><span>动态预览</span><span>{item.id === "A" ? "仅点阵" : item.id === "B" ? "仅边缘" : "静态"}</span></div>
                      <HudSurface scheme={item.id} state={state} reducedMotion={reducedMotion} freshFrames={freshFrames} scale={scale} />
                    </div>

                    <div className="preview-block preview-block--still">
                      <div className="preview-caption"><span>平面参考</span><span>同状态 · 静止</span></div>
                      <HudSurface scheme={item.id} state={state} staticPreview reducedMotion={true} freshFrames={false} scale={scale} />
                    </div>

                    <p className="scheme-reason">{item.reason}</p>
                  </article>
                ))}
              </div>
            </section>

            <section className="state-section" aria-labelledby="states-title">
              <div className="section-heading section-heading--states">
                <div>
                  <h2 id="states-title">A · 状态延展</h2>
                  <p>录音只保留真实点阵动效；准备、处理、错误、恢复与学习状态直接展示对应胶囊。</p>
                </div>
                <a className="text-button" href="?view=states&scheme=A">单独查看</a>
              </div>
              <StateGallery reducedMotion={reducedMotion} freshFrames={freshFrames} />
              <p className="hidden-note">已输入并确认时，HUD 立即隐藏；若学习或恢复提示仍在处理，则由该提示继续占用面板。</p>
            </section>
          </>
        ) : (
          <section className="states-page">
            <div className="section-heading">
              <div>
                <button type="button" className="text-button back-button" onClick={() => setView("compare")}>← 返回比较</button>
                <h2>A · 完整状态预览</h2>
                <p>状态条模拟产品状态的文案与层级；每项都使用中性黑白灰。</p>
              </div>
              <div className="states-feature">
                <span>当前选择：{selectedStateLabel}</span>
                <HudSurface scheme="A" state={state} staticPreview reducedMotion={true} freshFrames={false} />
                {state === "entered" && <p className="hidden-note">已确认输入，HUD 已隐藏。</p>}
              </div>
            </div>
            <StateGallery reducedMotion={reducedMotion} freshFrames={freshFrames} />
          </section>
        )}

        <footer className="page-footer">
          <span>预览倍率仅作用于 HUD 本身。AirPods Pro 与 00:08 是固定示例内容。</span>
          <span>只读原型 · 未改产品源代码</span>
        </footer>
      </div>
    </main>
  );
}

export default App;
