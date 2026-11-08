import { CheckCircle2, Keyboard } from "lucide-react";
import { finishHints } from "../../lib/activationCopy";
import { iconPropsSm, iconPropsLg } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";
import { readinessLabel, readinessStageLabels, type ReadinessFact } from "../../lib/onboardingReadiness";
import { ghostButtonClass, secondaryButtonClass } from "../../lib/theme";
import type { OnboardingSettingsPage } from "./DataChoices";

export function FinishStep({
  hotkeyDisplay,
  activationMode,
  settings, facts, onExternal, onRetry, onSettings,
}: {
  hotkeyDisplay: string;
  activationMode: string;
  settings: Record<string, unknown>;
  facts: ReadinessFact[];
  onExternal: () => void;
  onRetry: () => void;
  onSettings: (page: OnboardingSettingsPage) => void;
}) {
  const { t } = useI18n();
  const hints = finishHints(activationMode, hotkeyDisplay, t);

  return (
    <div className="text-center">
      <div className="mx-auto flex h-11 w-11 items-center justify-center rounded-xl bg-success/10 text-success-ink">
        <CheckCircle2 {...iconPropsLg} className="text-success-ink" />
      </div>
      <h1 className="mt-4 text-[28px] font-semibold leading-tight tracking-tight">{t("设置已完成，随时可以重测")}</h1>
      <p className="mt-2 text-[13px] leading-6 text-secondary">{t("记住下面的快捷键。未完成的试用可以稍后回来继续。")}</p>
      <div className="mx-auto mt-6 max-w-sm border-y border-border py-4 text-left text-sm text-secondary">
        {hints.map((hint) => (
          <p key={hint} className="mt-2.5 flex items-center gap-2 first:mt-0">
            <Keyboard {...iconPropsSm} className="shrink-0 text-tertiary" aria-hidden="true" />
            {hint}
          </p>
        ))}
      </div>
      <dl className="mt-5 divide-y divide-border text-left text-[13px] text-secondary">
        {([
          ["microphone_frames", "麦克风自检"], ["in_app_trial", "听写试用"], ["external_delivery", "写入其他 App"],
        ] as const).map(([stage, label]) => <div key={stage} className="flex justify-between gap-4 py-3"><dt>{t(label)}</dt><dd>{readinessLabel(facts.find((fact) => fact.stage === stage)?.state ?? "not_tested", t)}</dd></div>)}
      </dl>
      <p className="mt-2 text-left text-xs leading-5 text-secondary">{t("完成设置不代表已经录音成功或确认外部插入。下面只显示当前配置的检查事实。")}</p>
      <div className="mt-4 flex flex-wrap justify-center gap-2"><button type="button" className={secondaryButtonClass} onClick={onRetry}>{t("重新试用听写")}</button><button type="button" className={ghostButtonClass} onClick={() => onSettings("permissions")}>{t("检查权限与设备")}</button><button type="button" className={ghostButtonClass} onClick={onExternal}>{t("开始受控外部测试（可选）")}</button></div>
      <details className="mt-5 border-y border-border py-4 text-left text-xs leading-5 text-secondary">
        <summary className="cursor-pointer text-sm font-medium text-primary">{t("查看配置与详细检查")}</summary>
        <div className="mt-3">
        <p>{t("选定麦克风")}：{String(settings.input_device || t("默认（跟随系统）"))}</p>
        <p>{t("当前实际设备")}：{String(settings.effective_input_device || t("尚未测试"))}</p>
        <p>{t("转写服务")}：{String(settings.asr_provider ?? "groq")} / {String(settings.asr_model ?? "")}</p>
        <p>{t("识别语言")}：{settings.language === "zh" ? t("中文") : settings.language === "en" ? "English" : t("自动检测")}</p>
        <dl className="mt-3 space-y-1">{Object.entries(readinessStageLabels).filter(([stage]) => settings.asr_provider === "on_device" || !["files_validated", "runtime_capability", "model_loaded"].includes(stage)).map(([stage, label]) => {
          const fact = facts.find((item) => item.stage === stage);
          return <div key={stage} className="flex flex-wrap justify-between gap-2"><dt>{t(label)}</dt><dd>{readinessLabel(fact?.state ?? "not_tested", t)}</dd></div>;
        })}</dl>
        <p className="mt-3">{t("应用内试用是预览。可主动准备固定 TextEdit 文档进行外部插入测试；回读与剪贴板恢复分别显示。")}</p>
        </div>
      </details>
      <details className="mt-5 text-left"><summary className="cursor-pointer text-sm font-medium">{t("失败时如何恢复")}</summary>
        <p className="mt-3 text-xs leading-5 text-secondary">{t("无声先自检设备；服务或模型失败先检查语音服务。插入未确认时检查目标和已有文字，不要盲目再次粘贴。")}</p>
        <p className="mt-2 text-xs leading-5 text-secondary">{t("只有实际保存成功才有历史记录；有恢复音频时可主动重转写，结果只复制。历史保存失败时不能保证结果已保存。")}</p>
        <button type="button" className={`mt-2 ${ghostButtonClass}`} onClick={() => onSettings("history")}>{t("查看历史与恢复")}</button>
      </details>
      <details className="mt-5 text-left"><summary className="cursor-pointer text-sm font-medium">{t("按需了解词典、上下文与风格")}</summary>
        <div className="mt-3 space-y-4 text-xs leading-5 text-secondary">
          <p>{t("添加常用术语，或审查纠错学习建议。学习规则只在符合其 App 与场景范围时应用，可随时撤销或忘记；替换次数不代表准确率。")} <button type="button" className={ghostButtonClass} onClick={() => onSettings("dictionary")}>{t("打开词典")}</button></p>
          <p>{t("上下文按具体 App 规则授权。AX、本机 OCR、向服务发送文字和云端视觉分别选择；看屏幕是独立操作，普通听写不默认截图。")} <button type="button" className={ghostButtonClass} onClick={() => onSettings("context")}>{t("打开上下文设置")}</button></p>
          <p>{t("聊天和邮件可选择不同语气；代码、终端和表单保持保守。已保存偏好、待确认风格草稿和已批准样例分别管理，批准可撤销。")} <button type="button" className={ghostButtonClass} onClick={() => onSettings("writing")}>{t("查看语气与试跑")}</button></p>
        </div>
      </details>
    </div>
  );
}
