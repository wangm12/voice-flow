import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass, ghostButtonClass, focusRingClass } from "../lib/theme";
import type { WritingMode } from "./ContextSettings";

type TrialResult = {
  text: string;
  status: "model" | "local_only" | "provider_fallback" | "guard_fallback";
  elapsed_ms: number;
};
type TrialResponse = { saved: TrialResult | null; draft: TrialResult };

const statusLabels: Record<TrialResult["status"], string> = {
  model: "模型结果",
  local_only: "本地处理，未调用模型",
  provider_fallback: "服务未返回结果，已回退本地处理",
  guard_fallback: "事实保护规则触发，已回退",
};

export function WritingPreview({ mode, compareSaved, isDraft = compareSaved }: { mode: WritingMode; compareSaved: boolean; isDraft?: boolean }) {
  const { t } = useI18n();
  const [text, setText] = useState(() => t("嗯，帮我跟同事说一下，周五下午三点我们一起看 API 方案，先确认接口，再安排测试。"));
  const [result, setResult] = useState<TrialResponse | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestRef = useRef<string | null>(null);
  const runButtonRef = useRef<HTMLButtonElement>(null);
  const restoreRunFocusRef = useRef(false);
  const reasonId = useId();
  const unavailableReason = !mode.label.trim() ? t("请先填写语气名称。")
    : !mode.prompt.trim() ? t("请先填写 Prompt。")
      : !text.trim() ? t("请先填写试跑文本。")
        : new TextEncoder().encode(text).length > 16_384 ? t("试跑文本过长，请缩短后再试。") : null;

  useEffect(() => {
    if (!pending && restoreRunFocusRef.current) {
      restoreRunFocusRef.current = false;
      runButtonRef.current?.focus({ preventScroll: true });
    }
  }, [pending]);

  function cancel() {
    const requestId = requestRef.current;
    requestRef.current = null;
    if (requestId) void invoke("cancel_writing_preview", { requestId }).catch(() => {});
  }

  useEffect(() => {
    cancel();
    setPending(false);
    setResult(null);
    setError(null);
    return cancel;
  }, [mode.id, mode.label, mode.prompt, mode.family, compareSaved]);

  async function run() {
    cancel();
    const requestId = crypto.randomUUID();
    requestRef.current = requestId;
    setPending(true);
    setResult(null);
    setError(null);
    try {
      const response = await invoke<TrialResponse>("preview_writing_mode", {
        request: { request_id: requestId, text, mode, compare_saved: compareSaved },
      });
      if (requestRef.current === requestId) setResult(response);
    } catch (reason) {
      if (requestRef.current !== requestId) return;
      setError(reason === "preview_timeout" ? "试跑超时，请稍后重试。" : reason === "preview_cancelled"
        ? "配置已变化，请重新试跑。" : "试跑未完成，请检查语气和服务配置后重试。");
    } finally {
      if (requestRef.current === requestId) {
        requestRef.current = null;
        setPending(false);
      }
    }
  }

  return (
    <div className="mt-6 border-t border-border pt-4">
      <label className="block text-xs font-medium text-secondary">
        {t("语气试跑")}
        <textarea
          aria-label={t("试跑文本")}
          value={text}
          rows={3}
          maxLength={16_384}
          onChange={(event) => { cancel(); setPending(false); setResult(null); setError(null); setText(event.target.value); }}
          className={`mt-2 w-full resize-y rounded-lg border border-border bg-elevated px-3 py-2.5 text-sm font-normal leading-6 text-primary outline-none focus:border-accent ${focusRingClass}`}
        />
      </label>
      <p className="mt-2 text-xs leading-5 text-tertiary">{t("点击试跑后使用样例文字和 Prompt，可能调用当前整理服务；不会读取历史、保存结果或粘贴到其他 App。")}</p>
      {compareSaved && <p className="mt-1 text-xs text-secondary">{t("将使用同一段文字分别试跑已保存语气和当前草稿。")}</p>}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <button ref={runButtonRef} type="button" aria-busy={pending} aria-describedby={unavailableReason ? reasonId : undefined} onClick={() => void run()} disabled={pending || Boolean(unavailableReason)} className={`${secondaryButtonClass} min-w-36`}>
          {pending ? t("正在试跑…") : compareSaved ? t("试跑并对比") : t("试跑当前语气")}
        </button>
        {pending && <button type="button" className={ghostButtonClass} onClick={() => { restoreRunFocusRef.current = true; cancel(); setPending(false); }}>{t("取消")}</button>}
        <span className="text-xs text-secondary">{t(isDraft ? "使用当前草稿" : "使用已保存配置")}</span>
      </div>
      {unavailableReason && <p id={reasonId} className="mt-2 text-xs leading-5 text-secondary">{unavailableReason}</p>}
      <div aria-live="polite" aria-busy={pending}>
        {error && <p role="alert" className="mt-3 text-xs text-error-ink">{t(error)}</p>}
        {result && <div className={`mt-4 grid gap-3 ${result.saved ? "sm:grid-cols-2" : ""}`}>
          {result.saved && <Result result={result.saved} label={t("已保存语气")} />}
          <Result result={result.draft} label={t(isDraft ? "当前草稿" : "已保存语气")} />
        </div>}
      </div>
    </div>
  );
}

function Result({ result, label }: { result: TrialResult; label: string }) {
  const { t } = useI18n();
  return <div className="min-w-0 rounded-lg border border-border bg-elevated/60 p-3">
    <p className="text-xs font-medium text-primary">{label}</p>
    <p className={`mt-1 text-xs ${result.status === "model" ? "text-tertiary" : "text-warning-ink"}`}>{t(statusLabels[result.status])} · {(result.elapsed_ms / 1000).toFixed(1)}s</p>
    <p className="mt-3 whitespace-pre-wrap break-words text-sm leading-6 text-primary">{result.text}</p>
  </div>;
}
