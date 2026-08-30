import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, KeyRound } from "lucide-react";
import { ConfirmDialog } from "../ConfirmDialog";
import { PasswordInput } from "../PasswordInput";
import { SettingsGroup, SettingsPageHeader, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { useI18n } from "../../lib/i18n";
import {
  draftFromSettings,
  hasProviderSecret,
  persistPatch,
  probePayload,
  providerConfigured,
  providerHint,
  providerOf,
  step2Ready,
  switchProvider,
  type EngineDraft,
} from "../../lib/engineWizard";
import {
  PROVIDERS,
  capabilityLabel,
  defaultModel,
  isKnownModel,
  isProviderId,
  providerById,
  providersFor,
  type ProviderId,
} from "../../lib/providers";
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

function capabilityText(id: ProviderId, draft: EngineDraft, t: (key: string) => string): string {
  const definition = providerById(id);
  const capabilities = id === "custom"
    ? [
        ...(draft.customAsr ? ["asr" as const] : []),
        ...(draft.customLlm ? ["llm" as const] : []),
      ]
    : definition?.capabilities ?? [];
  const kind = capabilityLabel(capabilities);
  if (kind === "both") return `${t("转写")} · ${t("润色")}`;
  if (kind === "asr") return t("转写");
  return t("润色");
}

export function EngineSettings({
  settings,
  save,
  removeProviderKey,
  commitEngine,
}: {
  settings: Settings;
  save: SaveSettings;
  saveApiKey?: (apiKey: string) => Promise<void>;
  removeApiKey?: () => Promise<void>;
  saveAsrApiKey?: (apiKey: string, asrBaseUrl?: string) => Promise<void>;
  removeAsrApiKey?: () => Promise<void>;
  removeCleanupApiKey?: () => Promise<void>;
  removeProviderKey: (provider: string) => Promise<void>;
  commitEngine: (patch: Record<string, unknown>) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<EngineDraft>(() => draftFromSettings(settings));
  const [probing, setProbing] = useState(false);
  const [rowProbing, setRowProbing] = useState<ProviderId | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [probeSuccess, setProbeSuccess] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [stageFail, setStageFail] = useState<{ asr?: string | null; cleanup?: string | null }>({});
  const [expanded, setExpanded] = useState<Set<ProviderId>>(() => {
    const next = new Set<ProviderId>([providerOf(settings.asr_provider, settings.asr_base_url)]);
    if (settings.cleanup_enabled) next.add(providerOf(settings.cleanup_provider, settings.cleanup_base_url));
    return next;
  });
  const [confirmRemove, setConfirmRemove] = useState<ProviderId | null>(null);
  const [removing, setRemoving] = useState(false);
  const rowRefs = useRef<Partial<Record<ProviderId, HTMLDivElement | null>>>({});

  const asrMissing = !hasProviderSecret(draft.asrProvider, draft, settings);
  const cleanupMissing = settings.cleanup_enabled && !hasProviderSecret(draft.cleanupProvider, draft, settings);
  const ready = step2Ready(draft, settings);

  useEffect(() => {
    setExpanded((current) => {
      const next = new Set(current);
      next.add(draft.asrProvider);
      if (settings.cleanup_enabled) next.add(draft.cleanupProvider);
      return next;
    });
  }, [draft.asrProvider, draft.cleanupProvider, settings.cleanup_enabled]);

  useEffect(() => {
    const missing = asrMissing ? draft.asrProvider : cleanupMissing ? draft.cleanupProvider : null;
    if (!missing) return;
    rowRefs.current[missing]?.scrollIntoView?.({ block: "nearest" });
    document.getElementById(`provider-key-${missing}`)?.focus();
  }, [asrMissing, cleanupMissing, draft.asrProvider, draft.cleanupProvider]);

  const routingLive =
    draft.asrProvider === providerOf(settings.asr_provider, settings.asr_base_url)
    && (!settings.cleanup_enabled || draft.cleanupProvider === providerOf(settings.cleanup_provider, settings.cleanup_base_url))
    && draft.customBaseUrl.trim() === (settings.custom_base_url ?? "").trim()
    && draft.ollamaBaseUrl.trim() === (settings.ollama_base_url ?? providerById("ollama")?.defaultBaseUrl ?? "").trim()
    && draft.localWhisperBaseUrl.trim() === (settings.local_whisper_base_url ?? providerById("local_whisper")?.defaultBaseUrl ?? "").trim()
    && !Object.values(draft.providerKeys).some((value) => value?.trim());

  const failKind = (kind: string | null | undefined) => {
    if (kind === "key" || kind === "missing_key") return t("密钥无效");
    if (kind) return probeCopy(kind, undefined, t);
    return null;
  };

  const runProbe = async () => {
    setProbing(true);
    setProbeError(null);
    setProbeSuccess(false);
    setCommitError(null);
    try {
      const result = await invoke<ProbeResult>("probe_engine_draft", { draft: probePayload(draft, settings) });
      if (!result.asr.ok) {
        setStageFail({ asr: result.asr.error_kind, cleanup: undefined });
        setProbeError(`${t("转写")}：${probeCopy(result.asr.error_kind, result.asr.message, t)}`);
        return;
      }
      if (!result.cleanup.ok) {
        setStageFail({ asr: undefined, cleanup: result.cleanup.error_kind });
        setProbeError(`${t("文字整理")}：${probeCopy(result.cleanup.error_kind, result.cleanup.message, t)}`);
        return;
      }
      setStageFail({});
      await commitEngine(persistPatch(draft, settings));
      setDraft((current) => ({ ...current, providerKeys: {} }));
      setProbeSuccess(true);
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setProbing(false);
    }
  };

  const testConnection = async (id: ProviderId) => {
    const definition = providerById(id);
    if (!definition) return;
    setRowProbing(id);
    setCommitError(null);
    try {
      const asrOnly = definition.capabilities.includes("asr");
      const result = await invoke<ProbeResult>("probe_engine_draft", {
        draft: probePayload(draft, settings, asrOnly
          ? { asrProvider: id, cleanupEnabled: false }
          : { cleanupProvider: id, cleanupEnabled: true }),
      });
      const stage = asrOnly ? result.asr : result.cleanup;
      if (!stage.ok) {
        setStageFail((current) => (
          asrOnly ? { ...current, asr: stage.error_kind } : { ...current, cleanup: stage.error_kind }
        ));
        setProbeError(`${definition.label}：${probeCopy(stage.error_kind, stage.message, t)}`);
        return;
      }
      setStageFail((current) => (
        asrOnly ? { ...current, asr: undefined } : { ...current, cleanup: undefined }
      ));
      const typed = draft.providerKeys[id]?.trim();
      if (typed) {
        await commitEngine({ provider_keys: { [id]: typed } });
        setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
      }
      setProbeSuccess(true);
      setProbeError(null);
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setRowProbing(null);
    }
  };

  const removeKey = async (id: ProviderId) => {
    setConfirmRemove(null);
    setRemoving(true);
    try {
      await removeProviderKey(id);
      setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
      setStageFail({});
    } finally {
      setRemoving(false);
    }
  };

  const changeAsrProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "asr", value));
    setProbeSuccess(false);
  };

  const changeCleanupProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "cleanup", value));
    setProbeSuccess(false);
  };

  const changeAsrModel = (asrModel: string) => {
    setDraft({ ...draft, asrModel });
    if (routingLive && isKnownModel(draft.asrProvider, "asr", asrModel)) {
      save({ asr_model: asrModel });
    }
  };

  const changeCleanupModel = (cleanupModel: string) => {
    setDraft({ ...draft, cleanupModel });
    setStageFail((current) => ({ ...current, cleanup: undefined }));
    if (routingLive && isKnownModel(draft.cleanupProvider, "llm", cleanupModel)) {
      save({ cleanup_model: cleanupModel });
    }
  };

  const toggleExpanded = (id: ProviderId) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const asrOptions = providersFor("asr", draft.customAsr, draft.customLlm);
  const cleanupOptions = providersFor("llm", draft.customAsr, draft.customLlm);
  const asrFailLabel = failKind(stageFail.asr);
  const asrStatus = asrMissing
    ? { label: t("未配置密钥"), tone: "warning" as const }
    : asrFailLabel
      ? { label: asrFailLabel, tone: "error" as const }
      : { label: t("已就绪"), tone: "success" as const };
  const cleanupFailLabel = failKind(stageFail.cleanup);
  const cleanupStatus = !settings.cleanup_enabled
    ? { label: t("关闭 · 只用本地规则"), tone: "unused" as const }
    : cleanupMissing
      ? { label: t("未配置密钥"), tone: "warning" as const }
      : cleanupFailLabel
        ? { label: cleanupFailLabel, tone: "error" as const }
        : { label: t("已就绪"), tone: "success" as const };

  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t("语音服务")}
        description={t("上面选择转写和润色用谁，下面管理各服务商密钥。改服务商或密钥后，测试通过才会保存。")}
      />
      <SettingsGroup title={t("正在使用")}>
        <div className="px-4 py-4 sm:px-5">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-primary">{t("转写")}</p>
            <SettingsStatus label={asrStatus.label} tone={asrStatus.tone} />
          </div>
          <div className="mt-3 flex w-full flex-col gap-2 sm:flex-row">
            <select aria-label={t("转写服务")} value={draft.asrProvider} onChange={(event) => changeAsrProvider(event.target.value)} className={`${controlClass} w-full sm:w-40 text-xs`}>
              {asrOptions.map((provider) => (
                <option key={provider.id} value={provider.id}>{provider.id === "custom" ? t("兼容接口") : provider.label}</option>
              ))}
            </select>
            <ModelControl
              id={draft.asrProvider}
              side="asr"
              value={draft.asrModel}
              ariaLabel={t("ASR 模型")}
              onChange={changeAsrModel}
            />
          </div>
          {draft.asrProvider === "groq" && (
            <p className="mt-2 text-xs leading-5 text-tertiary">
              {t("Groq Whisper 英文更快，中文人名和专有名词较弱。中文推荐 SiliconFlow SenseVoice 或兼容接口的 Qwen3-ASR。")}
            </p>
          )}
        </div>
        <div className="px-4 py-4 sm:px-5">
          <div className="flex items-center justify-between gap-3">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <p className="text-sm font-medium text-primary">{t("AI 文字整理")}</p>
              <SettingsStatus label={cleanupStatus.label} tone={cleanupStatus.tone} />
            </div>
            <Toggle checked={settings.cleanup_enabled} onChange={(checked) => save({ cleanup_enabled: checked })} label={t("AI 文字整理")} />
          </div>
          {settings.cleanup_enabled && (
            <div className="mt-3 flex w-full flex-col gap-2 sm:flex-row">
              <select aria-label={t("整理服务")} value={draft.cleanupProvider} onChange={(event) => changeCleanupProvider(event.target.value)} className={`${controlClass} w-full sm:w-40 text-xs`}>
                {cleanupOptions.map((provider) => (
                  <option key={provider.id} value={provider.id}>{provider.id === "custom" ? t("兼容接口") : provider.label}</option>
                ))}
              </select>
              <ModelControl
                id={draft.cleanupProvider}
                side="llm"
                value={draft.cleanupModel}
                ariaLabel={t("AI 文字整理模型")}
                onChange={changeCleanupModel}
              />
            </div>
          )}
        </div>
      </SettingsGroup>
      <div className="mt-3 flex flex-wrap items-center justify-end gap-3">
        {probeError && <p role="alert" className="text-xs leading-5 text-error">{probeError}</p>}
        {probeSuccess && <p role="status" className="text-xs leading-5 text-success">{t("测试通过，设置已保存")}</p>}
        {commitError && <p role="alert" className="text-xs leading-5 text-error">{commitError}</p>}
        <button type="button" onClick={() => void runProbe()} disabled={probing || !ready} className={buttonClass}>
          {probing ? t("测试中…") : t("测试当前配置")}
        </button>
      </div>
      <SettingsGroup title={t("服务商")} description={t("密钥按服务商保存，转写和润色可以共用同一份。")}>
        {PROVIDERS.map((provider) => {
          const inUse = draft.asrProvider === provider.id || (settings.cleanup_enabled && draft.cleanupProvider === provider.id);
          const configured = providerConfigured(settings, provider.id) || Boolean(draft.providerKeys[provider.id]?.trim());
          const open = expanded.has(provider.id);
          const rowKind = [
            draft.asrProvider === provider.id ? stageFail.asr : undefined,
            settings.cleanup_enabled && draft.cleanupProvider === provider.id
              ? stageFail.cleanup
              : undefined,
          ].find((kind) => kind);
          const rowFail = failKind(rowKind);
          const rowTone = rowFail
            ? { label: rowFail, tone: "error" as const }
            : inUse
              ? { label: t("使用中"), tone: "accent" as const }
              : providerConfigured(settings, provider.id)
                ? { label: t("已配置"), tone: "unused" as const }
                : { label: t("未添加"), tone: "unused" as const };
          return (
            <div key={provider.id} ref={(node) => { rowRefs.current[provider.id] = node; }}>
              <div className="flex flex-wrap items-center gap-3 px-4 py-3 sm:px-5">
                <button type="button" className="min-w-0 flex-1 text-left" onClick={() => toggleExpanded(provider.id)}>
                  <p className="text-sm font-medium text-primary">{provider.id === "custom" ? t("兼容接口") : provider.label}</p>
                  <p className="mt-0.5 text-xs text-tertiary">{capabilityText(provider.id, draft, t)}</p>
                </button>
                <SettingsStatus label={rowTone.label} tone={rowTone.tone} />
                {!configured && !open ? (
                  <button type="button" onClick={() => toggleExpanded(provider.id)} className="rounded-lg px-2 py-1 text-xs text-secondary hover:bg-elevated">{t("添加")}</button>
                ) : (
                  <button type="button" aria-label={open ? t("收起") : t("展开")} onClick={() => toggleExpanded(provider.id)} className="rounded-lg p-1 text-tertiary hover:bg-elevated">
                    <ChevronDown size={16} className={open ? "rotate-180" : ""} aria-hidden="true" />
                  </button>
                )}
              </div>
              {open && (
                <div className="space-y-3 px-4 pb-4 sm:px-5">
                  {provider.id === "custom" && (
                    <>
                      <label className="block text-xs text-secondary">
                        {t("兼容地址")}
                        <input
                          aria-label={t("兼容地址")}
                          value={draft.customBaseUrl}
                          onChange={(event) => setDraft({ ...draft, customBaseUrl: event.target.value })}
                          placeholder="https://api.example.com/v1"
                          autoComplete="off"
                          spellCheck={false}
                          className={`${controlClass} mt-1 w-full font-mono text-xs`}
                        />
                      </label>
                      <div className="flex flex-wrap gap-2">
                        <p className="w-full text-xs text-tertiary">{t("中文转写预设")}</p>
                        <button
                          type="button"
                          className="rounded-lg px-2 py-1 text-xs text-secondary hover:bg-elevated"
                          onClick={() => setDraft((current) => ({
                            ...current,
                            asrProvider: "custom",
                            customAsr: true,
                            customBaseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
                            asrModel: "qwen3-asr-flash",
                          }))}
                        >
                          {t("阿里云百炼 Qwen3-ASR")}
                        </button>
                        <button
                          type="button"
                          className="rounded-lg px-2 py-1 text-xs text-secondary hover:bg-elevated"
                          onClick={() => setDraft((current) => ({
                            ...current,
                            asrProvider: "custom",
                            customAsr: true,
                            customBaseUrl: "http://127.0.0.1:10095/v1",
                            asrModel: "paraformer-zh",
                          }))}
                        >
                          {t("本机 FunASR")}
                        </button>
                      </div>
                      <label className="flex items-center gap-2 text-xs text-secondary">
                        <input type="checkbox" checked={draft.customAsr} onChange={(event) => {
                          const customAsr = event.target.checked;
                          setDraft((current) => {
                            const next = { ...current, customAsr };
                            return !customAsr && current.asrProvider === "custom" ? switchProvider(next, "asr", "groq") : next;
                          });
                        }} />
                        {t("用于转写")}
                      </label>
                      <label className="flex items-center gap-2 text-xs text-secondary">
                        <input type="checkbox" checked={draft.customLlm} onChange={(event) => {
                          const customLlm = event.target.checked;
                          setDraft((current) => {
                            const next = { ...current, customLlm };
                            return !customLlm && current.cleanupProvider === "custom" ? switchProvider(next, "cleanup", "groq") : next;
                          });
                        }} />
                        {t("用于润色")}
                      </label>
                    </>
                  )}
                  {provider.editableBaseUrl && provider.id !== "custom" && (
                    <label className="block text-xs text-secondary">
                      {t("本机地址")}
                      <input
                        aria-label={t("本机地址")}
                        value={provider.id === "ollama" ? draft.ollamaBaseUrl : draft.localWhisperBaseUrl}
                        onChange={(event) => setDraft(provider.id === "ollama"
                          ? { ...draft, ollamaBaseUrl: event.target.value }
                          : { ...draft, localWhisperBaseUrl: event.target.value })}
                        placeholder={provider.defaultBaseUrl}
                        autoComplete="off"
                        spellCheck={false}
                        className={`${controlClass} mt-1 w-full font-mono text-xs`}
                      />
                    </label>
                  )}
                  <div>
                    <p className="text-sm font-medium text-primary">{t("API Key")}</p>
                    <SecretField
                      id={`provider-key-${provider.id}`}
                      ariaLabel={`${provider.label} API Key`}
                      typed={draft.providerKeys[provider.id] ?? ""}
                      hint={providerHint(settings, provider.id)}
                      placeholder={provider.allowsEmptyKey ? t("本机可不填") : `${provider.label} key`}
                      onTypedChange={(value) => {
                        setDraft({ ...draft, providerKeys: { ...draft.providerKeys, [provider.id]: value } });
                        setProbeSuccess(false);
                      }}
                    />
                    {provider.allowsEmptyKey && (
                      <p className="mt-2 text-xs leading-5 text-tertiary">{t("本机地址可不填密钥")}</p>
                    )}
                    <p className="mt-2 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
                      <KeyRound size={14} aria-hidden="true" />
                      {t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到所选服务。")}
                    </p>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <button
                        type="button"
                        onClick={() => void testConnection(provider.id)}
                        disabled={rowProbing === provider.id || probing}
                        className="rounded-lg px-3 py-2 text-xs text-secondary transition-colors hover:bg-elevated disabled:opacity-50"
                      >
                        {rowProbing === provider.id ? t("测试中…") : t("测试连接")}
                      </button>
                      {(providerConfigured(settings, provider.id) || draft.providerKeys[provider.id]?.trim()) && (
                        <button
                          type="button"
                          onClick={() => setConfirmRemove(provider.id)}
                          disabled={removing}
                          className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50"
                        >
                          {t("删除密钥")}
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </SettingsGroup>
      <ConfirmDialog
        open={confirmRemove !== null}
        title={t("删除密钥")}
        description={t("删除后需要重新填写这个服务商的密钥。确定删除吗？")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemove(null)}
        onConfirm={() => confirmRemove && void removeKey(confirmRemove)}
      />
    </SettingsShell>
  );
}

function ModelControl({
  id,
  side,
  value,
  ariaLabel,
  onChange,
}: {
  id: ProviderId;
  side: "asr" | "llm";
  value: string;
  ariaLabel: string;
  onChange: (value: string) => void;
}) {
  const { t } = useI18n();
  const definition = providerById(id);
  if (!definition) return null;
  const field = side === "asr" ? definition.asrModelField : definition.llmModelField;
  const options = side === "asr" ? definition.asrModels : definition.llmModels;
  if (field === "select" && options.length > 0) {
    return (
      <select aria-label={ariaLabel} value={value} onChange={(event) => onChange(event.target.value)} className={`${controlClass} w-full sm:w-56 text-xs`}>
        {options.map((option) => (
          <option key={option.value} value={option.value}>{option.note ? `${option.label} · ${t(option.note)}` : option.label}</option>
        ))}
      </select>
    );
  }
  const listId = `${id}-${side}-models`;
  return (
    <>
      <input
        aria-label={ariaLabel}
        value={value}
        list={options.length ? listId : undefined}
        onChange={(event) => onChange(event.target.value)}
        placeholder={defaultModel(id, side === "asr" ? "asr" : "llm") || "model"}
        autoComplete="off"
        spellCheck={false}
        className={`${controlClass} w-full sm:w-56 font-mono text-xs`}
      />
      {options.length > 0 && (
        <datalist id={listId}>
          {options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </datalist>
      )}
    </>
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
