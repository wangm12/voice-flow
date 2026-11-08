import { useEffect, useRef, useState } from "react";
import { BorderBeam } from "border-beam";
import { ThinkingOrb } from "thinking-orbs";
import catalogue from "./states.json";
import notesIconUrl from "../assets/notes.png?inline";

type Specimen = (typeof catalogue.cases)[number];
const cases = catalogue.cases;
const byId = new Map(cases.map((item) => [item.id, item]));
const query = new URLSearchParams(window.location.search);
const exampleText = "帮我查看以下两点：\n1. 稳定性怎么样。\n2. 准确率怎么样。";
const scenarios = [
  { id: "normal", name: "顺利输入", steps: ["starting", "recording-airpods", "processing", "success"] },
  { id: "microphone", name: "AirPods 中断", steps: ["recording-airpods", "mic-stale", "mic-interrupted", "audio-retained"] },
  { id: "retry", name: "网络重试", steps: ["processing", "retry-wait", "processing", "success"] },
  { id: "target", name: "录音时切换 App", steps: ["recording-airpods", "app-switch-recording", "processing", "target-changed"] },
  { id: "unverified", name: "输入无法回读", steps: ["recording-airpods", "processing", "input-unverified"] },
  { id: "recovery", name: "录音保存失败", steps: ["processing", "audio-retained", "audio-saving", "audio-save-failed"] },
  { id: "learning", name: "学习与撤销失败", steps: ["learned", "learn-undo", "learn-undo-failed"] },
];

function Icon({ name, className = "" }: { name: string; className?: string }) {
  return <svg className={`glyph ${className}`} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.35" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {name === "stop" ? <rect x="5" y="5" width="10" height="10" rx="1" fill="currentColor" strokeWidth="1.6" />
      : name === "arrow" ? <path d="m8 5 5 5-5 5" />
      : name === "close" ? <path d="m6 6 8 8M14 6l-8 8" />
      : name === "mic" || name === "mic-off" ? <><rect x="7" y="2.5" width="6" height="10" rx="3" /><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5m-3 0h6" />{name === "mic-off" && <path d="M3 3 17 17" />}</>
      : name === "lock" ? <><rect x="4.5" y="8.5" width="11" height="9" rx="2" /><path d="M7 8.5V5a3 3 0 0 1 6 0v3.5M10 12v2" /></>
      : name === "offline" ? <><path d="M2.5 6.5a12 12 0 0 1 15 0M5 10a8 8 0 0 1 10 0M8 13a3 3 0 0 1 4 0M3 3l14 14" /><circle cx="10" cy="16" r=".7" fill="currentColor" /></>
      : name === "clock" ? <><circle cx="10" cy="10" r="7" /><path d="M10 5.5V10l3 2" /></>
      : name === "archive" ? <><rect x="3" y="3" width="14" height="4" rx="1" /><path d="M4.5 7v9.5h11V7M8 10h4" /></>
      : name === "clipboard" ? <><rect x="5" y="4" width="11" height="13" rx="2" /><rect x="7.5" y="2" width="6" height="4" rx="1" /><path d="M8 10h5M8 13h4" /></>
      : name === "target" ? <><rect x="3" y="3" width="14" height="14" rx="3" /><path d="M7 10h6M10 7v6" /></>
      : name === "cursor" ? <><path d="m5 2 10 8-5 .5L8 16 5 2ZM11 12l3 5" /></>
      : name === "dictionary" ? <><path d="M3 4h5a3 3 0 0 1 2 1 3 3 0 0 1 2-1h5v12h-5a3 3 0 0 0-2 1 3 3 0 0 0-2-1H3V4ZM10 5v12" /></>
      : name === "model" ? <><rect x="5" y="5" width="10" height="10" rx="2" /><path d="M8 1.5V5M12 1.5V5M8 15v3.5M12 15v3.5M1.5 8H5M1.5 12H5M15 8h3.5M15 12h3.5" /></>
      : name === "shield" ? <><path d="m10 2 6 2.5V10c0 4-6 7.5-6 7.5S4 14 4 10V4.5L10 2Z" /><path d="m7 10 2 2 4-4" /></>
      : name === "document" ? <><path d="M5 2.5h7l4 4v11H5V2.5ZM12 2.5V7h4M8 10h5M8 13h4" /></>
      : name === "settings" ? <><path d="M3 5h14M3 10h14M3 15h14" /><circle cx="7" cy="5" r="2" fill="#222" /><circle cx="13" cy="10" r="2" fill="#222" /><circle cx="8" cy="15" r="2" fill="#222" /></>
      : name === "orb" ? <><circle cx="6" cy="7" r="1.3" fill="currentColor" /><circle cx="13" cy="5" r=".8" fill="currentColor" /><circle cx="15" cy="11" r="1.1" fill="currentColor" /><circle cx="10" cy="15" r="1.4" fill="currentColor" /><circle cx="5" cy="13" r=".7" fill="currentColor" /><circle cx="10" cy="9" r=".7" fill="currentColor" /></>
      : <><circle cx="10" cy="10" r="7" /><path d="M10 6v4M10 13h.01" /></>}
  </svg>;
}

