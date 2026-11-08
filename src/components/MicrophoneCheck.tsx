import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useI18n } from "../lib/i18n";
import { closedLidMicrophoneErrorMessage } from "../lib/deliveryCopy";
import { secondaryButtonClass } from "../lib/theme";

export type MicrophoneStatus = {
  session_id: string;
  device_name: string;
  state: string;
  stream_released: boolean;
  elapsed_secs: number;
  input_gain: number;
  level: number;
  peak: number;
  received_frames: boolean;
  frame_fresh: boolean;
  signal_detected: boolean;
  clipping_detected: boolean;
  error: string | null;
};

type MicrophoneLease = {
  id: string;
  configuration: string;
  unlisten: (() => void) | null;
  stop: Promise<void> | null;
  failure: string | null;
  released: boolean;
};
// Failed cleanup remains owned across unmount/reentry; the same lease also owns
// its listener until a matching audio-thread release fact arrives.
const pendingMicrophoneStops = new Map<string, MicrophoneLease>();
function confirmMicrophoneRelease(lease: MicrophoneLease) {
  lease.released = true;
  pendingMicrophoneStops.delete(lease.id);
  lease.unlisten?.();
  lease.unlisten = null;
  lease.failure = null;
}
async function releaseMicrophoneLease(lease: MicrophoneLease): Promise<void> {
  if (lease.released) {
    // A concurrent cleanup may revisit a lease after its drop fact already
    // arrived. Keep the global ownership map and listener retired as well.
    confirmMicrophoneRelease(lease);
    return;
  }
  if (lease.stop) return lease.stop;
  pendingMicrophoneStops.set(lease.id, lease);
  const request = (async () => {
    try {
      const receipt = await invoke<unknown>("stop_microphone_check", { sessionId: lease.id });
      if (!receipt || typeof receipt !== "object") throw new Error("microphone_check_stop_ack_unconfirmed");
      const fact = receipt as Record<string, unknown>;
      if (fact.session_id !== lease.id || (fact.state !== "stopped" && fact.state !== "inactive_cancelled")) throw new Error("microphone_check_stop_ack_unconfirmed");
      confirmMicrophoneRelease(lease);
    } catch (reason) {
      if (lease.released) return; // Matching terminal-after-drop event already confirmed it.
      lease.failure = String(reason);
      throw reason;
    }
  })();
  lease.stop = request;
  try { await request; } finally { lease.stop = null; }
}

