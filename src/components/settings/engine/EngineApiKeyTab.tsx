import type { Dispatch, SetStateAction } from "react";
import { KeyRound } from "lucide-react";
import { SettingsGroup, SettingsRow, SettingsStatus } from "../../SettingsLayout";
import { useI18n } from "../../../lib/i18n";
import { customHostChanged, hasProviderSecret, providerConfigured, providerHint, type EngineDraft } from "../../../lib/engineWizard";
import { PROVIDERS, isLoopbackUrl, type ProviderId } from "../../../lib/providers";
import { compactButtonClass, compactDangerButtonClass } from "../../../lib/theme";
import type { CredentialReadState, ProviderCredentialPurpose, ProviderCredentialReadResult, Settings } from "../../../types/settings";
import { SecretField } from "./EngineControls";
import { capabilityText, providerHasCredentialInput, type EnginePage } from "./engineHelpers";

export type EngineApiKeyTabProps = {
  page: EnginePage;
  settings: Settings;
  draft: EngineDraft;
  savedDraft: EngineDraft;
  setDraft: Dispatch<SetStateAction<EngineDraft>>;
  draftCleanupEnabled: boolean;
  currentAsrProvider: ProviderId;
  currentCleanupProvider: ProviderId;
  stageFail: { asr?: string | null; cleanup?: string | null };
  setStageFail: Dispatch<SetStateAction<{ asr?: string | null; cleanup?: string | null }>>;
  failKind: (kind: string | null | undefined) => string | null;
  expanded: Set<ProviderId>;
  setExpanded: Dispatch<SetStateAction<Set<ProviderId>>>;
  onToggleExpanded: (id: ProviderId) => void;
  rowProbeFeedback: { provider: ProviderId; stage?: "asr" | "cleanup"; tone: "success" | "error"; message: string } | null;
  onClearRowProbeFeedback: () => void;
  onClearProbeSuccess: () => void;
  onResetTestedAsrRoute: () => void;
  onResetTestedCleanupRoute: () => void;
  testedAsrRoute: string | null;
  testedCleanupRoute: string | null;
  asrRoute: string;
  cleanupRoute: string;
  onDeviceReady: boolean;
  probing: boolean;
  rowProbing: ProviderId | null;
  rowProbingStage: "asr" | "cleanup" | null;
  removing: boolean;
  strictOfflineSaving: boolean;
  strictOfflineBlocksProbe: (provider: ProviderId, stage: "asr" | "cleanup") => boolean;
  customParametersOk: boolean;
  customRecoveryPurpose: ProviderCredentialPurpose | null;
  customReadState: CredentialReadState | undefined;
  groqReadState: CredentialReadState | undefined;
  customLatestReadStatus: ProviderCredentialReadResult["status"] | null;
  groqLatestReadStatus: ProviderCredentialReadResult["status"] | null;
  credentialReadPending: boolean;
  credentialReadTarget: ProviderId | null;
  credentialReadMessage: { tone: "success" | "status" | "error"; text: string } | null;
  hasReadProviderCredential: boolean;
  onReadCredential: (provider: ProviderId, purpose?: ProviderCredentialPurpose) => void;
  onRequestRemoveKey: (provider: ProviderId) => void;
  onTestConnection: (id: ProviderId, stageScope: "asr" | "cleanup") => void;
  onNavigateToPage: (nextPage: EnginePage, targetId?: string) => void;
};