function Beam({ mode, visible, active }: { mode: "rotate" | "line"; visible: boolean; active: boolean }) {
  return <BorderBeam className={`beam-layer beam-layer--${mode}`} style={{ position: "absolute", inset: 0, width: 220, height: 48, zIndex: 2, opacity: visible ? 1 : 0, pointerEvents: "none", transition: "opacity 180ms cubic-bezier(.22,1,.36,1)" }} size={mode === "rotate" ? "md" : "line"} colorVariant="mono" theme="dark" staticColors strength={mode === "rotate" ? 1 : .65} brightness={1.3} duration={mode === "rotate" ? 1.96 : 3.1} borderRadius={24} active={active} aria-hidden="true"><span className="beam-layer__inner" /></BorderBeam>;
}

function Pill({ item, live = false, motion = false, onAction, onDetails }: { item: Specimen; live?: boolean; motion?: boolean; onAction?: () => void; onDetails?: () => void }) {
  if (item.hidden) return <div className="absent-slot"><span>HUD 隐藏</span><span>{item.id === "idle" ? "等待快捷键" : "不弹成功提示"}</span></div>;
  const busy = item.action === "busy";
  const stop = item.action === "stop";
  const recording = item.motion === "rotate";
  const twoRows = !!item.subtitle;
  const actionName = stop ? (recording || item.id === "recording-limit" ? "停止录音并整理" : "取消本次听写") : busy ? item.primary : "查看恢复详情";
  const copy = <><span className="pill-title">{item.title}</span>{twoRows && <span className="pill-subtitle">{item.subtitle}</span>}</>;
  return <div className={`hud-capsule${motion ? "" : " hud-capsule--paused"}`} data-state-id={item.id} data-live={live}>
    <div className="hud-shell">
      <span className="icon-slot" aria-hidden="true" key={item.icon}>
        {item.icon === "notes" ? <img src={notesIconUrl} alt="" /> : item.icon === "orb" && live ? <ThinkingOrb state="working" size={20} theme="dark" paused={!motion} /> : <Icon name={item.icon} />}
      </span>
      {live && !stop && !busy ? <button type="button" className={`copy-dock${twoRows ? " copy-dock--two" : ""}`} onClick={onDetails} aria-label={`${item.title}，${item.subtitle}。查看恢复详情`}>{copy}</button> : <span className={`copy-dock${twoRows ? " copy-dock--two" : ""}`}>{copy}</span>}
      {recording && !item.id.startsWith("app-switch") && <time className="recording-timer" aria-hidden="true">00:08</time>}
    </div>
    {live ? <button className="hud-action-button" type="button" aria-label={actionName} title={actionName} disabled={busy} onKeyDown={(e) => { if (e.repeat && (e.key === "Enter" || e.key === " ")) e.preventDefault(); }} onClick={(e) => { if (e.detail > 1) return; stop ? onAction?.() : onDetails?.(); }}><Icon name={stop ? "stop" : busy ? "clock" : "arrow"} /></button> : <span className="hud-action-button" aria-hidden="true"><Icon name={stop ? "stop" : busy ? "clock" : "arrow"} /></span>}
    {live && <><Beam mode="rotate" visible={item.motion === "rotate"} active={motion && item.motion === "rotate"} /><Beam mode="line" visible={item.motion === "line"} active={motion && item.motion === "line"} /></>}
  </div>;
}

