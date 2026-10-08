import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass } from "../lib/theme";

export type MicrophoneStatus = {
  session_id: string;
  device_name: string;
  state: string;
  elapsed_secs: number;
  input_gain: number;
  level: number;
  peak: number;
  received_frames: boolean;
  signal_detected: boolean;
  clipping_detected: boolean;
  error: string | null;
};

export function MicrophoneCheck({ configurationKey = "" }: { configurationKey?: string }) {
  const { t } = useI18n();
  const [phase, setPhase] = useState("idle");
  const [status, setStatus] = useState<MicrophoneStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const session = useRef<string | null>(null);
  const subscription = useRef<(() => void) | null>(null);

  const release = () => {
    const id = session.current;
    session.current = null;
    subscription.current?.();
    subscription.current = null;
    if (id) void invoke("stop_microphone_check", { sessionId: id }).catch(() => undefined);
  };

  useEffect(() => {
    const cancel = () => {
      release();
      setPhase("idle");
      setStatus(null);
      setError(null);
    };
    const onVisibility = () => { if (document.visibilityState !== "visible") cancel(); };
    document.addEventListener("visibilitychange", onVisibility);
    // A settings change invalidates the device / gain used by the previous check.
    cancel();
    return () => { document.removeEventListener("visibilitychange", onVisibility); release(); };
  }, [configurationKey]);

  const start = async () => {
    if (session.current) return;
    const id = crypto.randomUUID();
    session.current = id;
    setPhase("starting");
    setStatus(null);
    setError(null);
    const accept = (next: MicrophoneStatus) => {
      if (session.current !== id || next.session_id !== id) return;
      setStatus(next);
      setPhase(next.state);
      if (next.state === "error") setError(t("麦克风已断开或无法继续输入，请重新选择设备后测试。"));
      if (next.state !== "running") release();
    };
    try {
      const unlisten = await listen<MicrophoneStatus>("microphone-check://state", (event) => accept(event.payload));
      if (session.current !== id) { unlisten(); return; }
      subscription.current = unlisten;
      const initial = await invoke<MicrophoneStatus>("start_microphone_check", { sessionId: id });
      // Events can finish the check before the start command resolves.
      if (session.current === id) accept(initial);
    } catch (reason) {
      if (session.current !== id) return;
      release();
      setPhase("error");
      const message = String(reason);
      setError(t(message.includes("microphone_check_busy")
        ? "请先结束当前听写或文字操作，再测试麦克风。"
        : message.includes("microphone_check_permission")
          ? "请先在权限设置中允许麦克风访问，再开始测试。"
          : "麦克风测试未能启动，请检查设备和系统权限后重试。"));
    }
  };

  const stop = () => { release(); setPhase("stopped"); };
  const active = phase === "starting" || phase === "running";
  const result = status?.clipping_detected
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
        <button type="button" aria-busy={phase === "starting"} className={`${secondaryButtonClass} w-44 max-w-full`} onClick={active ? stop : () => void start()}>{active ? t("停止测试") : t("开始麦克风测试")}</button>
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