export function MicrophoneCheck({ configurationKey = "", disabled = false, onStatus }: { configurationKey?: string; disabled?: boolean; onStatus?: (status: MicrophoneStatus) => void }) {
  const { t } = useI18n();
  const [phase, setPhase] = useState("idle");
  const [status, setStatus] = useState<MicrophoneStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const statusCallback = useRef(onStatus);
  statusCallback.current = onStatus;
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;
  const currentConfiguration = useRef(configurationKey);
  currentConfiguration.current = configurationKey;
  const mounted = useRef(false);
  const session = useRef<MicrophoneLease | null>(null);

  const release = async () => {
    const lease = session.current;
    if (lease?.released) confirmMicrophoneRelease(lease);
    else if (lease) pendingMicrophoneStops.set(lease.id, lease);
    try {
      for (const owned of [...pendingMicrophoneStops.values()]) {
        await releaseMicrophoneLease(owned);
        if (session.current === owned) session.current = null;
      }
    } catch (reason) {
      if (mounted.current) {
        setPhase("cleanup_failed");
        setError(t("麦克风自检关闭尚未确认，请重试关闭后再开始新测试。"));
      }
      throw reason;
    }
  };

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    const cancel = () => {
      setStatus(null);
      setError(null);
      if (session.current || pendingMicrophoneStops.size) setPhase("stopping");
      void release().then(() => { if (mounted.current && !session.current) setPhase("idle"); }).catch(() => undefined);
    };
    const onVisibility = () => { if (document.visibilityState !== "visible") cancel(); };
    document.addEventListener("visibilitychange", onVisibility);
    // A settings change invalidates the device / gain used by the previous check.
    cancel();
    return () => { document.removeEventListener("visibilitychange", onVisibility); void release().catch(() => undefined); };
  }, [configurationKey, disabled]);

  const start = async () => {
    if (disabledRef.current) return;
    if (session.current && !pendingMicrophoneStops.has(session.current.id)) return;
    try { await release(); } catch { return; }
    if (!mounted.current || disabledRef.current || currentConfiguration.current !== configurationKey) return;
    const id = crypto.randomUUID();
    const startedConfiguration = configurationKey;
    const lease: MicrophoneLease = { id, configuration: startedConfiguration, unlisten: null, stop: null, failure: null, released: false };
    session.current = lease;
    setPhase("starting");
    setStatus(null);
    setError(null);
    const accept = (next: MicrophoneStatus) => {
      if (session.current !== lease || lease.released || next.session_id !== id) return;
      const terminal = ["completed", "stopped", "interrupted", "error"].includes(next.state);
      if (terminal && next.stream_released === true) {
        confirmMicrophoneRelease(lease);
        session.current = null;
        if (!mounted.current || currentConfiguration.current !== lease.configuration) return;
        setStatus(next); statusCallback.current?.(next); setPhase(next.state);
        setError(next.state === "error" ? t("麦克风已断开或无法继续输入，请重新选择设备后测试。") : null);
        return;
      }
      if (disabledRef.current || currentConfiguration.current !== lease.configuration) {
        setPhase("stopping");
        void release().then(() => {
          if (mounted.current && !session.current) { setPhase("idle"); setStatus(null); setError(null); }
        }).catch(() => undefined);
        return;
      }
      if (!mounted.current || pendingMicrophoneStops.has(id)) return;
      if (next.state !== "running" || next.stream_released !== false) {
        // A terminal label without causal drop evidence cannot close the lease.
        setPhase("stopping");
        void release().then(() => { if (mounted.current && !session.current) { setPhase("stopped"); setError(null); } }).catch(() => undefined);
        return;
      }
      setStatus(next); statusCallback.current?.(next); setPhase("running");
    };
    try {
      const unlisten = await listen<MicrophoneStatus>("microphone-check://state", (event) => accept(event.payload));
      if (session.current !== lease || lease.released) {
        unlisten();
        if (lease.released) confirmMicrophoneRelease(lease);
        if (session.current === lease) session.current = null;
        return;
      }
      lease.unlisten = unlisten;
      if (disabledRef.current || currentConfiguration.current !== lease.configuration) {
        setPhase("stopping");
        void release().then(() => {
          if (mounted.current && !session.current) { setPhase("idle"); setStatus(null); setError(null); }
        }).catch(() => undefined);
        return;
      }
      const initial = await invoke<MicrophoneStatus>("start_microphone_check", { sessionId: id });
      // Events can finish the check before the start command resolves.
      if (session.current === lease && !lease.released) accept(initial);
    } catch (reason) {
      if (session.current !== lease) return;
      void release().then(() => { if (mounted.current && !session.current) setPhase("error"); }).catch(() => undefined);
      const message = String(reason);
      setError(closedLidMicrophoneErrorMessage(message, t) ?? t(
        message.includes("microphone_check_busy")
          ? "请先结束当前听写或文字操作，再测试麦克风。"
          : message.includes("microphone_check_permission")
            ? "请先在权限设置中允许麦克风访问，再开始测试。"
            : "麦克风测试未能启动，请检查设备和系统权限后重试。"));
    }
  };

  const stop = async () => { setPhase("stopping"); try { await release(); if (mounted.current && !session.current) { setPhase("stopped"); setError(null); } } catch { /* Ownership and error are retained by release. */ } };
  const active = phase === "starting" || phase === "running" || phase === "stopping" || phase === "cleanup_failed";
  const result = phase === "running" && status?.frame_fresh === false
    ? status.received_frames
      ? t("麦克风暂未收到新的音频帧；此前输入记录仍保留，请检查设备连接。")
      : t("尚未收到音频输入，请检查设备连接。")
    : status?.clipping_detected
    ? t("输入峰值过高，请降低麦克风音量或输入增益后重试。")
    : status?.signal_detected
      ? t("已收到输入信号。音量测试不能判断识别准确率。")
      : status?.received_frames
        ? t("设备正在输入，但声音较轻。请说一句话，或检查静音和输入音量。")
        : t("尚未收到音频输入，请检查设备连接。") ;

  return (
    <section aria-label={t("麦克风自检")} className="py-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-primary">{t("麦克风自检")}</h2>
          <p className="mt-1 max-w-prose text-xs leading-5 text-secondary">{t("主动测试 30 秒，只在本机测量音量，不保存音频或发送到语音服务。")}</p>
        </div>
        <button type="button" aria-busy={phase === "starting"} className={`${secondaryButtonClass} w-44 max-w-full`} disabled={phase === "stopping" || (disabled && !active)} onClick={active ? () => void stop() : () => void start()}>{active ? t("停止测试") : t("开始麦克风测试")}</button>
      </div>
      {phase === "starting" && <p role="status" className="mt-3 text-xs text-secondary">{t("正在打开麦克风…")}</p>}
      {status && (
        <div className="mt-3 space-y-2">
          <p className="text-xs text-secondary">{status.device_name} · {t("输入增益")} {status.input_gain.toFixed(1)}× · {Math.min(status.elapsed_secs, 30)}/30 {t("秒")}</p>
          <meter aria-label={t("麦克风输入音量")} min={0} max={1} value={active ? status.level : 0} className="block h-3 w-full accent-success" />
          <p role="status" className={`text-xs leading-5 ${status.clipping_detected ? "text-warning-ink" : "text-secondary"}`}>
            {phase === "interrupted" ? t("测试已结束：设备设置已改变，或正式听写已开始。") : result}
          </p>
          {(phase === "completed" || phase === "stopped") && <p className="text-xs text-tertiary">{t("测试已结束，麦克风自检已关闭。")}</p>}
        </div>
      )}
      {error && <p role="alert" className="mt-3 text-xs text-error-ink">{error}</p>}
    </section>
  );
}