export default function App() {
  const [selected, setSelected] = useState(byId.has(query.get("state") ?? "") ? query.get("state")! : "mic-interrupted");
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [theme, setTheme] = useState(query.get("theme") === "light" ? "light" : "dark");
  const [manualReduced, setManualReduced] = useState(query.get("motion") === "reduced");
  const [systemReduced, setSystemReduced] = useState(false);
  const [pageVisible, setPageVisible] = useState(!document.hidden);
  const [inView, setInView] = useState(true);
  const [details, setDetails] = useState(false);
  const [feedback, setFeedback] = useState("");
  const [scenarioId, setScenarioId] = useState("normal");
  const [playing, setPlaying] = useState(false);
  const [stepIndex, setStepIndex] = useState(0);
  const [sourceOpen, setSourceOpen] = useState(false);
  const timers = useRef<number[]>([]);
  const stageRef = useRef<HTMLDivElement>(null);
  const detailTrigger = useRef<HTMLButtonElement | null>(null);
  const item = byId.get(selected)!;
  const scenario = scenarios.find((entry) => entry.id === scenarioId)!;
  const reduced = manualReduced || systemReduced;
  const motion = !reduced && pageVisible && inView;
  function clearTimers() { timers.current.forEach(window.clearTimeout); timers.current = []; }
  function select(id: string) { clearTimers(); setPlaying(false); setDetails(false); setFeedback(""); setSelected(id); }
  function openDetails() { clearTimers(); setPlaying(false); detailTrigger.current = document.activeElement instanceof HTMLButtonElement ? document.activeElement : null; setDetails(true); setFeedback(""); }
  function closeDetails() { setDetails(false); setFeedback(""); detailTrigger.current?.focus(); }
  function handleStop() { select(item.motion === "rotate" || item.id === "recording-limit" ? "processing" : "cancelled"); }
  function run() { clearTimers(); setDetails(false); setFeedback(""); setStepIndex(0); setSelected(scenario.steps[0]); setPlaying(true); }
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const sync = () => setSystemReduced(media.matches);
    const visibility = () => { setPageVisible(!document.hidden); if (document.hidden) { clearTimers(); setPlaying(false); } };
    sync(); media.addEventListener("change", sync); document.addEventListener("visibilitychange", visibility);
    return () => { media.removeEventListener("change", sync); document.removeEventListener("visibilitychange", visibility); clearTimers(); };
  }, []);
  useEffect(() => {
    if (!stageRef.current) return;
    const observer = new IntersectionObserver(([entry]) => { setInView(entry.isIntersecting); if (!entry.isIntersecting) setPlaying(false); }, { threshold: .1 });
    observer.observe(stageRef.current); return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!playing || reduced || !pageVisible || !inView) return;
    if (stepIndex >= scenario.steps.length - 1) { setPlaying(false); return; }
    const timer = window.setTimeout(() => { setStepIndex(stepIndex + 1); setSelected(scenario.steps[stepIndex + 1]); }, selected === "starting" ? 800 : 1800);
    return () => window.clearTimeout(timer);
  }, [playing, reduced, pageVisible, inView, stepIndex, scenarioId, selected]);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.target instanceof HTMLSelectElement || event.target instanceof HTMLInputElement) return;
      if (details) { event.preventDefault(); closeDetails(); }
      else if (item.action === "stop") { event.preventDefault(); select("cancelled"); }
    };
    document.addEventListener("keydown", escape); return () => document.removeEventListener("keydown", escape);
  }, [details, item.id]);
  useEffect(() => {
    const params = new URLSearchParams({ state: selected, theme, motion: manualReduced ? "reduced" : "full" });
    window.history.replaceState(null, "", `${location.pathname}?${params.toString()}`);
  }, [selected, theme, manualReduced]);

  function primaryAction() {
    const route = item.route;
    if (route === "save-audio" || route === "undo") {
      select(route === "save-audio" ? "audio-saving" : "learn-undo");
      timers.current.push(window.setTimeout(() => { setSelected(route === "save-audio" ? "audio-saved" : "learn-undone"); }, 1000));
    } else if (route === "dismiss") select("cancelled");
    else setFeedback(route === "result" ? "示例文字如下；本原型没有读写系统剪贴板。" : route === "history" ? "模拟：打开对应的历史记录。不会再次自动粘贴。" : "模拟：打开对应设置或说明。没有修改 macOS 或 VoiceFlow 设置。");
  }

  const visible = cases.filter((entry) => (filter === "all" || entry.group === filter) && `${entry.name} ${entry.title} ${entry.subtitle} ${entry.sources.map((s) => s.value).join(" ")}`.toLowerCase().includes(search.toLowerCase()));
  const counts = catalogue.groups.map((group) => ({ ...group, count: cases.filter((entry) => entry.group === group.id).length }));
  return <main className={`workbench workbench--${theme}${reduced ? " motion-reduced" : ""}`}>
    <div className="page-shell">
      <header className="page-header"><div><p className="eyebrow">VOICEFLOW · STATE ATLAS / 10</p><h1>听写状态全览</h1><p className="intro">正常流程、异常提示与恢复操作，一起检查。</p></div><div className="prototype-tag">交互设计稿<br />不连接录音、模型或剪贴板</div></header>
      <section className="preview-section" aria-labelledby="preview-title">
        <div className="preview-heading"><div><h2 id="preview-title">{item.name}</h2><p>{item.proposed ? "补充设计 · 需要原生事件支持" : "依据现有状态 · 文案与布局为本次设计"}</p></div><span className="size-badge">220 × 48 · R24 · 1×</span></div>
        <div className="live-stage" ref={stageRef}>
          <div className="live-presentation"><Pill item={item} live motion={motion} onAction={handleStop} onDetails={openDetails} /></div>
          <p className="stage-note">{item.note}</p>
          {details && <section className="recovery-panel" aria-label="恢复详情"><div className="recovery-panel__head"><strong>{item.title}</strong><button className="plain-icon" aria-label="关闭恢复详情" onClick={closeDetails}><Icon name="close" /></button></div><p>{item.message}</p><div className="recovery-panel__actions"><button className="primary-button" onClick={primaryAction}>{item.primary}</button><button className="quiet-button" onClick={() => select("cancelled")}>关闭提示</button></div>{feedback && <p className="feedback" role="status">{feedback}</p>}{feedback && ["history", "result"].includes(item.route) && <pre className="example-text" aria-label="示例结果">{exampleText}</pre>}<span className="simulation-caption">以上操作均为模拟，不调用系统功能</span></section>}
        </div>
        <div className="live-description" role="status" aria-live="polite" aria-atomic="true"><span>{item.hidden ? item.detail : `${item.title}${item.subtitle ? ` · ${item.subtitle}` : ""}`}</span><button className="quiet-button" onClick={() => select("recording-airpods")}>重新开始预览</button></div>
        <div className="control-rail"><label className="scenario-picker"><span>模拟流程</span><select value={scenarioId} onChange={(e) => { clearTimers(); setPlaying(false); setScenarioId(e.target.value); setStepIndex(0); }} aria-label="模拟流程">{scenarios.map((entry) => <option value={entry.id} key={entry.id}>{entry.name}</option>)}</select></label><button className="quiet-button play-button" onClick={playing ? () => setPlaying(false) : run} disabled={reduced}>{playing ? "暂停演示" : "播放流程"}</button><label className="check-control"><input type="checkbox" checked={manualReduced} onChange={(e) => { setManualReduced(e.target.checked); setPlaying(false); }} />减少动态效果</label><div className="theme-control" aria-label="页面背景"><button aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>深色</button><button aria-pressed={theme === "light"} onClick={() => setTheme("light")}>浅色</button></div></div>
        <p className="simulation-note">示例设备、时间和演示节奏均为固定样本；光束表示正在工作，不表示完成百分比。只有上方胶囊运行动效，下方为静态总览。</p>
      </section>
      <section className="atlas-section" aria-labelledby="atlas-title"><div className="atlas-heading"><div><h2 id="atlas-title">全部 {cases.length} 个状态样例</h2><p>点击任意样例，在上方检查动效、短提示和恢复操作。</p></div><label className="search-field"><span className="sr-only">搜索状态</span><input type="search" placeholder="搜索状态或错误原因" value={search} onChange={(e) => setSearch(e.target.value)} /></label></div>
        <nav className="filter-nav" aria-label="状态分类"><button aria-pressed={filter === "all"} onClick={() => setFilter("all")}>全部 <span>{cases.length}</span></button>{counts.map((group) => <button key={group.id} aria-pressed={filter === group.id} onClick={() => setFilter(group.id)}>{group.label} <span>{group.count}</span></button>)}</nav>
        <div className="state-grid">{visible.map((entry) => <button key={entry.id} className={`state-tile${selected === entry.id ? " state-tile--selected" : ""}`} aria-pressed={selected === entry.id} aria-label={`预览：${entry.name}`} onClick={() => { select(entry.id); stageRef.current?.scrollIntoView({ behavior: reduced ? "instant" : "smooth", block: "center" }); }}><span className="tile-heading"><span className="tile-index">{String(cases.indexOf(entry) + 1).padStart(2, "0")}</span><strong>{entry.name}</strong>{entry.proposed && <span className="proposed-tag">补充</span>}</span><span className="tile-preview"><Pill item={entry} /></span><span className="tile-note">{entry.note}</span></button>)}</div>
        {visible.length === 0 && <p className="empty-search">没有匹配的状态。<button className="quiet-button" onClick={() => { setSearch(""); setFilter("all"); }}>清除筛选</button></p>}
      </section>
      <section className="coverage-section"><button className="coverage-toggle" aria-expanded={sourceOpen} onClick={() => setSourceOpen(!sourceOpen)}>状态覆盖与设计边界 <span>{sourceOpen ? "−" : "+"}</span></button>{sourceOpen && <div className="coverage-content"><p>覆盖当前代码的 12 个主状态、8 个处理阶段、7 个采集状态、4 个音频恢复结果、6 个选中文本操作分支及 {Object.keys(catalogue.coverage.delivery_reason).length} 个交付原因。多个内部状态共用一个视觉样例；这不代表每一个任意组合都可达。</p><p>新增短文案、统一尺寸、恢复面板和“补充”标记的状态均为设计稿，尚未接入产品。保存或复制的承诺只属于详情中已明确确认的示例条件。用户不需要在日常 HUD 看到这些内部编码。</p><div className="source-mapping"><strong>当前样例：{item.name}</strong><p>{item.detail}</p>{item.sources.length ? <ul>{item.sources.map((source) => <li key={`${source.kind}:${source.value}`}><span>{source.kind}</span><code>{source.value}</code></li>)}</ul> : <p>体验变体或补充设计，没有独立的现有状态编码。</p>}</div><p className="source-list">核对来源：{catalogue.sourceFiles.join(" · ")}</p></div>}</section>
      <footer><span>黑白灰 · 同尺寸 · 正常完成即隐藏</span><span>Esc 取消活动听写 / 关闭详情 · 仅设计预览</span></footer>
    </div>
  </main>;
}
