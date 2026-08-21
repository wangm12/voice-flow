import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { KeyRound } from "lucide-react";
import { ConfirmDialog } from "../ConfirmDialog";
import { PasswordInput } from "../PasswordInput";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { ValidationStatus } from "../Onboarding/ValidationStatus";
import { useI18n } from "../../lib/i18n";
import { friendlySettingsError } from "../../lib/settingsError";
import { buttonClass, colors, focusRingClass, radius, secondaryButtonClass } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;
const cleanupModelOptions = [
  { value: "openai/gpt-oss-20b", label: "GPT-OSS 20B", note: "默认 · 更快" },
  { value: "openai/gpt-oss-120b", label: "GPT-OSS 120B", note: "质量更高 · 较慢" },
] as const;

function modelLabel(model: string): string {
  if (model === "whisper-large-v3-turbo") return "Whisper Large v3 Turbo";
  if (model === "openai/gpt-oss-20b") return "GPT-OSS 20B";
  if (model === "openai/gpt-oss-120b") return "GPT-OSS 120B";
  return model;
}

export function EngineSettings({
  settings,
  save,
  saveApiKey,
  removeApiKey,
  saveAsrApiKey,
  removeAsrApiKey,
}: {
  settings: Settings;
  save: SaveSettings;
  saveApiKey: (apiKey: string) => Promise<void>;
  removeApiKey: () => Promise<void>;
  saveAsrApiKey: (apiKey: string) => Promise<void>;
  removeAsrApiKey: () => Promise<void>;
}) {
  const { t } = useI18n();
  const [apiKeyDraft, setApiKeyDraft] = useState("");
  const [apiKeySaveError, setApiKeySaveError] = useState<string | null>(null);
  const [validating, setValidating] = useState(false);
  const [valid, setValid] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [confirmRemoveKey, setConfirmRemoveKey] = useState(false);
  const [asrKeyDraft, setAsrKeyDraft] = useState("");
  const [asrKeySaveError, setAsrKeySaveError] = useState<string | null>(null);
  const [removingAsrKey, setRemovingAsrKey] = useState(false);
  const [confirmRemoveAsrKey, setConfirmRemoveAsrKey] = useState(false);
  const asrKeyConfigured = Boolean(settings.asr_api_key_configured);

  const validate = async () => {
    setValidating(true);
    try {
      setValid(apiKeyDraft.trim()
        ? await invoke<string>("validate_api_key", { key: apiKeyDraft.trim() })
        : await invoke<string>("validate_configured_api_key"));
    } catch {
      setValid("ipc_error");
    } finally {
      setValidating(false);
    }
  };

  const commitApiKey = async () => {
    if (!apiKeyDraft.trim()) return;
    setApiKeySaveError(null);
    setValid(null);
    try {
      await saveApiKey(apiKeyDraft);
      setApiKeyDraft("");
      setValid("valid");
    } catch (reason) {
      setApiKeySaveError(friendlySettingsError(reason, t));
    }
  };

  const commitRemoveApiKey = async () => {
    setConfirmRemoveKey(false);
    setRemoving(true);
    try {
      await removeApiKey();
      setApiKeyDraft("");
      setValid(null);
      setApiKeySaveError(null);
    } catch (reason) {
      setApiKeySaveError(friendlySettingsError(reason, t));
    } finally {
      setRemoving(false);
    }
  };

  const commitAsrApiKey = async () => {
    if (!asrKeyDraft.trim()) return;
    setAsrKeySaveError(null);
    try {
      await saveAsrApiKey(asrKeyDraft);
      setAsrKeyDraft("");
    } catch (reason) {
      setAsrKeySaveError(friendlySettingsError(reason, t));
    }
  };

  const commitRemoveAsrApiKey = async () => {
    setConfirmRemoveAsrKey(false);
    setRemovingAsrKey(true);
    try {
      await removeAsrApiKey();
      setAsrKeyDraft("");
      setAsrKeySaveError(null);
    } catch (reason) {
      setAsrKeySaveError(friendlySettingsError(reason, t));
    } finally {
      setRemovingAsrKey(false);
    }
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("语音服务")} description={`${t("连接 Groq：")}${modelLabel(settings.asr_model)}${t("负责语音转文字，")}${modelLabel(settings.cleanup_model)}${t("负责整理文字。密钥只保存在这台 Mac 上。")}`} />
      <SettingsGroup title={t("服务凭据")}>
        <div className="px-4 py-4 sm:px-5">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <p className="text-sm font-medium text-primary">{t("Groq API Key")}</p>
              <p className="mt-1 text-xs leading-5 text-tertiary">{settings.api_key_configured ? t("当前已配置：") + (settings.api_key_hint ?? t("已隐藏")) : <>{t("在")} <a className="text-accent underline underline-offset-2" href="https://console.groq.com" target="_blank" rel="noreferrer">{t("Groq Console")}</a> {t("创建，通常以 gsk_ 开头。")}</>}</p>
            </div>
            <SettingsStatus label={settings.api_key_configured ? t("已配置") : t("未配置")} tone={settings.api_key_configured ? "success" : "warning"} />
          </div>
          <div className="mt-4 max-w-xl">
            <PasswordInput
              id="settings-groq-api-key"
              ariaLabel={t("Groq API Key（访问密钥）")}
              value={apiKeyDraft}
              onChange={(value) => { setApiKeyDraft(value); setValid(null); setApiKeySaveError(null); }}
              placeholder={settings.api_key_configured ? t("留空保持当前密钥…") : "gsk_…"}
              valid={valid === "valid"}
              monospace
            />
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <button type="button" onClick={() => void commitApiKey()} disabled={!apiKeyDraft.trim()} className={buttonClass}>{t("验证并保存")}</button>
              <button type="button" onClick={() => void validate()} disabled={(!apiKeyDraft.trim() && !settings.api_key_configured) || validating} className={secondaryButtonClass}>{validating ? t("验证中…") : t("仅验证")}</button>
              {settings.api_key_configured && <button type="button" onClick={() => setConfirmRemoveKey(true)} disabled={removing} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{removing ? t("删除中…") : t("删除本机密钥")}</button>}
              <ValidationStatus status={valid} validating={validating} />
            </div>
            {apiKeySaveError && <p role="alert" className="mt-3 text-xs leading-5 text-error">{apiKeySaveError}</p>}
            <p className="mt-3 flex items-center gap-1.5 text-xs leading-5 text-tertiary"><KeyRound size={14} aria-hidden="true" />{t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到 Groq。")}</p>
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("ASR 兼容接口")}>
        <SettingsRow
          title={t("ASR 兼容地址")}
          description={t("可选，例如 http://127.0.0.1:8000/v1。须为 OpenAI 兼容的 Whisper /audio/transcriptions 接口。留空则使用 Groq。")}
        >
          <input
            id="settings-asr-base-url"
            aria-label={t("ASR 兼容地址")}
            value={settings.asr_base_url ?? ""}
            onChange={(event) => save({ asr_base_url: event.target.value })}
            placeholder="https://api.groq.com/openai/v1"
            autoComplete="off"
            spellCheck={false}
            className={`${controlClass} w-72 max-w-full font-mono text-xs`}
          />
        </SettingsRow>
        <div className="px-4 py-4 sm:px-5">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <p className="text-sm font-medium text-primary">{t("ASR API Key（可选）")}</p>
              <p className="mt-1 text-xs leading-5 text-tertiary">
                {asrKeyConfigured
                  ? t("当前已配置：") + (settings.asr_api_key_hint ?? t("已隐藏"))
                  : t("留空时，仅默认 Groq 或 api.groq.com 会使用上面的 Groq 密钥；其他地址必须填写 ASR 密钥。")}
              </p>
            </div>
            <SettingsStatus label={asrKeyConfigured ? t("已配置") : t("未配置")} tone={asrKeyConfigured ? "success" : "neutral"} />
          </div>
          <div className="mt-4 max-w-xl">
            <PasswordInput
              id="settings-asr-api-key"
              ariaLabel={t("ASR API Key（可选）")}
              value={asrKeyDraft}
              onChange={(value) => { setAsrKeyDraft(value); setAsrKeySaveError(null); }}
              placeholder={asrKeyConfigured ? t("留空保持当前密钥…") : t("可选，兼容端点密钥")}
              monospace
            />
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <button type="button" onClick={() => void commitAsrApiKey()} disabled={!asrKeyDraft.trim()} className={buttonClass}>{t("保存 ASR 密钥")}</button>
              {asrKeyConfigured && <button type="button" onClick={() => setConfirmRemoveAsrKey(true)} disabled={removingAsrKey} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{removingAsrKey ? t("删除中…") : t("删除 ASR 密钥")}</button>}
            </div>
            {asrKeySaveError && <p role="alert" className="mt-3 text-xs leading-5 text-error">{asrKeySaveError}</p>}
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("文字整理")}>
        <SettingsRow title={t("AI 文字整理")} description={t("自动去掉口头禅、重复和明显语法问题，尽量保留你的原意。") + " " + t("关闭后只使用本地规则，不会请求文字整理服务。")}>
          <Toggle checked={settings.cleanup_enabled} onChange={(checked) => save({ cleanup_enabled: checked })} label={t("AI 文字整理")} />
        </SettingsRow>
        {settings.cleanup_enabled && (
          <SettingsRow title={t("使用模型")} description={`${t("当前服务 · Groq")} · ${modelLabel(settings.cleanup_model)}`}>
            <select id="cleanup-model" aria-label={t("AI 文字整理模型")} value={settings.cleanup_model} onChange={(event) => save({ cleanup_model: event.target.value })} className={`${controlClass} w-52 text-xs`}>
              {cleanupModelOptions.map((option) => <option key={option.value} value={option.value}>{`${option.label} · ${t(option.note)}`}</option>)}
            </select>
          </SettingsRow>
        )}
      </SettingsGroup>
      <ConfirmDialog
        open={confirmRemoveKey}
        title={t("删除本机密钥")}
        description={t("删除后需要重新配置 API Key 才能使用语音输入。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemoveKey(false)}
        onConfirm={() => void commitRemoveApiKey()}
      />
      <ConfirmDialog
        open={confirmRemoveAsrKey}
        title={t("删除 ASR 密钥")}
        description={t("删除后，仅默认 Groq 或 api.groq.com 会改用上面的 Groq 密钥；其他地址需要重新填写 ASR 密钥。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemoveAsrKey(false)}
        onConfirm={() => void commitRemoveAsrApiKey()}
      />
    </SettingsShell>
  );
}