export function EngineApiKeyTab({
  page,
  settings,
  draft,
  savedDraft,
  setDraft,
  draftCleanupEnabled,
  currentAsrProvider,
  currentCleanupProvider,
  stageFail,
  setStageFail,
  failKind,
  expanded,
  setExpanded,
  onToggleExpanded,
  rowProbeFeedback,
  onClearRowProbeFeedback,
  onClearProbeSuccess,
  onResetTestedAsrRoute,
  onResetTestedCleanupRoute,
  testedAsrRoute,
  testedCleanupRoute,
  asrRoute,
  cleanupRoute,
  onDeviceReady,
  probing,
  rowProbing,
  rowProbingStage,
  removing,
  strictOfflineSaving,
  strictOfflineBlocksProbe,
  customParametersOk,
  customRecoveryPurpose,
  customReadState,
  groqReadState,
  customLatestReadStatus,
  groqLatestReadStatus,
  credentialReadPending,
  credentialReadTarget,
  credentialReadMessage,
  hasReadProviderCredential,
  onReadCredential,
  onRequestRemoveKey,
  onTestConnection,
  onNavigateToPage,
}: EngineApiKeyTabProps) {
  const { t } = useI18n();

  const renderProvider = (provider: typeof PROVIDERS[number]) => {
    const inUse = currentAsrProvider === provider.id || (settings.cleanup_enabled && currentCleanupProvider === provider.id);
    const inDraft = draft.asrProvider === provider.id || (draftCleanupEnabled && draft.cleanupProvider === provider.id);
    const configured = providerConfigured(settings, provider.id) || Boolean(draft.providerKeys[provider.id]?.trim());
    const requestedOpen = expanded.has(provider.id);
    const customAsrActive = draft.asrProvider === "custom";
    const customCleanupActive = draftCleanupEnabled && draft.cleanupProvider === "custom";
    const customProviderTitle = customAsrActive && customCleanupActive
      ? t("自定义服务")
      : customCleanupActive
        ? t("自定义 AI 整理")
        : customAsrActive
          ? t("自定义语音转写")
          : t("自定义服务");
    const rowKind = [
      draft.asrProvider === provider.id ? stageFail.asr : undefined,
      draftCleanupEnabled && draft.cleanupProvider === provider.id
        ? stageFail.cleanup
        : undefined,
    ].find((kind) => kind);
    const rowFail = failKind(rowKind);
    const rowError = Boolean(rowFail) || (rowProbeFeedback?.provider === provider.id && rowProbeFeedback.tone === "error");
    const open = requestedOpen || rowError;
    const providerProbeStages: Array<"asr" | "cleanup"> = provider.id === "soniox" ? [] : [
      ...(provider.capabilities.includes("asr") ? ["asr" as const] : []),
      ...(provider.capabilities.includes("llm") ? ["cleanup" as const] : []),
    ];
    const groqCredentialStatus = draft.providerKeys.groq?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : credentialReadPending && credentialReadTarget === "groq"
        ? { label: t("正在读取"), tone: "warning" as const }
        : groqLatestReadStatus === "missing"
          ? providerConfigured(settings, "groq")
            ? { label: t("本次会话有凭据 · 钥匙串中未找到密钥"), tone: "warning" as const }
            : { label: t("未添加"), tone: "unused" as const }
          : groqLatestReadStatus === null && groqReadState === "unavailable"
            ? providerConfigured(settings, "groq")
              ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
              : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
            : groqLatestReadStatus === null && groqReadState === "missing"
              ? { label: t("未添加"), tone: "unused" as const }
              : groqLatestReadStatus === null
                ? { label: t("未读取"), tone: "neutral" as const }
                : { label: t("待确认"), tone: "neutral" as const };
    const customCredentialStatus = draft.providerKeys.custom?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : credentialReadPending && credentialReadTarget === "custom"
        ? { label: t("正在读取"), tone: "warning" as const }
        : customLatestReadStatus === "missing"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串中未找到密钥"), tone: "warning" as const }
            : { label: t("未添加"), tone: "unused" as const }
        : customLatestReadStatus === "unavailable"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
            : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
        : customReadState === "unavailable"
          ? providerConfigured(settings, "custom")
            ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
            : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
          : customReadState === "missing"
            ? { label: t("未添加"), tone: "unused" as const }
            : { label: t("待确认"), tone: "neutral" as const };
    const otherKeychainState = provider.id !== "groq" && provider.id !== "custom" && !provider.allowsEmptyKey
      ? settings.provider_keys?.[provider.id]?.keychain_state
      : undefined;
    const rowTone = rowFail
      ? { label: rowFail, tone: "error" as const }
      : provider.id === "groq" && !providerConfigured(settings, "groq")
        ? groqCredentialStatus
      : provider.id === "groq" && groqReadState === "unavailable" && providerConfigured(settings, "groq")
        ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
      : provider.id === "custom" && customRecoveryPurpose !== null && customReadState !== "configured"
        ? customCredentialStatus
      : otherKeychainState !== undefined && credentialReadPending && credentialReadTarget === provider.id
        ? { label: t("正在读取"), tone: "warning" as const }
      : otherKeychainState === "unavailable"
        ? providerConfigured(settings, provider.id)
          ? { label: t("本次会话有凭据 · 钥匙串暂时不可读取"), tone: "warning" as const }
          : { label: t("钥匙串暂时不可读取"), tone: "warning" as const }
      : inUse
        ? { label: t("使用中"), tone: "accent" as const }
        : inDraft
          ? { label: t("待应用"), tone: "warning" as const }
        : providerConfigured(settings, provider.id)
          ? { label: t("已配置"), tone: "unused" as const }
          : { label: t("未添加"), tone: "unused" as const };
    const hasTypedKey = Boolean(draft.providerKeys[provider.id]?.trim());
    const rowCredentialStatus = inUse || inDraft || hasTypedKey
      ? (() => {
          if (!providerHasCredentialInput(provider.id)) return null;
          const credentialDraft = inDraft || hasTypedKey ? draft : savedDraft;
          if (hasTypedKey) {
            return { label: t("密钥待保存"), tone: "warning" as const };
          }
          if (provider.id === "groq" || (provider.id === "custom" && customRecoveryPurpose !== null)) return null;
          if (provider.id === "custom" && customHostChanged(credentialDraft, settings)) {
            return { label: t("未配置密钥"), tone: "warning" as const };
          }
          if (!hasProviderSecret(provider.id, credentialDraft, settings, onDeviceReady)) {
            return { label: t("未配置密钥"), tone: "warning" as const };
          }
          if (provider.id === "soniox") {
            return { label: t("凭据已保存 · 听写时连接"), tone: "neutral" as const };
          }
          const testedForCurrentStage = provider.id === draft.asrProvider && testedAsrRoute === asrRoute
            || draftCleanupEnabled && provider.id === draft.cleanupProvider && testedCleanupRoute === cleanupRoute;
          return providerConfigured(settings, provider.id)
            ? { label: t(testedForCurrentStage ? "已配置" : "凭据已保存 · 本次尚未检查"), tone: "neutral" as const }
            : { label: t("本机服务尚未检查"), tone: "neutral" as const };
        })()
      : null;
    const distinctCredentialStatus = rowCredentialStatus?.label === rowTone.label ? null : rowCredentialStatus;
    const providerTitle = provider.id === "custom" ? customProviderTitle : provider.label;
    return (
      <div key={provider.id} data-provider={provider.id} data-selected={inDraft} className="vf-provider-row">
        <SettingsRow title={providerTitle} description={capabilityText(provider.id, t)}>
          <SettingsStatus label={rowTone.label} tone={rowTone.tone} />
          {distinctCredentialStatus && <SettingsStatus label={distinctCredentialStatus.label} tone={distinctCredentialStatus.tone} />}
          {provider.id === "groq"
            && (!providerConfigured(settings, "groq") || groqReadState === "unavailable")
            && !draft.providerKeys.groq?.trim()
            && hasReadProviderCredential && (
            <button
              type="button"
              onClick={() => {
                setExpanded((current) => new Set([...current, "groq"]));
                onReadCredential("groq");
              }}
              aria-busy={credentialReadPending && credentialReadTarget === "groq"}
              disabled={credentialReadPending || rowProbing !== null || probing || removing}
              className={compactButtonClass}
            >
              {credentialReadPending && credentialReadTarget === "groq" ? t("正在请求钥匙串授权…") : t("从钥匙串重新读取")}
            </button>
          )}
          {provider.id === "custom"
            && customRecoveryPurpose !== null
            && (!providerConfigured(settings, "custom") || customReadState === "unavailable")
            && !draft.providerKeys.custom?.trim()
            && hasReadProviderCredential && (
            <button
              type="button"
              onClick={() => {
                setExpanded((current) => new Set([...current, "custom"]));
                onReadCredential("custom", customRecoveryPurpose);
              }}
              aria-busy={credentialReadPending && credentialReadTarget === "custom"}
              disabled={credentialReadPending || rowProbing !== null || probing || removing}
              className={compactButtonClass}
            >
              {credentialReadPending && credentialReadTarget === "custom" ? t("正在请求钥匙串授权…") : t("从钥匙串重新读取")}
            </button>
          )}
          {provider.id !== "groq" && provider.id !== "custom"
            && !provider.allowsEmptyKey
            && (!providerConfigured(settings, provider.id)
              || settings.provider_keys?.[provider.id]?.keychain_state === "unavailable")
            && !draft.providerKeys[provider.id]?.trim()
            && hasReadProviderCredential && (
            <button
              type="button"
              onClick={() => {
                setExpanded((current) => new Set([...current, provider.id]));
                onReadCredential(provider.id);
              }}
              aria-busy={credentialReadPending && credentialReadTarget === provider.id}
              disabled={credentialReadPending || rowProbing !== null || probing || removing}
              className={compactButtonClass}
            >
              {credentialReadPending && credentialReadTarget === provider.id
                ? t("正在请求钥匙串授权…")
                : t("从钥匙串重新读取")}
            </button>
          )}
          <button type="button" aria-label={`${open ? t("收起") : configured ? t("编辑") : t("添加")} ${providerTitle}`} aria-expanded={open} aria-controls={`provider-details-${provider.id}`} onClick={() => onToggleExpanded(provider.id)} className={compactButtonClass}>
            {open ? t("收起") : configured ? t("编辑") : t("添加")}
          </button>
        </SettingsRow>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} id={`provider-details-${provider.id}`} hidden={!open} inert={!open} className="min-w-0 space-y-3 px-5 pb-4">
          {provider.id === "custom" && (
            <div>
              <p className="text-sm font-medium text-primary">{t("API Key")}</p>
              <SecretField
                id="provider-key-custom"
                ariaLabel={t("自定义服务 API Key")}
                typed={draft.providerKeys.custom ?? ""}
                hint={customHostChanged(draft, settings) ? "" : providerHint(settings, "custom")}
                placeholder={!isLoopbackUrl(draft.customBaseUrl) ? t("远程服务必填") : t("本机可不填")}
                onTypedChange={(value) => {
                  setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, custom: value } }));
                  onClearRowProbeFeedback();
                  onClearProbeSuccess();
                  onResetTestedAsrRoute();
                  onResetTestedCleanupRoute();
                }}
              />
              <p className="mt-2 text-xs leading-5 text-tertiary">{t("凭据绑定的自定义服务地址")}: <code className="break-all font-mono">{draft.customBaseUrl || t("尚未设置")}</code></p>
              <button type="button" className={`${compactButtonClass} mt-2`} onClick={() => onNavigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>
              <p className="mt-2 text-xs leading-5 text-tertiary">{t("远程服务需要填写服务商或 Router 提供的 API Key；只有这台 Mac 上的本机服务可不填。")}</p>
              <p className="mt-2 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
                <KeyRound size={14} aria-hidden="true" />
                {t("自定义 API Key 会在保存时写入钥匙串；明确测试时只发送到此处显示的服务地址。")}
              </p>
              {customHostChanged(draft, settings) && !draft.providerKeys.custom?.trim() && !isLoopbackUrl(draft.customBaseUrl) && (
                <p role="status" className="mt-2 text-xs text-warning-ink">{t("服务地址目标已更改；请填写新地址对应的 API Key。")}</p>
              )}
              {providerConfigured(settings, "custom") && (
                <div className="mt-3 flex flex-wrap gap-2">
                  <button type="button" onClick={() => onRequestRemoveKey("custom")} disabled={removing || rowProbing !== null || probing} className={compactDangerButtonClass}>{t("删除密钥")}</button>
                </div>
              )}
              {rowProbeFeedback?.provider === "custom" && (
                <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={`mt-2 break-words text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}>
                  {rowProbeFeedback.message}
                </p>
              )}
              {credentialReadTarget === "custom" && credentialReadMessage && (
                <p role={credentialReadMessage.tone === "error" ? "alert" : "status"} aria-live="polite" className={`mt-2 break-words text-xs leading-5 ${credentialReadMessage.tone === "error" ? "text-error-ink" : credentialReadMessage.tone === "success" ? "text-success-ink" : "text-secondary"}`}>
                  {credentialReadMessage.text}
                </p>
              )}
              <div className="mt-3 flex flex-wrap gap-2">
                {providerProbeStages.map((stage) => {
                  const busy = rowProbing === "custom" && rowProbingStage === stage;
                  return (
                    <button key={stage} type="button" onClick={() => onTestConnection("custom", stage)} aria-busy={busy} disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe("custom", stage) || (stage === "cleanup" && !customParametersOk)} className={`${compactButtonClass} min-w-36`}>
                      {busy ? t("测试中…") : t(stage === "asr" ? "测试转写" : "测试整理")}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
          {provider.id !== "custom" && (
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
                  onClearRowProbeFeedback();
                  onClearProbeSuccess();
                  setStageFail((current) => ({
                    ...current,
                    asr: provider.id === draft.asrProvider ? null : current.asr,
                    cleanup: provider.id === draft.cleanupProvider ? null : current.cleanup,
                  }));
                  if (provider.id === draft.asrProvider) onResetTestedAsrRoute();
                  if (provider.id === draft.cleanupProvider) onResetTestedCleanupRoute();
                }}
              />
              {provider.allowsEmptyKey && (
                <p className="mt-2 text-xs leading-5 text-tertiary">{t("本机地址可不填密钥")}</p>
              )}
              <p className="mt-2 flex items-center gap-1.5 text-xs leading-5 text-tertiary">
                <KeyRound size={14} aria-hidden="true" />
                {provider.id === "soniox"
                  ? t("Soniox 密钥仅保存在钥匙串中；设置时不会连接服务，实时连接只在开始听写时建立。")
                  : t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到所选服务。")}
              </p>
              {provider.id !== "groq"
                && credentialReadTarget === provider.id
                && credentialReadMessage && (
                <p
                  role={credentialReadMessage.tone === "error" ? "alert" : "status"}
                  aria-live="polite"
                  className={`mt-2 break-words text-xs leading-5 ${credentialReadMessage.tone === "error" ? "text-error-ink" : credentialReadMessage.tone === "success" ? "text-success-ink" : "text-secondary"}`}
                >
                  {credentialReadMessage.text}
                </p>
              )}
              <div className="mt-3 flex flex-wrap gap-2">
                {provider.id === "soniox" ? (
                  <p className="text-xs leading-5 text-tertiary">{t("实时连接在听写时建立；API Key 通过下方共用操作保存。")}</p>
                ) : (
                  providerProbeStages.map((stage) => {
                    const busy = rowProbing === provider.id && rowProbingStage === stage;
                    return (
                      <button
                        key={stage}
                        type="button"
                        onClick={() => onTestConnection(provider.id, stage)}
                        aria-busy={busy}
                        disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(provider.id, stage)}
                        className={`${compactButtonClass} min-w-36`}
                      >
                        {busy ? t("测试中…") : t(stage === "asr" ? "测试转写" : "测试整理")}
                      </button>
                    );
                  })
                )}
                {providerConfigured(settings, provider.id) && (
                  <button
                    type="button"
                    onClick={() => onRequestRemoveKey(provider.id)}
                    disabled={removing || rowProbing !== null || probing}
                    className={compactDangerButtonClass}
                  >
                    {t("删除密钥")}
                  </button>
                )}
              </div>
              {rowProbeFeedback?.provider === provider.id && (
                <p
                  role={rowProbeFeedback.tone === "error" ? "alert" : "status"}
                  className={`mt-2 break-words text-xs leading-5 ${rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}`}
                >
                  {rowProbeFeedback.message}
                </p>
              )}
              {provider.id === "groq"
                && credentialReadTarget === "groq"
                && credentialReadMessage && (
                <p
                  role={credentialReadMessage.tone === "error" ? "alert" : "status"}
                  aria-live="polite"
                  className={`mt-2 break-words text-xs leading-5 ${credentialReadMessage.tone === "error" ? "text-error-ink" : credentialReadMessage.tone === "success" ? "text-success-ink" : "text-secondary"}`}
                >
                  {credentialReadMessage.text}
                </p>
              )}
            </div>
          )}
        </fieldset>
      </div>
    );
  };

  return (
    <div
      id="engine-panel-api_key"
      role="region"
      aria-label={t("API Key")}
      hidden={page !== "api_key"}
      inert={page !== "api_key"}
      className="vf-engine-page-content"
    >
      <SettingsGroup title={t("服务商凭据")} description={t("密钥按服务商保存在钥匙串中；转写和整理共用同一服务商密钥。")}>
        <div id="provider-list" className="vf-provider-list flex flex-col">
          {PROVIDERS
            .filter((provider) => providerHasCredentialInput(provider.id))
            .slice()
            .sort((left, right) => {
              const priority = (provider: typeof left) => {
                if (currentAsrProvider === provider.id || (settings.cleanup_enabled && currentCleanupProvider === provider.id)) return 0;
                if (draft.asrProvider === provider.id || (draftCleanupEnabled && draft.cleanupProvider === provider.id)) return 1;
                if (providerConfigured(settings, provider.id)) return 2;
                return 3;
              };
              return priority(left) - priority(right);
            })
            .map(renderProvider)}
        </div>
      </SettingsGroup>
    </div>
  );
}
