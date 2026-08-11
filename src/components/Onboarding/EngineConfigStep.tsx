import { CloudCog, Lock } from "lucide-react";
import { PasswordInput } from "../PasswordInput";
import { buttonClass } from "../../lib/theme";
import { ValidationStatus } from "./ValidationStatus";
import { iconProps, iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function EngineConfigStep({
  keyValue,
  onKeyChange,
  valid,
  validating,
  onValidate,
  error,
}: {
  keyValue: string;
  onKeyChange: (value: string) => void;
  valid: string | null;
  validating: boolean;
  onValidate: () => void;
  error?: string | null;
}) {
  const { t } = useI18n();
  const isValid = valid === "valid";

  return (
    <div>
      <h1 className="text-2xl font-semibold tracking-tight text-primary">
        {t("连接语音服务")}
      </h1>
      <p className="mt-1.5 text-sm text-secondary">
        {t("VoiceFlow 使用 Groq 将语音转换成文字。输入访问密钥，验证通过后即可开始。")}
      </p>

      <div className="mt-6 border-y border-border">
        <div className="flex items-start gap-3 py-4">
          <CloudCog {...iconProps} className="mt-0.5 shrink-0 text-secondary" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-primary">{t("当前服务 · Groq")}</p>
            <p className="mt-1 text-xs leading-relaxed text-secondary">{t("低延迟")} · {t("适合日常口述；密钥只保存在这台 Mac 上。")}</p>
          </div>
        </div>

        <div className="border-t border-border py-4">
          <label htmlFor="onboarding-groq-api-key" className="block text-sm font-medium text-primary">{t("Groq API Key（访问密钥）")}</label>
          <p className="mt-0.5 text-xs text-tertiary">{t("在 Groq Console 创建，通常以 gsk_ 开头。")}</p>
          <PasswordInput id="onboarding-groq-api-key" ariaLabel={t("Groq API Key")} value={keyValue} onChange={onKeyChange} placeholder="gsk_…" valid={isValid} monospace className="mt-2" />
        </div>

        <div className="flex items-center gap-2 pb-4">
          <button type="button" onClick={() => void onValidate()} disabled={!keyValue.trim() || validating} className={`${buttonClass} min-w-[80px]`}>
            {validating ? t("验证中…") : t("验证 API Key")}
          </button>
          <ValidationStatus status={valid} validating={validating} />
        </div>

        {error && <p role="alert" className="border-t border-error/20 py-3 text-xs text-error">{error}</p>}

        <p className="flex items-center gap-1.5 border-t border-border py-4 text-xs text-tertiary">
          <Lock {...iconPropsSm} className="shrink-0" />
          {t("密钥仅保存在这台 Mac 的钥匙串中；VoiceFlow 不会代存，验证时只发送到 Groq")}
        </p>
      </div>
    </div>
  );
}
