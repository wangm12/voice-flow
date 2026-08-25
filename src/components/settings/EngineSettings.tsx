import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { KeyRound } from "lucide-react";
import { ConfirmDialog } from "../ConfirmDialog";
import { PasswordInput } from "../PasswordInput";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { useI18n } from "../../lib/i18n";
import {
  GROQ_ASR_MODELS,
  GROQ_CLEANUP_MODELS,
  draftFromSettings,
  maskSecret,
  modelLabel,
  persistPatch,
  probePayload,
  step2Ready,
  switchProvider,
  type EngineDraft,
  type EngineProvider,
} from "../../lib/engineWizard";
import { friendlySettingsError } from "../../lib/settingsError";
import { buttonClass, colors, focusRingClass, radius } from "../../lib/theme";
import type { SaveSettings, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;

type ProbeStage = {
  ok: boolean;
  skipped: boolean;
  error_kind?: string | null;
  message?: string;
};

type ProbeResult = {
  asr: ProbeStage;
  cleanup: ProbeStage;
};

function probeCopy(kind: string | null | undefined, message: string | undefined, t: (key: string) => string): string {
  if (kind === "address") return t("地址连不上");
  if (kind === "key") return t("密钥无效");
  if (kind === "model") return t("模型名不被这个接口接受");
  if (kind === "path") return t("地址路径不对");
  if (kind === "missing_key") return t("缺少密钥");
  if (kind === "provider") return t("服务返回错误");
  return message?.trim() || t("服务返回错误");
}

export function EngineSettings({
  settings,
  save,
  removeApiKey,
  removeAsrApiKey,
  removeCleanupApiKey,
  commitEngine,
}: {
  settings: Settings;
  save: SaveSettings;
  saveApiKey?: (apiKey: string) => Promise<void>;
  removeApiKey: () => Promise<void>;
  saveAsrApiKey?: (apiKey: string, asrBaseUrl?: string) => Promise<void>;
  removeAsrApiKey: () => Promise<void>;
  removeCleanupApiKey: () => Promise<void>;
  commitEngine: (patch: Record<string, string>) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<EngineDraft>(() => draftFromSettings(settings));
  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [probeSuccess, setProbeSuccess] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [confirmRemoveKey, setConfirmRemoveKey] = useState<"groq" | "asr" | "cleanup" | null>(null);
  const [hints, setHints] = useState({
    groq: settings.api_key_hint ?? "",
    asr: settings.asr_api_key_hint ?? "",
    cleanup: settings.cleanup_api_key_hint ?? "",
  });

  const runProbe = async () => {
    setProbing(true);
    setProbeError(null);
    setProbeSuccess(false);
    setCommitError(null);
    try {
      const result = await invoke<ProbeResult>("probe_engine_draft", { draft: probePayload(draft, settings) });
      if (!result.asr.ok) {
        setProbeError(`${t("转写")}：${probeCopy(result.asr.error_kind, result.asr.message, t)}`);
        return;
      }
      if (!result.cleanup.ok) {
        setProbeError(`${t("文字整理")}：${probeCopy(result.cleanup.error_kind, result.cleanup.message, t)}`);
        return;
      }
      await commitEngine(persistPatch(draft, settings));
      setHints((current) => ({
        groq: draft.apiKey.trim() ? maskSecret(draft.apiKey) : current.groq,
        asr: draft.asrApiKey.trim() ? maskSecret(draft.asrApiKey) : current.asr,
        cleanup: draft.cleanupApiKey.trim() ? maskSecret(draft.cleanupApiKey) : current.cleanup,
      }));
      setDraft((current) => ({ ...current, apiKey: "", asrApiKey: "", cleanupApiKey: "" }));
      setProbeSuccess(true);
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setProbing(false);
    }
  };

  const removeKey = async (kind: "groq" | "asr" | "cleanup") => {
    setConfirmRemoveKey(null);
    setRemoving(true);
    try {
      if (kind === "groq") await removeApiKey();
      else if (kind === "asr") await removeAsrApiKey();
      else await removeCleanupApiKey();
    } finally {
      setRemoving(false);
    }
  };

  const showGroqKey = draft.asrProvider === "groq" || (settings.cleanup_enabled && draft.cleanupProvider === "groq");
  const showCustomCleanupKey = settings.cleanup_enabled && draft.cleanupProvider === "custom";
  const probe = (
    <ProbeActions
      probing={probing}
      ready={step2Ready(draft, settings)}
      probeError={probeError}
      probeSuccess={probeSuccess}
      commitError={commitError}
      onProbe={() => void runProbe()}
      t={t}
    />
  );

  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t("语音服务")}
        description={t("选择转写和整理服务，测试通过后才会写入设置。")}
      />
      <SettingsGroup title={t("服务")}>
        <ProviderRow
          title={t("转写服务")}
          value={draft.asrProvider}
          onChange={(value) => setDraft(switchProvider(draft, "asr", value))}
          t={t}
        />
        {settings.cleanup_enabled && (
          <ProviderRow
            title={t("整理服务")}
            value={draft.cleanupProvider}
            onChange={(value) => setDraft(switchProvider(draft, "cleanup", value))}
            t={t}
          />
        )}
      </SettingsGroup>
      <SettingsGroup title={t("密钥和模型")}>
        {showGroqKey && (
          <div className="px-4 py-4 sm:px-5">
            <p className="text-sm font-medium text-primary">{t("Groq API Key")}</p>
            <SecretField
              id="wizard-groq-api-key"
              ariaLabel={t("Groq API Key（访问密钥）")}
              typed={draft.apiKey}
              hint={hints.groq}
              placeholder="gsk_…"
              onTypedChange={(apiKey) => setDraft({ ...draft, apiKey })}
            />
            {probe}
            <p className="mt-3 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
              <KeyRound size={14} aria-hidden="true" />
              {t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到所选服务。")}
            </p>
          </div>
        )}
        {draft.asrProvider === "groq" ? (
          <SettingsRow title={t("ASR 模型")} description={t("Groq Whisper 模型。")}>
            <select
              aria-label={t("ASR 模型")}
              value={draft.asrModel}
              onChange={(event) => {
                const asrModel = event.target.value;
                setDraft({ ...draft, asrModel });
                save({ asr_model: asrModel });
              }}
              className={`${controlClass} w-72 max-w-full text-xs`}
            >
              {GROQ_ASR_MODELS.map((option) => (
                <option key={option.value} value={option.value}>{`${option.label} · ${t(option.note)}`}</option>
              ))}
            </select>
          </SettingsRow>
        ) : (
          <>
            <SettingsRow title={t("ASR 兼容地址")} description={t("须为 OpenAI 兼容的 Whisper /audio/transcriptions 接口。")}>
              <input
                aria-label={t("ASR 兼容地址")}
                value={draft.asrBaseUrl}
                onChange={(event) => setDraft({ ...draft, asrBaseUrl: event.target.value })}
                placeholder="http://127.0.0.1:8000/v1"
                autoComplete="off"
                spellCheck={false}
                className={`${controlClass} w-72 max-w-full font-mono text-xs`}
              />
            </SettingsRow>
            <SettingsRow title={t("ASR 模型名")} description={t("填写这个接口使用的模型名，例如 whisper-1。")}>
              <input
                aria-label={t("ASR 模型名")}
                value={draft.asrModel}
                onChange={(event) => setDraft({ ...draft, asrModel: event.target.value })}
                placeholder="whisper-1"
                autoComplete="off"
                spellCheck={false}
                className={`${controlClass} w-72 max-w-full font-mono text-xs`}
              />
            </SettingsRow>
            <div className="px-4 py-4 sm:px-5">
              <p className="text-sm font-medium text-primary">{t("ASR API Key")}</p>
              <SecretField
                id="wizard-asr-api-key"
                ariaLabel={t("ASR API Key")}
                typed={draft.asrApiKey}
                hint={hints.asr}
                placeholder={t("兼容端点密钥")}
                onTypedChange={(asrApiKey) => setDraft({ ...draft, asrApiKey })}
              />
              {!showGroqKey && !showCustomCleanupKey && probe}
            </div>
          </>
        )}
        {settings.cleanup_enabled && draft.cleanupProvider === "groq" && (
          <SettingsRow title={t("使用模型")} description={`${t("当前服务 · Groq")} · ${modelLabel(draft.cleanupModel)}`}>
            <select
              aria-label={t("AI 文字整理模型")}
              value={draft.cleanupModel}
              onChange={(event) => {
                const cleanupModel = event.target.value;
                setDraft({ ...draft, cleanupModel });
                save({ cleanup_model: cleanupModel });
              }}
              className={`${controlClass} w-52 text-xs`}
            >
              {GROQ_CLEANUP_MODELS.map((option) => (
                <option key={option.value} value={option.value}>{`${option.label} · ${t(option.note)}`}</option>
              ))}
            </select>
          </SettingsRow>
        )}
        {settings.cleanup_enabled && draft.cleanupProvider === "custom" && (
          <>
            <SettingsRow title={t("整理兼容地址")} description={t("须为 OpenAI 兼容的 /v1 或 chat/completions 接口。")}>
              <input
                aria-label={t("整理兼容地址")}
                value={draft.cleanupBaseUrl}
                onChange={(event) => setDraft({ ...draft, cleanupBaseUrl: event.target.value })}
                placeholder="https://api.openai.com/v1"
                autoComplete="off"
                spellCheck={false}
                className={`${controlClass} w-72 max-w-full font-mono text-xs`}
              />
            </SettingsRow>
            <SettingsRow title={t("整理模型名")} description={t("填写这个接口使用的模型名。")}>
              <input
                aria-label={t("整理模型名")}
                value={draft.cleanupModel}
                onChange={(event) => setDraft({ ...draft, cleanupModel: event.target.value })}
                placeholder="gpt-4o-mini"
                autoComplete="off"
                spellCheck={false}
                className={`${controlClass} w-72 max-w-full font-mono text-xs`}
              />
            </SettingsRow>
            <div className="px-4 py-4 sm:px-5">
              <p className="text-sm font-medium text-primary">{t("整理 API Key")}</p>
              <SecretField
                id="wizard-cleanup-api-key"
                ariaLabel={t("整理 API Key")}
                typed={draft.cleanupApiKey}
                hint={hints.cleanup}
                placeholder={t("兼容端点密钥")}
                onTypedChange={(cleanupApiKey) => setDraft({ ...draft, cleanupApiKey })}
              />
              {!showGroqKey && probe}
            </div>
          </>
        )}
      </SettingsGroup>
      <SettingsGroup title={t("AI 文字整理")}>
        <SettingsRow title={t("AI 文字整理")} description={t("自动去掉口头禅、重复和明显语法问题，尽量保留你的原意。") + " " + t("关闭后只使用本地规则，不会请求文字整理服务。")}>
          <Toggle checked={settings.cleanup_enabled} onChange={(checked) => save({ cleanup_enabled: checked })} label={t("AI 文字整理")} />
        </SettingsRow>
      </SettingsGroup>
      <div className="mt-6 flex flex-wrap gap-2">
        {settings.api_key_configured && (
          <button type="button" onClick={() => setConfirmRemoveKey("groq")} disabled={removing} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{t("删除本机密钥")}</button>
        )}
        {settings.asr_api_key_configured && (
          <button type="button" onClick={() => setConfirmRemoveKey("asr")} disabled={removing} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{t("删除 ASR 密钥")}</button>
        )}
        {settings.cleanup_api_key_configured && (
          <button type="button" onClick={() => setConfirmRemoveKey("cleanup")} disabled={removing} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{t("删除整理密钥")}</button>
        )}
      </div>
      <ConfirmDialog
        open={confirmRemoveKey === "groq"}
        title={t("删除本机密钥")}
        description={t("删除后需要重新配置 API Key 才能使用语音输入。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemoveKey(null)}
        onConfirm={() => void removeKey("groq")}
      />
      <ConfirmDialog
        open={confirmRemoveKey === "asr"}
        title={t("删除 ASR 密钥")}
        description={t("删除后，仅默认 Groq 或 api.groq.com 会改用上面的 Groq 密钥；其他地址需要重新填写 ASR 密钥。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemoveKey(null)}
        onConfirm={() => void removeKey("asr")}
      />
      <ConfirmDialog
        open={confirmRemoveKey === "cleanup"}
        title={t("删除整理密钥")}
        description={t("删除后，自定义整理需要重新填写密钥。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemoveKey(null)}
        onConfirm={() => void removeKey("cleanup")}
      />
    </SettingsShell>
  );
}

function SecretField({
  id,
  ariaLabel,
  typed,
  hint,
  placeholder,
  onTypedChange,
}: {
  id: string;
  ariaLabel: string;
  typed: string;
  hint: string;
  placeholder: string;
  onTypedChange: (value: string) => void;
}) {
  const showingHint = !typed && Boolean(hint);
  return (
    <PasswordInput
      id={id}
      ariaLabel={ariaLabel}
      value={typed || hint}
      onChange={(value) => {
        if (showingHint && value === hint) return;
        onTypedChange(value === hint ? "" : value);
      }}
      placeholder={placeholder}
      monospace
      plain={showingHint}
      className="mt-2"
    />
  );
}

function ProbeActions({
  probing,
  ready,
  probeError,
  probeSuccess,
  commitError,
  onProbe,
  t,
}: {
  probing: boolean;
  ready: boolean;
  probeError: string | null;
  probeSuccess: boolean;
  commitError: string | null;
  onProbe: () => void;
  t: (key: string) => string;
}) {
  return (
    <div className="mt-3">
      {probeError && <p role="alert" className="mb-2 text-xs leading-5 text-error">{probeError}</p>}
      {probeSuccess && <p role="status" className="mb-2 text-xs leading-5 text-success">{t("测试通过，设置已保存")}</p>}
      {commitError && <p role="alert" className="mb-2 text-xs leading-5 text-error">{commitError}</p>}
      <button type="button" onClick={onProbe} disabled={probing || !ready} className={buttonClass}>
        {probing ? t("测试中…") : t("开始测试")}
      </button>
    </div>
  );
}

function ProviderRow({
  title,
  value,
  onChange,
  t,
}: {
  title: string;
  value: EngineProvider;
  onChange: (value: EngineProvider) => void;
  t: (key: string) => string;
}) {
  return (
    <SettingsRow title={title} description={t("Groq 使用内置地址和下拉模型；其他服务需要自己填写。")}>
      <select
        aria-label={title}
        value={value}
        onChange={(event) => onChange(event.target.value as EngineProvider)}
        className={`${controlClass} w-40 text-xs`}
      >
        <option value="groq">Groq</option>
        <option value="custom">{t("其他")}</option>
      </select>
    </SettingsRow>
  );
}
