import { useSessionDraftState } from "../../lib/sessionDraft";
import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ConfirmDialog } from "../ConfirmDialog";
import { SettingsPageHeader, SettingsShell } from "../SettingsLayout";
import { useI18n } from "../../lib/i18n";
import { useLocalModelsStatus } from "../../hooks/useLocalModelsStatus";
import {
  draftFromSettings,
  customOriginChanged,
  hasSupportedAsrLanguage,
  hasProviderSecret,
  persistPatch,
  probePayload,
  providerConfigured,
  providerOf,
  step2Ready,
  switchProvider,
  type EngineDraft,
} from "../../lib/engineWizard";
import { modelLanguageSupport } from "../../lib/languages";
import { parseCustomCleanupParameters } from "../../lib/customCleanupParameters";
import {
  PROVIDERS,
  asrModelProfile,
  defaultModel,
  isLoopbackUrl,
  isProviderId,
  onDeviceMemoryCompatibilityCopy,
  providerById,
  providersFor,
  type ModelOption,
  type ProviderId,
} from "../../lib/providers";
import { friendlySettingsError, settingsErrorCode, settingsErrorMessage } from "../../lib/settingsError";
import { buttonClass, ghostButtonClass, compactButtonClass } from "../../lib/theme";
import type {
  CleanupModelStatus,
  DeviceMemoryBudget,
  LocalCleanupModelStatus,
  LocalCleanupStatus,
  MlxCleanupModelStatus,
  OnDeviceModelStatus,
  ProviderCredentialPurpose,
  ProviderCredentialReadResult,
  Settings,
} from "../../types/settings";
import {
  LEGACY_MLX_CLEANUP_MODEL_ID,
  ENGINE_PAGES,
  asrDraftIdentity,
  cleanupDraftIdentity,
  formatModelBytes,
  isMlxCleanupModelOperationActive,
  isReadyLocalCleanupModel,
  isSelectableLocalCleanupModel,
  keepEngineControlInsideRails,
  localCleanupCompatibilityCopy,
  localCleanupModelStateCopy,
  mlxCleanupCompatibilityCopy,
  mlxCleanupLanguageCopy,
  mlxCleanupModelStateCopy,
  onDeviceRuntimeCopy,
  onDeviceStateCopy,
  probeCopy,
  providerHasCredentialInput,
  stableJson,
  type EnginePage,
  type ProbeResult,
  type ProviderKeyRevisions,
} from "./engine/engineHelpers";
import { EngineServiceTab } from "./engine/EngineServiceTab";
import { EngineApiKeyTab } from "./engine/EngineApiKeyTab";
import { LocalModelsTab } from "./engine/LocalModelsTab";
import { EngineAdvancedTab } from "./engine/EngineAdvancedTab";

export type { EnginePage } from "./engine/engineHelpers";

export function EngineSettings({
  settings,
  page,
  onPageChange,
  removeProviderKey,
  commitEngine,
  readProviderCredential,
  cancelProviderCredentialRead,
}: {
  settings: Settings;
  page: EnginePage;
  onPageChange: (page: EnginePage) => void;
  removeProviderKey: (provider: string) => Promise<void>;
  commitEngine: (patch: Record<string, unknown>) => Promise<void>;
  readProviderCredential?: (provider: string, requestId: string, purpose?: ProviderCredentialPurpose) => Promise<ProviderCredentialReadResult>;
  cancelProviderCredentialRead?: (requestId: string) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraftState] = useSessionDraftState<EngineDraft>("engine:draft", () => draftFromSettings(settings));
  const [draftCleanupEnabled, setDraftCleanupEnabledState] = useSessionDraftState("engine:draftCleanupEnabled", settings.cleanup_enabled);
  const [draftRevision, setDraftRevision] = useState(0);
  const [asrDraftRevision, setAsrDraftRevision] = useState(0);
  const [cleanupDraftRevision, setCleanupDraftRevision] = useState(0);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const cleanupEnabledRef = useRef(draftCleanupEnabled);
  cleanupEnabledRef.current = draftCleanupEnabled;
  const providerKeyRevisions = useRef<ProviderKeyRevisions>({});
  const setDraftCleanupEnabled = useCallback<Dispatch<SetStateAction<boolean>>>((action) => {
    const previous = cleanupEnabledRef.current;
    const next = typeof action === "function" ? action(previous) : action;
    if (previous !== next) setCleanupDraftRevision((revision) => revision + 1);
    cleanupEnabledRef.current = next;
    setDraftRevision((revision) => revision + 1);
    setDraftCleanupEnabledState(next);
  }, [setDraftCleanupEnabledState]);
  const setDraft = useCallback<Dispatch<SetStateAction<EngineDraft>>>((action) => {
    const previous = draftRef.current;
    const next = typeof action === "function" ? (action as (current: EngineDraft) => EngineDraft)(previous) : action;
    const previousKeyRevisions = providerKeyRevisions.current;
    const nextKeyRevisions: ProviderKeyRevisions = { ...previousKeyRevisions };
    for (const provider of PROVIDERS) {
      if ((previous.providerKeys[provider.id] ?? "") === (next.providerKeys[provider.id] ?? "")) continue;
      nextKeyRevisions[provider.id] = (nextKeyRevisions[provider.id] ?? 0) + 1;
    }
    if (asrDraftIdentity(previous, previousKeyRevisions) !== asrDraftIdentity(next, nextKeyRevisions)) {
      setAsrDraftRevision((revision) => revision + 1);
    }
    if (cleanupDraftIdentity(previous, cleanupEnabledRef.current, previousKeyRevisions)
      !== cleanupDraftIdentity(next, cleanupEnabledRef.current, nextKeyRevisions)) {
      setCleanupDraftRevision((revision) => revision + 1);
    }
    draftRef.current = next;
    providerKeyRevisions.current = nextKeyRevisions;
    setDraftRevision((revision) => revision + 1);
    setDraftState(next);
  }, [setDraftState]);
  const [pendingPageFocus, setPendingPageFocus] = useState<string | null>(null);
  const [advancedConnectionsOpen, setAdvancedConnectionsOpen] = useState(false);
  const [metricsError, setMetricsError] = useState<string | null>(null);
  const draftSettings = { ...settings, cleanup_enabled: draftCleanupEnabled };
  const settingsObjectRef = useRef(settings);
  const settingsSnapshotRevisionRef = useRef(0);
  if (settingsObjectRef.current !== settings) {
    settingsObjectRef.current = settings;
    settingsSnapshotRevisionRef.current += 1;
  }
  const savedCredentialRevision = (provider: ProviderId) => JSON.stringify([
    settings.provider_keys?.[provider]?.configured,
    settings.provider_keys?.[provider]?.hint,
    settings.provider_keys?.[provider]?.keychain_state,
    provider === "groq" ? settings.api_key_configured : undefined,
    provider === "custom" ? settings.asr_api_key_configured : undefined,
    provider === "custom" ? settings.cleanup_api_key_configured : undefined,
  ]);
  const asrRoute = JSON.stringify([
    asrDraftIdentity(draft, providerKeyRevisions.current),
    asrDraftRevision,
    savedCredentialRevision(draft.asrProvider),
  ]);
  const cleanupRoute = JSON.stringify([
    cleanupDraftIdentity(draft, draftCleanupEnabled, providerKeyRevisions.current),
    cleanupDraftRevision,
    savedCredentialRevision(draft.cleanupProvider),
  ]);
  const probeConfigurationRevision = JSON.stringify([
    asrRoute,
    cleanupRoute,
    draftCleanupEnabled,
    draftRevision,
    settingsSnapshotRevisionRef.current,
    settings.asr_provider,
    settings.asr_model,
    settings.asr_base_url,
    settings.cleanup_provider,
    settings.cleanup_model,
    settings.cleanup_enabled,
    settings.custom_base_url,
    settings.cleanup_base_url,
    settings.strict_offline_enabled,
    ...PROVIDERS.flatMap((provider) => [
      provider.id,
      settings.provider_keys?.[provider.id]?.configured,
      settings.provider_keys?.[provider.id]?.keychain_state,
    ]),
  ]);
  const probeConfigurationRevisionRef = useRef(probeConfigurationRevision);
  probeConfigurationRevisionRef.current = probeConfigurationRevision;
  const onDeviceModelsRequestSeqRef = useRef(0);
  const customParametersParse = parseCustomCleanupParameters(draft.customCleanupParametersText);
  const [probing, setProbing] = useState(false);
  const [rowProbing, setRowProbing] = useState<ProviderId | null>(null);
  const [rowProbingStage, setRowProbingStage] = useState<"asr" | "cleanup" | null>(null);
  const [rowProbeFeedback, setRowProbeFeedback] = useState<{
    provider: ProviderId;
    stage?: "asr" | "cleanup";
    tone: "success" | "error";
    message: string;
  } | null>(null);
  const [probeSuccess, setProbeSuccess] = useState<"tested" | "saved" | "">("");
  const [testedCleanupRoute, setTestedCleanupRoute] = useState<string | null>(null);
  const [testedAsrRoute, setTestedAsrRoute] = useState<string | null>(null);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [stageFail, setStageFail] = useState<{ asr?: string | null; cleanup?: string | null }>({});
  const [credentialReadPending, setCredentialReadPending] = useState(false);
  const [credentialReadStatus, setCredentialReadStatus] = useState<ProviderCredentialReadResult["status"] | null>(null);
  const [credentialReadTarget, setCredentialReadTarget] = useState<ProviderId | null>(null);
  const [credentialReadMessage, setCredentialReadMessage] = useState<{ tone: "success" | "status" | "error"; text: string } | null>(null);
  const credentialReadRequest = useRef<string | null>(null);
  const credentialReadBlurTimer = useRef<number | null>(null);
  const cancelCredentialReadRef = useRef(cancelProviderCredentialRead);
  const clearCredentialReadBlurTimer = useCallback(() => {
    if (credentialReadBlurTimer.current === null) return;
    window.clearTimeout(credentialReadBlurTimer.current);
    credentialReadBlurTimer.current = null;
  }, []);
  useEffect(() => {
    cancelCredentialReadRef.current = cancelProviderCredentialRead;
  }, [cancelProviderCredentialRead]);
  useEffect(() => () => {
    clearCredentialReadBlurTimer();
    const requestId = credentialReadRequest.current;
    credentialReadRequest.current = null;
    const cancel = cancelCredentialReadRef.current;
    if (requestId && cancel) void cancel(requestId).catch(() => undefined);
  }, [clearCredentialReadBlurTimer]);
  const credentialReadConfiguration = JSON.stringify([
    draft.asrProvider,
    draft.asrModel,
    draft.cleanupProvider,
    draft.cleanupModel,
    draft.customAsr,
    draft.customLlm,
    draft.dashscopeRegion,
    draftCleanupEnabled,
    draft.customBaseUrl,
    draft.customCleanupParametersText,
    draft.providerKeys.custom,
    draft.ollamaBaseUrl,
    draft.localWhisperBaseUrl,
    draft.providerKeys.groq,
    settings.asr_provider,
    settings.cleanup_provider,
    settings.cleanup_enabled,
    settings.custom_base_url,
    settings.asr_base_url,
    settings.cleanup_base_url,
    draftRevision,
  ]);
  const credentialReadConfigurationRef = useRef(credentialReadConfiguration);
  useEffect(() => {
    if (credentialReadConfigurationRef.current === credentialReadConfiguration) return;
    credentialReadConfigurationRef.current = credentialReadConfiguration;
    const requestId = credentialReadRequest.current;
    if (!requestId) return;
    credentialReadRequest.current = null;
    clearCredentialReadBlurTimer();
    setCredentialReadPending(false);
    setCredentialReadStatus("stale");
    setCredentialReadMessage({ tone: "status", text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") });
    const cancel = cancelCredentialReadRef.current;
    if (cancel) void cancel(requestId).catch(() => undefined);
  }, [credentialReadConfiguration, t, clearCredentialReadBlurTimer]);

  useEffect(() => {
    if (page !== "api_key") return;
    const onWindowBlur = () => {
      if (!credentialReadRequest.current || credentialReadBlurTimer.current !== null) return;
      credentialReadBlurTimer.current = window.setTimeout(() => {
        credentialReadBlurTimer.current = null;
        const requestId = credentialReadRequest.current;
        if (!requestId || document.hasFocus()) return;
        credentialReadRequest.current = null;
        setCredentialReadPending(false);
        setCredentialReadStatus("cancelled");
        setCredentialReadMessage({ tone: "status", text: t("钥匙串读取超时；当前设置未更改。") });
        const cancel = cancelCredentialReadRef.current;
        if (cancel) void cancel(requestId).catch(() => undefined);
      }, 10_000);
    };
    const onWindowFocus = () => clearCredentialReadBlurTimer();
    window.addEventListener("blur", onWindowBlur);
    window.addEventListener("focus", onWindowFocus);
    return () => {
      clearCredentialReadBlurTimer();
      window.removeEventListener("blur", onWindowBlur);
      window.removeEventListener("focus", onWindowFocus);
    };
  }, [page, t, clearCredentialReadBlurTimer]);
  const [expanded, setExpanded] = useState<Set<ProviderId>>(() => new Set());
  const [confirmRemove, setConfirmRemove] = useState<ProviderId | null>(null);
  const [removing, setRemoving] = useState(false);
  const [onDeviceModels, setOnDeviceModels] = useState<OnDeviceModelStatus[]>([]);
  const [onDeviceModelsLoading, setOnDeviceModelsLoading] = useState(true);
  const [onDeviceModelReadError, setOnDeviceModelReadError] = useState<string | null>(null);
  const [deviceMemoryBudget, setDeviceMemoryBudget] = useState<DeviceMemoryBudget | null>(null);
  const [onDeviceActionId, setOnDeviceActionId] = useState<string | null>(null);
  const [onDeviceActionError, setOnDeviceActionError] = useState<{ id: string; message: string } | null>(null);
  const [legacyModelsOpen, setLegacyModelsOpen] = useState(false);
  const [cleanupModelStoreStatus, setCleanupModelStoreStatus] = useState<CleanupModelStatus | null>(null);
  const [cleanupModelStoreBusy, setCleanupModelStoreBusy] = useState(false);
  const [cleanupModelStoreActionError, setCleanupModelStoreActionError] = useState<string | null>(null);
  const [cleanupModelStoreReadError, setCleanupModelStoreReadError] = useState<string | null>(null);
  const [confirmDeleteCleanupModel, setConfirmDeleteCleanupModel] = useState(false);
  const [localCleanupModels, setLocalCleanupModels] = useState<LocalCleanupModelStatus[]>([]);
  const [localCleanupModelsLoading, setLocalCleanupModelsLoading] = useState(true);
  const [localCleanupModelsReadError, setLocalCleanupModelsReadError] = useState<string | null>(null);
  const [localCleanupModelBusy, setLocalCleanupModelBusy] = useState<Record<string, string>>({});
  const [localCleanupModelActionErrors, setLocalCleanupModelActionErrors] = useState<Record<string, string>>({});
  const [confirmDeleteLocalCleanupModel, setConfirmDeleteLocalCleanupModel] = useState<string | null>(null);
  const [mlxCleanupModels, setMlxCleanupModels] = useState<MlxCleanupModelStatus[]>([]);
  const [mlxCleanupModelsLoading, setMlxCleanupModelsLoading] = useState(true);
  const [mlxCleanupModelsReadError, setMlxCleanupModelsReadError] = useState<string | null>(null);
  const [mlxCleanupModelBusy, setMlxCleanupModelBusy] = useState<Record<string, string>>({});
  const [mlxCleanupModelActionErrors, setMlxCleanupModelActionErrors] = useState<Record<string, string>>({});
  const [confirmDeleteMlxCleanupModel, setConfirmDeleteMlxCleanupModel] = useState<string | null>(null);
  const [confirmDeleteModel, setConfirmDeleteModel] = useState<string | null>(null);
  const [localCleanupStatus, setLocalCleanupStatus] = useState<LocalCleanupStatus | null>(null);
  const [localCleanupChecking, setLocalCleanupChecking] = useState(false);
  const [localCleanupError, setLocalCleanupError] = useState<string | null>(null);
  const [strictOfflineSaving, setStrictOfflineSaving] = useState(false);
  const [strictOfflineSaveError, setStrictOfflineSaveError] = useState<string | null>(null);
  const [savedDraftSnapshot, setSavedDraftSnapshot] = useSessionDraftState("engine:saved", () => ({ draft: draftFromSettings(settings), cleanup: settings.cleanup_enabled }));
  const previousSettings = useRef(savedDraftSnapshot);
  useEffect(() => {
    const previous = previousSettings.current;
    const next = { draft: draftFromSettings(settings), cleanup: settings.cleanup_enabled };
    const savedCustomOriginChanged = customOriginChanged(previous.draft.customBaseUrl, next.draft.customBaseUrl);
    previousSettings.current = next;
    setSavedDraftSnapshot(next);
    setDraft((current) => {
      if (JSON.stringify({ ...current, providerKeys: {} }) !== JSON.stringify(previous.draft)) return current;
      return {
        ...next.draft,
        providerKeys: {
          ...current.providerKeys,
          ...(savedCustomOriginChanged ? { custom: "" } : {}),
        },
      };
    });
    setDraftCleanupEnabled((current) => current === previous.cleanup ? settings.cleanup_enabled : current);
  }, [settings, setDraft, setDraftCleanupEnabled, setSavedDraftSnapshot]);

  const navigateToPage = useCallback((nextPage: EnginePage, targetId?: string) => {
    if (nextPage === "api_key") {
      const provider = targetId?.startsWith("provider-key-") ? targetId.slice("provider-key-".length) : null;
      if (provider && isProviderId(provider) && providerHasCredentialInput(provider)) {
        setExpanded((current) => new Set([...current, provider]));
      }
    }
    if (nextPage === "advanced" && targetId && targetId !== "strict-offline-toggle") setAdvancedConnectionsOpen(true);
    if (nextPage === "local_models" && targetId === "local-asr-model-sensevoice-small") setLegacyModelsOpen(true);
    setPendingPageFocus(targetId ?? null);
    onPageChange(nextPage);
  }, [onPageChange]);
  useEffect(() => {
    if (!pendingPageFocus) return;
    let frame = 0;
    frame = window.requestAnimationFrame(() => {
      const target = document.getElementById(pendingPageFocus);
      if (!target) {
        setPendingPageFocus(null);
        return;
      }
      keepEngineControlInsideRails(target, true);
      target.focus({ preventScroll: true });
      setPendingPageFocus(null);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [page, pendingPageFocus, advancedConnectionsOpen, expanded, legacyModelsOpen]);

  useEffect(() => {
    if (page === "api_key") return;
    const requestId = credentialReadRequest.current;
    if (!requestId) return;
    credentialReadRequest.current = null;
    clearCredentialReadBlurTimer();
    setCredentialReadPending(false);
    setCredentialReadStatus("stale");
    setCredentialReadMessage({ tone: "status", text: t("离开 API Key 页面后，读取请求已取消；请重新发起读取。") });
    const cancel = cancelCredentialReadRef.current;
    if (cancel) void cancel(requestId).catch(() => undefined);
  }, [page, t, clearCredentialReadBlurTimer]);

  useEffect(() => {
    let cancelled = false;
    const seq = ++onDeviceModelsRequestSeqRef.current;
    const apply = (models: OnDeviceModelStatus[]) => {
      if (cancelled || seq !== onDeviceModelsRequestSeqRef.current) return;
      setOnDeviceModels(Array.isArray(models) ? models : []);
      setOnDeviceModelReadError(null);
      setOnDeviceModelsLoading(false);
    };
    void Promise.resolve(invoke<OnDeviceModelStatus[]>("list_on_device_models"))
      .then(apply)
      .catch(() => {
        if (!cancelled && seq === onDeviceModelsRequestSeqRef.current) {
          setOnDeviceModels([]);
          setOnDeviceModelReadError(t("无法读取本机模型状态，请重试。"));
          setOnDeviceModelsLoading(false);
        }
      });
    void Promise.resolve(invoke<DeviceMemoryBudget>("get_on_device_memory_budget"))
      .then((budget) => {
        if (cancelled) return;
        setDeviceMemoryBudget(
          budget && budget.budget_percent === 80
            && (budget.physical_memory_bytes === null || Number.isFinite(budget.physical_memory_bytes))
            && (budget.budget_bytes === null || Number.isFinite(budget.budget_bytes))
            ? budget
            : null,
        );
      })
      .catch(() => {
        if (!cancelled) setDeviceMemoryBudget(null);
      });
    return () => {
      cancelled = true;
    };
  }, [t]);

  const selectedOnDeviceModel = onDeviceModels.find((model) => model.id === draft.asrModel);
  const onDeviceReady = selectedOnDeviceModel?.inference_ready === true;

  const refreshOnDeviceModels = async () => {
    const seq = ++onDeviceModelsRequestSeqRef.current;
    setOnDeviceModelsLoading(true);
    setOnDeviceModelReadError(null);
    try {
      const models = await invoke<OnDeviceModelStatus[]>("list_on_device_models");
      if (seq !== onDeviceModelsRequestSeqRef.current) return;
      setOnDeviceModels(Array.isArray(models) ? models : []);
      setOnDeviceModelReadError(null);
    } catch {
      if (seq === onDeviceModelsRequestSeqRef.current) {
        setOnDeviceModelReadError(t("无法读取本机模型状态，请重试。"));
      }
    } finally {
      if (seq === onDeviceModelsRequestSeqRef.current) {
        setOnDeviceModelsLoading(false);
      }
    }
  };

  const refreshCleanupModelStore = async () => {
    try {
      const status = await invoke<CleanupModelStatus>("get_cleanup_model_store_status");
      setCleanupModelStoreStatus(status);
      setCleanupModelStoreReadError(null);
    } catch {
      setCleanupModelStoreReadError(t("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。"));
    }
  };

  const refreshLocalCleanupModels = async (showLoading = false) => {
    if (showLoading) setLocalCleanupModelsLoading(true);
    try {
      const models = await invoke<LocalCleanupModelStatus[]>("list_local_cleanup_models");
      setLocalCleanupModels(Array.isArray(models) ? models : []);
      setLocalCleanupModelsReadError(null);
    } catch (reason) {
      setLocalCleanupModelsReadError(`${t("本机整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
    } finally {
      if (showLoading) setLocalCleanupModelsLoading(false);
    }
  };

  const runLocalCleanupModelAction = async (id: string, action: "download" | "cancel" | "delete") => {
    setLocalCleanupModelBusy((current) => ({ ...current, [id]: action }));
    setLocalCleanupModelActionErrors((current) => ({ ...current, [id]: "" }));
    try {
      if (action === "download") await invoke("download_local_cleanup_model", { id });
      else if (action === "cancel") {
        const cancelled = await invoke<boolean>("cancel_local_cleanup_model_download", { id });
        if (!cancelled) setLocalCleanupModelActionErrors((current) => ({
          ...current,
          [id]: t("下载已经结束或当前无法取消；模型状态已刷新。"),
        }));
      } else await invoke("delete_local_cleanup_model", { id });
    } catch (reason) {
      const rawMessage = settingsErrorMessage(reason);
      const isActive = settingsErrorCode(reason) === "local_model_active" || rawMessage === "local_cleanup_model_is_active";
      const message = isActive
        ? t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")
        : `${action === "download" ? t("模型下载失败") : action === "cancel" ? t("取消下载失败") : t("删除模型失败")}: ${rawMessage}`;
      setLocalCleanupModelActionErrors((current) => ({ ...current, [id]: message }));
    } finally {
      await refreshLocalCleanupModels();
      setLocalCleanupModelBusy((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
  };

  const refreshMlxCleanupModels = async (showLoading = false) => {
    if (showLoading) setMlxCleanupModelsLoading(true);
    try {
      const models = await invoke<MlxCleanupModelStatus[]>("list_mlx_cleanup_models");
      setMlxCleanupModels(Array.isArray(models) ? models : []);
      setMlxCleanupModelsReadError(null);
    } catch (reason) {
      setMlxCleanupModelsReadError(`${t("本机 MLX 整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
    } finally {
      if (showLoading) setMlxCleanupModelsLoading(false);
    }
  };

  const runMlxCleanupModelAction = async (id: string, action: "download" | "verify" | "cancel" | "delete") => {
    setMlxCleanupModelBusy((current) => ({ ...current, [id]: action }));
    setMlxCleanupModelActionErrors((current) => ({ ...current, [id]: "" }));
    try {
      if (action === "download") await invoke("download_mlx_cleanup_model", { id });
      else if (action === "verify") await invoke("verify_mlx_cleanup_model", { id });
      else if (action === "cancel") await invoke("cancel_mlx_cleanup_model_download", { id });
      else await invoke("delete_mlx_cleanup_model", { id });
    } catch (reason) {
      const rawMessage = settingsErrorMessage(reason);
      const isActive = settingsErrorCode(reason) === "local_model_active" || rawMessage === "local_cleanup_model_is_active";
      const message = isActive
        ? t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")
        : `${action === "download" ? t("MLX 整理模型下载失败") : action === "verify" ? t("验证 MLX 整理模型失败") : action === "cancel" ? t("取消 MLX 整理模型下载失败") : t("删除 MLX 整理模型失败")}: ${rawMessage}`;
      setMlxCleanupModelActionErrors((current) => ({ ...current, [id]: message }));
    } finally {
      await refreshMlxCleanupModels();
      setMlxCleanupModelBusy((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
  };

  const runCleanupModelStoreAction = async (action: "import" | "download" | "cancel" | "delete") => {
    setCleanupModelStoreBusy(true);
    setCleanupModelStoreActionError(null);
    try {
      if (action === "import") await invoke("import_cleanup_model");
      else if (action === "download") await invoke("download_cleanup_model");
      else if (action === "cancel") await invoke("cancel_cleanup_model_store_operation");
      else await invoke("delete_cleanup_model_store");
    } catch {
      setCleanupModelStoreActionError(t("本机 MLX 模型操作失败，请重试。"));
    } finally {
      await refreshCleanupModelStore();
      setCleanupModelStoreBusy(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const status = await invoke<CleanupModelStatus>("get_cleanup_model_store_status");
        if (cancelled) return;
        setCleanupModelStoreStatus(status);
        setCleanupModelStoreReadError(null);
      } catch {
        if (!cancelled) setCleanupModelStoreReadError(t("无法读取 VoiceFlow 本机 MLX 模型状态，请重试。"));
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    const applyModels = (models: MlxCleanupModelStatus[]) => {
      if (cancelled) return;
      setMlxCleanupModels(Array.isArray(models) ? models : []);
      setMlxCleanupModelsReadError(null);
      setMlxCleanupModelsLoading(false);
    };
    void Promise.resolve(invoke<MlxCleanupModelStatus[]>("list_mlx_cleanup_models"))
      .then(applyModels)
      .catch((reason) => {
        if (!cancelled) {
          setMlxCleanupModelsReadError(`${t("本机 MLX 整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
          setMlxCleanupModelsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const models = await invoke<LocalCleanupModelStatus[]>("list_local_cleanup_models");
        if (cancelled) return;
        const nextModels = Array.isArray(models) ? models : [];
        setLocalCleanupModels(nextModels);
        setLocalCleanupModelsReadError(null);
      } catch (reason) {
        if (cancelled) return;
        setLocalCleanupModelsReadError(`${t("本机整理模型状态读取失败，请重试。")} ${String(reason)}`.trim());
      } finally {
        if (!cancelled) setLocalCleanupModelsLoading(false);
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [t]);

  const engineMountedRef = useRef(true);
  useEffect(() => {
    engineMountedRef.current = true;
    return () => {
      engineMountedRef.current = false;
    };
  }, []);

  useLocalModelsStatus({
    onOnDeviceAsrStatus: (status) => {
      onDeviceModelsRequestSeqRef.current += 1;
      setOnDeviceModels((current) => {
        const previous = current.find((model) => model.id === status.id);
        const keepPreviousRuntime = status.state === "downloading"
          || (!status.inference_ready && status.runtime_status === "not_checked");
        const nextStatus = previous && keepPreviousRuntime
          ? {
              ...status,
              runtime_status: previous.runtime_status,
              inference_ready: status.state === "ready" && previous.inference_ready,
              loaded: status.state === "ready" && previous.loaded,
            }
          : status;
        return current.some((model) => model.id === status.id)
          ? current.map((model) => model.id === status.id ? nextStatus : model)
          : [...current, nextStatus];
      });
      setOnDeviceModelReadError(null);
      setOnDeviceModelsLoading(false);
      if (status.state !== "downloading") {
        const seq = ++onDeviceModelsRequestSeqRef.current;
        void Promise.resolve(invoke<OnDeviceModelStatus[]>("list_on_device_models"))
          .then((models) => {
            if (!engineMountedRef.current || seq !== onDeviceModelsRequestSeqRef.current) return;
            setOnDeviceModels(Array.isArray(models) ? models : []);
            setOnDeviceModelReadError(null);
            setOnDeviceModelsLoading(false);
          })
          .catch(() => {
            // Keep the last known runtime state until the next runtime event or manual refresh.
          });
      }
    },
    onOnDeviceRuntimeChanged: () => {
      const seq = ++onDeviceModelsRequestSeqRef.current;
      void Promise.resolve(invoke<OnDeviceModelStatus[]>("list_on_device_models"))
        .then((models) => {
          if (!engineMountedRef.current || seq !== onDeviceModelsRequestSeqRef.current) return;
          setOnDeviceModels(Array.isArray(models) ? models : []);
          setOnDeviceModelReadError(null);
          setOnDeviceModelsLoading(false);
        })
        .catch(() => {
          // Keep the last known runtime state until the next runtime event or manual refresh.
        });
    },
    onCleanupStoreStatus: (status) => {
      setCleanupModelStoreStatus(status);
      setCleanupModelStoreReadError(null);
    },
    onMlxCleanupStatus: (status) => {
      setMlxCleanupModels((current) => (
        current.some((model) => model.id === status.id)
          ? current.map((model) => model.id === status.id ? status : model)
          : [...current, status]
      ));
      setMlxCleanupModelsReadError(null);
      setMlxCleanupModelsLoading(false);
    },
    onGgufCleanupStatus: (status) => {
      setLocalCleanupModels((current) => (
        current.some((model) => model.id === status.id)
          ? current.map((model) => model.id === status.id ? status : model)
          : [...current, status]
      ));
      setLocalCleanupModelsReadError(null);
      setLocalCleanupModelsLoading(false);
    },
  });

  const runOnDeviceAction = async (id: string, action: "download" | "cancel" | "cancel_load" | "delete") => {
    setOnDeviceActionId(id);
    setOnDeviceActionError(null);
    try {
      if (action === "download") await invoke("download_on_device_model", { id });
      else if (action === "cancel") await invoke("cancel_on_device_download", { id });
      else if (action === "cancel_load") await invoke<boolean>("cancel_on_device_model_load", { id });
      else await invoke("delete_on_device_model", { id });
      await refreshOnDeviceModels();
    } catch {
      setOnDeviceActionError({ id, message: t("本机模型操作失败，请重试。") });
      await refreshOnDeviceModels();
    } finally {
      setOnDeviceActionId(null);
    }
  };

  const checkLocalCleanupStatus = async () => {
    setLocalCleanupChecking(true);
    setLocalCleanupError(null);
    try {
      setLocalCleanupStatus(await invoke<LocalCleanupStatus>("get_local_cleanup_status"));
    } catch {
      setLocalCleanupStatus(null);
      setLocalCleanupError(t("无法读取本机 Ollama 状态，请重试。"));
    } finally {
      setLocalCleanupChecking(false);
    }
  };

  const chooseLocalCleanup = () => {
    const ollamaBaseUrl = providerById("ollama")?.defaultBaseUrl ?? "http://127.0.0.1:11434/v1";
    setDraft((current) => ({
      ...current,
      cleanupProvider: "ollama",
      cleanupModel: "qwen3.5:4b",
      ollamaBaseUrl,
    }));
    setDraftCleanupEnabled(true);
    setProbeSuccess("");
    setLocalCleanupStatus(null);
  };

  const currentCleanupProvider = providerOf(settings.cleanup_provider, settings.cleanup_base_url);
  const savedCleanupModel = settings.cleanup_model || defaultModel(currentCleanupProvider, "llm");
  const selectableLocalCleanupModels = localCleanupModels.filter(isSelectableLocalCleanupModel);
  const selectedLocalCleanupModel = localCleanupModels.find((model) => model.id === draft.cleanupModel);
  const selectedSavedLocalCleanupModel = localCleanupModels.find((model) => model.id === savedCleanupModel);
  const draftLocalCleanupModelReady = isSelectableLocalCleanupModel(selectedLocalCleanupModel)
    || (draft.cleanupProvider === "on_device_llama_cpp"
      && currentCleanupProvider === "on_device_llama_cpp"
      && draft.cleanupModel === savedCleanupModel
      && isReadyLocalCleanupModel(selectedLocalCleanupModel));
  const savedLocalCleanupModelReady = isReadyLocalCleanupModel(selectedSavedLocalCleanupModel);
  const localCleanupDeleteTarget = localCleanupModels.find((model) => model.id === confirmDeleteLocalCleanupModel);
  const cleaningOnlyPartialModel = Boolean(localCleanupDeleteTarget && !localCleanupDeleteTarget.managed && localCleanupDeleteTarget.partial_managed);
  const selectedMlxCleanupModel = mlxCleanupModels.find((model) => model.id === draft.cleanupModel);
  const savedMlxCleanupModel = mlxCleanupModels.find((model) => model.id === savedCleanupModel);
  const selectedMlxCleanupLanguage = selectedMlxCleanupModel
    ? mlxCleanupLanguageCopy(selectedMlxCleanupModel.language)
    : null;
  const mlxCleanupDeleteTarget = mlxCleanupModels.find((model) => model.id === confirmDeleteMlxCleanupModel);
  const cleaningOnlyPartialMlxCleanupModel = Boolean(mlxCleanupDeleteTarget && !mlxCleanupDeleteTarget.managed && mlxCleanupDeleteTarget.partial_managed);
  const mlxCleanupManageTarget = draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
    ? "local-cleanup-model"
    : selectedMlxCleanupModel ? `mlx-cleanup-model-${selectedMlxCleanupModel.id}` : "local-mlx-cleanup-models";

  const selectedAsrIsLocal = draft.asrProvider === "on_device";
  const cleanupMlxPlatformSupported = cleanupModelStoreStatus?.platform_supported === true;
  const cleanupMlxPlatformUnsupported = cleanupModelStoreStatus?.platform_supported === false;
  const cleanupModelStoreNeedsRefresh = !cleanupModelStoreStatus
    || cleanupModelStoreReadError !== null
    || cleanupModelStoreStatus.platform_supported === null
    || cleanupModelStoreStatus.platform_supported === undefined;
  const cleanupModelStoreIdle = Boolean(cleanupModelStoreStatus && !cleanupModelStoreStatus.operation && !cleanupModelStoreReadError);
  const cleanupMlxInstalled = cleanupMlxPlatformSupported
    && cleanupModelStoreStatus != null
    && cleanupModelStoreStatus.managed
    && ["ready", "installed"].includes(cleanupModelStoreStatus.state);
  const savedMlxCleanupModelReady = currentCleanupProvider === "on_device_mlx"
    && (savedCleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
      ? cleanupMlxInstalled
      : savedMlxCleanupModel?.selectable === true
        || (savedMlxCleanupModel?.state === "ready"
          && savedMlxCleanupModel.verified
          && savedMlxCleanupModel.compatible
          && !isMlxCleanupModelOperationActive(savedMlxCleanupModel)));
  const draftMlxCleanupModelReady = draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
    ? cleanupMlxInstalled
    : selectedMlxCleanupModel?.selectable === true
      || (draft.cleanupProvider === "on_device_mlx"
        && currentCleanupProvider === "on_device_mlx"
        && draft.cleanupModel === savedCleanupModel
        && selectedMlxCleanupModel?.state === "ready"
        && selectedMlxCleanupModel.verified
        && selectedMlxCleanupModel.compatible
        && !isMlxCleanupModelOperationActive(selectedMlxCleanupModel));
  const selectedCleanupIsLocal = (draft.cleanupProvider === "ollama" && isLoopbackUrl(draft.ollamaBaseUrl))
    || (draft.cleanupProvider === "on_device_mlx" && draftMlxCleanupModelReady)
    || (draft.cleanupProvider === "on_device_llama_cpp" && draftLocalCleanupModelReady);
  const savedCleanupIsLocal = (currentCleanupProvider === "ollama"
      && isLoopbackUrl(settings.ollama_base_url || providerById("ollama")?.defaultBaseUrl || ""))
    || savedMlxCleanupModelReady
    || (currentCleanupProvider === "on_device_llama_cpp" && savedLocalCleanupModelReady);
  const strictOfflineRoutesAllowed = settings.strict_offline_enabled !== true
    || (selectedAsrIsLocal && (!draftCleanupEnabled || selectedCleanupIsLocal));
  const asrMissing = !hasProviderSecret(draft.asrProvider, draft, settings, onDeviceReady);
  const cleanupMissing = draftCleanupEnabled && !hasProviderSecret(draft.cleanupProvider, draft, settings);
  const draftAsrLanguageSupport = modelLanguageSupport(draft.asrProvider, draft.asrModel);
  const hasSupportedAsrLanguageSetting = hasSupportedAsrLanguage(asrModelProfile(draft.asrProvider, draft.asrModel), settings.language, draft.asrProvider, draft.asrModel);
  const canCoerceAsrLanguageOnSave = hasSupportedAsrLanguageSetting || draftAsrLanguageSupport.autoAllowed;
  const cleanupModelRouteReady = draft.cleanupProvider === "on_device_mlx"
    ? draftMlxCleanupModelReady
    : draft.cleanupProvider === "on_device_llama_cpp"
      ? draftLocalCleanupModelReady
      : true;
  const ready = step2Ready(draft, draftSettings, onDeviceReady)
    && canCoerceAsrLanguageOnSave
    && strictOfflineRoutesAllowed
    && (!draftCleanupEnabled || cleanupModelRouteReady)
    && (!draftCleanupEnabled || draft.cleanupProvider !== "custom" || customParametersParse.ok);
  const saveBlock: { message: string; page: EnginePage; target: string; action: string } | null = !ready
    ? !canCoerceAsrLanguageOnSave
      ? { message: t("当前转写模型不支持所选语言，请更换转写模型。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
      : !strictOfflineRoutesAllowed
        ? { message: t("严格离线模式会阻止草稿中的路线；请选择允许的本机路线或关闭严格离线模式。"), page: "service", target: draft.asrProvider !== "on_device" ? "engine-asr-provider" : "engine-cleanup-provider", action: t("修改路线") }
        : draft.asrProvider === "custom" && !draft.customBaseUrl.trim()
          ? { message: t("请先填写自定义服务地址。"), page: "advanced", target: "advanced-custom-base-url", action: t("管理连接地址") }
          : draft.asrProvider === "on_device" && !onDeviceReady
            ? { message: t("请安装并加载所选本机转写模型。"), page: "local_models", target: "local-asr-models", action: t("管理模型") }
            : asrMissing
              ? { message: t("请先添加当前转写服务的 API Key。"), page: "api_key", target: `provider-key-${draft.asrProvider}`, action: t("管理凭据") }
              : !draft.asrModel.trim()
                ? { message: t("请先选择转写模型。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
                : draftCleanupEnabled && draft.cleanupProvider === "custom" && !draft.customBaseUrl.trim()
                  ? { message: t("请先填写自定义服务地址。"), page: "advanced", target: "advanced-custom-base-url", action: t("管理连接地址") }
                  : draftCleanupEnabled && cleanupMissing && draft.asrProvider !== "assemblyai"
                    ? { message: t("请先添加当前整理服务的 API Key。"), page: "api_key", target: `provider-key-${draft.cleanupProvider}`, action: t("管理凭据") }
                    : draftCleanupEnabled && draft.cleanupProvider === "on_device_llama_cpp" && selectableLocalCleanupModels.length === 0
                      ? { message: t("请先下载并验证兼容的本机整理模型。"), page: "local_models", target: "local-cleanup-models", action: t("管理模型") }
                    : draftCleanupEnabled && !draft.cleanupModel.trim()
                      ? { message: t("请先选择整理模型。"), page: "service", target: "engine-cleanup-provider", action: t("修改路线") }
                      : draftCleanupEnabled && !cleanupModelRouteReady
                        ? draft.cleanupProvider === "on_device_llama_cpp"
                          ? { message: t("请先下载并验证兼容的本机整理模型。"), page: "local_models", target: "local-cleanup-models", action: t("管理模型") }
                          : { message: t("请先安装并验证 VoiceFlow MLX 整理模型。"), page: "local_models", target: mlxCleanupManageTarget, action: t("管理模型") }
                        : draftCleanupEnabled && draft.cleanupProvider === "custom" && !customParametersParse.ok
                          ? { message: customParametersParse.error, page: "advanced", target: "advanced-custom-cleanup-parameters", action: t("检查参数") }
                          : { message: t("请检查当前服务配置后再保存。"), page: "service", target: "engine-asr-provider", action: t("修改路线") }
    : null;
  const previousCredentialRoute = useRef({ asr: draft.asrProvider, cleanup: draft.cleanupProvider, cleanupEnabled: draftCleanupEnabled });

  useEffect(() => {
    const previous = previousCredentialRoute.current;
    previousCredentialRoute.current = { asr: draft.asrProvider, cleanup: draft.cleanupProvider, cleanupEnabled: draftCleanupEnabled };
    const cleanupRequiredForSelectedAsr = cleanupMissing && draft.asrProvider !== "assemblyai";
    // Keep the page overview visible on entry; guide to a missing key only
    // after the user changes the corresponding service.
    const missing = previous.asr !== draft.asrProvider && asrMissing
      ? draft.asrProvider
      : (previous.cleanup !== draft.cleanupProvider || !previous.cleanupEnabled && draftCleanupEnabled) && cleanupRequiredForSelectedAsr
        ? draft.cleanupProvider
        : null;
    if (!missing) return;
    if (missing === "on_device") {
      navigateToPage("local_models", "local-asr-models");
    } else {
      navigateToPage("api_key", `provider-key-${missing}`);
    }
  }, [asrMissing, cleanupMissing, draft.asrProvider, draft.cleanupProvider, draftCleanupEnabled, navigateToPage]);

  const currentAsrProvider = providerOf(settings.asr_provider, settings.asr_base_url);
  const savedAsrModel = settings.asr_model || defaultModel(currentAsrProvider, "asr");
  const legacyModel = onDeviceModels.find((model) => model.id === "sensevoice-small");
  const legacyModelSavedSelected = currentAsrProvider === "on_device" && savedAsrModel === "sensevoice-small";
  const legacyModelDraftSelected = draft.asrProvider === "on_device" && draft.asrModel === "sensevoice-small";
  const legacyModelShouldOpen = Boolean(legacyModel && (
    legacyModelSavedSelected
    || legacyModelDraftSelected
    || legacyModel.state === "downloading"
    || legacyModel.runtime_status === "loading"
    || Boolean(legacyModel.error)
    || onDeviceActionError?.id === legacyModel.id
  ));
  useEffect(() => {
    if (legacyModelShouldOpen) setLegacyModelsOpen(true);
  }, [legacyModelShouldOpen]);
  const savedDraft = draftFromSettings(settings);
  const draftPatch = persistPatch({ ...draft, providerKeys: {} }, draftSettings);
  const savedPatch = persistPatch(savedDraft, draftSettings);
  delete draftPatch.custom_cleanup_parameters;
  delete savedPatch.custom_cleanup_parameters;
  const customParametersDirty = customParametersParse.ok
    ? stableJson(customParametersParse.value) !== stableJson(savedDraft.customCleanupParameters)
    : draft.customCleanupParametersText !== savedDraft.customCleanupParametersText;
  const pendingChanges = draftCleanupEnabled !== settings.cleanup_enabled
    || customParametersDirty
    || JSON.stringify(draftPatch) !== JSON.stringify(savedPatch)
    || Object.values(draft.providerKeys).some((value) => value?.trim());
  const showEngineFooter = pendingChanges || probing;

  useEffect(() => {
    setStageFail({});
    setCommitError(null);
    setRowProbeFeedback(null);
  }, [asrRoute, cleanupRoute, draftCleanupEnabled]);

  const failKind = (kind: string | null | undefined) => {
    if (kind === "key" || kind === "missing_key") return t("密钥无效");
    if (kind) return probeCopy(kind, undefined, t);
    return null;
  };

  const strictOfflineBlocksProbe = (provider: ProviderId, stage: "asr" | "cleanup") => {
    if (settings.strict_offline_enabled !== true) return false;
    if (stage === "asr") return provider !== "on_device";
    if (provider === "ollama") return !isLoopbackUrl(draft.ollamaBaseUrl);
    return provider !== "on_device_mlx" && provider !== "on_device_llama_cpp";
  };

  const applyDraft = async () => {
    setProbing(true);
    setProbeSuccess("");
    setCommitError(null);
    try {
      await commitEngine({ ...persistPatch(draft, draftSettings), cleanup_enabled: draftCleanupEnabled });
      setDraft((current) => ({
        ...current,
        providerKeys: {},
        customCleanupParametersText:
          !customParametersParse.ok && !(draftCleanupEnabled && draft.cleanupProvider === "custom")
            ? savedDraft.customCleanupParametersText
            : current.customCleanupParametersText,
      }));
      setStageFail({});
      setTestedAsrRoute(null);
      setTestedCleanupRoute(null);
      setProbeSuccess("saved");
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setProbing(false);
    }
  };

  const activateModelRoute = async (patch: Record<string, unknown>, phase: "asr" | "cleanup") => {
    setProbing(true);
    setProbeSuccess("");
    setCommitError(null);
    try {
      await commitEngine(patch);
      if (phase === "asr") {
        setTestedAsrRoute(null);
        if (patch.asr_provider === "on_device" && typeof patch.asr_model === "string") {
          setDraft((current) => ({ ...current, asrProvider: "on_device", asrModel: patch.asr_model as string }));
        }
      } else {
        setTestedCleanupRoute(null);
        if ((patch.cleanup_provider === "on_device_mlx" || patch.cleanup_provider === "on_device_llama_cpp")
          && typeof patch.cleanup_model === "string") {
          setDraft((current) => ({
            ...current,
            cleanupProvider: patch.cleanup_provider as ProviderId,
            cleanupModel: patch.cleanup_model as string,
          }));
          setDraftCleanupEnabled(true);
        }
      }
      setProbeSuccess("saved");
    } catch (reason) {
      setCommitError(friendlySettingsError(reason, t));
    } finally {
      setProbing(false);
    }
  };

  const commitOnDeviceModel = async (model: OnDeviceModelStatus) => {
    if (onDeviceModelActivationBlock(model) || probing || rowProbing !== null || removing || strictOfflineSaving) return;
    await activateModelRoute({ asr_provider: "on_device", asr_model: model.id }, "asr");
  };

  const cleanupActivationBlock = (): string | null => settings.strict_offline_enabled === true && currentAsrProvider !== "on_device"
    ? t("严格离线模式要求转写路线也使用本机模型。")
    : null;

  const commitMlxCleanupModel = async (model: MlxCleanupModelStatus) => {
    if (!model.selectable || model.state !== "ready" || !model.verified || !model.compatible
      || isMlxCleanupModelOperationActive(model) || cleanupActivationBlock()
      || probing || rowProbing !== null || removing || strictOfflineSaving) return;
    await activateModelRoute({
      cleanup_enabled: true,
      cleanup_provider: "on_device_mlx",
      cleanup_model: model.id,
    }, "cleanup");
  };

  const commitLocalCleanupModel = async (model: LocalCleanupModelStatus) => {
    if (!isSelectableLocalCleanupModel(model) || cleanupActivationBlock()
      || probing || rowProbing !== null || removing || strictOfflineSaving) return;
    await activateModelRoute({
      cleanup_enabled: true,
      cleanup_provider: "on_device_llama_cpp",
      cleanup_model: model.id,
    }, "cleanup");
  };

  const legacyMlxActivationBlock = (status: CleanupModelStatus): string | null => {
    if (status.id !== LEGACY_MLX_CLEANUP_MODEL_ID || !status.managed || status.state !== "ready"
      || status.operation || !cleanupMlxPlatformSupported) {
      return t("此模型尚未校验就绪，或本机运行时不可用。");
    }
    if (!status.memory_estimate || deviceMemoryBudget?.budget_bytes == null) {
      return t("无法确认此模型的固定内存兼容性。");
    }
    if (status.memory_estimate.bytes > deviceMemoryBudget.budget_bytes) {
      return t("此模型超出当前设备的内存准入预算。");
    }
    return cleanupActivationBlock();
  };

  const commitLegacyMlxModel = async (status: CleanupModelStatus) => {
    if (legacyMlxActivationBlock(status) || probing || rowProbing !== null || removing || strictOfflineSaving) return;
    await activateModelRoute({
      cleanup_enabled: true,
      cleanup_provider: "on_device_mlx",
      cleanup_model: status.id,
    }, "cleanup");
  };

  const updateStrictOfflineMode = async (enabled: boolean) => {
    setStrictOfflineSaving(true);
    setStrictOfflineSaveError(null);
    try {
      await commitEngine({ strict_offline_enabled: enabled });
    } catch (reason) {
      setStrictOfflineSaveError(friendlySettingsError(reason, t));
    } finally {
      setStrictOfflineSaving(false);
    }
  };

  const testConnection = async (id: ProviderId, stageScope: "asr" | "cleanup") => {
    const definition = providerById(id);
    if (!definition) return;
    if (!definition.capabilities.includes(stageScope === "asr" ? "asr" : "llm")) return;
    const requestRevision = probeConfigurationRevision;
    setRowProbing(id);
    setRowProbingStage(stageScope);
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setCommitError(null);
    if (stageScope === "cleanup" && id === "custom" && !customParametersParse.ok) {
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: customParametersParse.error });
      setRowProbing(null);
      setRowProbingStage(null);
      return;
    }
    if (strictOfflineBlocksProbe(id, stageScope)) {
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: t("严格离线模式会阻止连接云端服务。") });
      setRowProbing(null);
      setRowProbingStage(null);
      return;
    }
    try {
      if (stageScope === "asr" && id === draft.asrProvider) setTestedAsrRoute(null);
      if (stageScope === "cleanup" && id === draft.cleanupProvider) setTestedCleanupRoute(null);
      if (id === "soniox") return;
      const typedKey = draft.providerKeys[id]?.trim();
      const probeDraft: EngineDraft = {
        ...draft,
        providerKeys: typedKey ? { [id]: typedKey } : {},
      };
      const result = await invoke<ProbeResult>("probe_engine_draft", {
        draft: probePayload(probeDraft, draftSettings, stageScope === "asr"
          ? { asrProvider: id, cleanupEnabled: false, probeStage: "asr" }
          : { cleanupProvider: id, cleanupEnabled: true, probeStage: "cleanup" }),
      });
      if (probeConfigurationRevisionRef.current !== requestRevision) return;
      const stage = stageScope === "asr" ? result.asr : result.cleanup;
      if (stage.skipped) {
        setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: t("此阶段未运行，请检查服务选择后重试。") });
        return;
      }
      if (!stage.ok) {
        setStageFail((current) => (
          stageScope === "asr" && id === draft.asrProvider ? { ...current, asr: stage.error_kind }
            : stageScope === "cleanup" && id === draft.cleanupProvider ? { ...current, cleanup: stage.error_kind } : current
        ));
        setRowProbeFeedback({
          provider: id,
          stage: stageScope,
          tone: "error",
          message: `${definition.label}：${probeCopy(stage.error_kind, stage.message, t)}`,
        });
        return;
      }
      setStageFail((current) => (
        stageScope === "asr" && id === draft.asrProvider ? { ...current, asr: undefined }
          : stageScope === "cleanup" && id === draft.cleanupProvider ? { ...current, cleanup: undefined } : current
      ));
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "success", message: `${stageScope === "asr" ? t("转写") : t("文字整理")} · ${t("测试连接通过")}` });
      if (stageScope === "asr" && id === draft.asrProvider) setTestedAsrRoute(asrRoute);
      if (stageScope === "cleanup" && id === draft.cleanupProvider) setTestedCleanupRoute(cleanupRoute);
    } catch (reason) {
      if (probeConfigurationRevisionRef.current !== requestRevision) return;
      setRowProbeFeedback({ provider: id, stage: stageScope, tone: "error", message: friendlySettingsError(reason, t) });
    } finally {
      setRowProbing(null);
      setRowProbingStage(null);
    }
  };

  const removeKey = async (id: ProviderId) => {
    setConfirmRemove(null);
    setRemoving(true);
    setRowProbeFeedback(null);
    try {
      await removeProviderKey(id);
      if (id === draft.asrProvider) setTestedAsrRoute(null);
      if (id === draft.cleanupProvider) setTestedCleanupRoute(null);
      setDraft((current) => ({ ...current, providerKeys: { ...current.providerKeys, [id]: "" } }));
      setStageFail({});
    } catch (reason) {
      setExpanded((current) => new Set([...current, id]));
      setRowProbeFeedback({ provider: id, tone: "error", message: `${t("密钥删除失败，请重试。")} ${friendlySettingsError(reason, t)}` });
    } finally {
      setRemoving(false);
    }
  };

  const readCredential = async (provider: ProviderId, purpose?: ProviderCredentialPurpose) => {
    if (!readProviderCredential || credentialReadRequest.current) return;
    const requestId = crypto.randomUUID();
    credentialReadRequest.current = requestId;
    clearCredentialReadBlurTimer();
    setCredentialReadPending(true);
    setCredentialReadTarget(provider);
    setCredentialReadStatus(null);
    setCredentialReadMessage(null);
    try {
      const result = purpose
        ? await readProviderCredential(provider, requestId, purpose)
        : await readProviderCredential(provider, requestId);
      if (credentialReadRequest.current !== requestId) return;
      setCredentialReadStatus(result.status);
      const messages = provider === "custom" ? {
        session_configured: { tone: "success" as const, text: t("自定义服务密钥已在本次打开期间读取；重启后可能需要再次授权。") },
        missing: { tone: "status" as const, text: t("当前应用钥匙串中没有可读取的自定义服务密钥。") },
        unavailable: { tone: "error" as const, text: t("钥匙串暂时不可读取；当前设置未更改。") },
        cancelled: { tone: "status" as const, text: t("已取消钥匙串授权；当前设置未更改。") },
        busy: { tone: "status" as const, text: t("请等待当前钥匙串读取完成。") },
        foreground_required: { tone: "error" as const, text: t("请在前台打开设置后重试。") },
        stale: { tone: "status" as const, text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") },
        unsupported: { tone: "error" as const, text: t("请先保存一个有效的自定义服务地址，并确保当前转写或整理路线使用自定义服务。") },
      } : provider === "groq" ? {
        session_configured: { tone: "success" as const, text: t("钥匙串密钥已在本次打开期间读取；重启后可能需要再次授权。") },
        missing: { tone: "status" as const, text: t("当前应用的钥匙串中没有可读取的 Groq 密钥。") },
        unavailable: { tone: "error" as const, text: t("钥匙串暂时不可读取；当前设置未更改。") },
        cancelled: { tone: "status" as const, text: t("已取消钥匙串授权；当前设置未更改。") },
        busy: { tone: "status" as const, text: t("请等待当前钥匙串读取完成。") },
        foreground_required: { tone: "error" as const, text: t("请在前台打开设置后重试。") },
        stale: { tone: "status" as const, text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") },
        unsupported: { tone: "error" as const, text: t("此操作仅适用于 Groq 密钥。") },
      } : {
        session_configured: { tone: "success" as const, text: t("服务商密钥已在本次打开期间读取；重启后可能需要再次授权。") },
        missing: { tone: "status" as const, text: t("当前应用钥匙串中没有可读取的服务商密钥。") },
        unavailable: { tone: "error" as const, text: t("钥匙串暂时不可读取；当前设置未更改。") },
        cancelled: { tone: "status" as const, text: t("已取消钥匙串授权；当前设置未更改。") },
        busy: { tone: "status" as const, text: t("请等待当前钥匙串读取完成。") },
        foreground_required: { tone: "error" as const, text: t("请在前台打开设置后重试。") },
        stale: { tone: "status" as const, text: t("设置已变化、页面已关闭或失去前台；请重新发起读取。") },
        unsupported: { tone: "error" as const, text: t("此服务商不支持钥匙串重新读取。") },
      } satisfies Record<ProviderCredentialReadResult["status"], { tone: "success" | "status" | "error"; text: string }>;
      setCredentialReadMessage(messages[result.status]);
    } catch {
      if (credentialReadRequest.current === requestId) {
        setCredentialReadStatus("unavailable");
        setCredentialReadTarget(provider);
        setCredentialReadMessage({ tone: "error", text: t("钥匙串暂时不可读取；当前设置未更改。") });
      }
    } finally {
      if (credentialReadRequest.current === requestId) {
        clearCredentialReadBlurTimer();
        credentialReadRequest.current = null;
        setCredentialReadPending(false);
      }
    }
  };

  const changeAsrProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "asr", value));
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
  };

  const changeDashscopeRegion = (value: string) => {
    if (value !== "beijing" && value !== "singapore") return;
    setDraft((current) => ({ ...current, dashscopeRegion: value }));
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
    setStageFail({});

  };

  const changeCleanupProvider = (value: string) => {
    if (!isProviderId(value)) return;
    setDraft(switchProvider(draft, "cleanup", value));
    setRowProbeFeedback(null);
    setProbeSuccess("");
  };

  const changeAsrModel = (asrModel: string) => {
    setDraft({ ...draft, asrModel });
    setRowProbeFeedback(null);
    setProbeSuccess("");
    setTestedAsrRoute(null);
  };

  const changeCleanupModel = (cleanupModel: string) => {
    setDraft({ ...draft, cleanupModel });
    setRowProbeFeedback(null);
    setStageFail((current) => ({ ...current, cleanup: undefined }));
    setProbeSuccess("");
  };

  const toggleExpanded = (id: ProviderId) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const asrOptions = providersFor("asr", true, true);
  const cleanupOptions = providersFor("llm", true, true);
  const onDeviceAsrModelDefinitions = providerById("on_device")?.asrModels ?? [];
  const reportedOnDeviceAsrModelOptions: ModelOption[] = onDeviceModels
    .filter((model) => model.download_available === true
      || (currentAsrProvider === "on_device" && model.id === savedAsrModel))
    .map((model) => {
      const definition = onDeviceAsrModelDefinitions.find((option) => option.value === model.id);
      return {
        value: model.id,
        label: model.label,
        note: model.download_available === true ? formatModelBytes(model.bytes) : t("旧配置保留；不提供新下载或选择"),
        retiredForNewSelection: model.download_available !== true,
        asrProfile: definition?.asrProfile,
      };
    });
  const onDeviceAsrModelOptions = reportedOnDeviceAsrModelOptions.length > 0
    ? reportedOnDeviceAsrModelOptions
    : onDeviceAsrModelDefinitions.filter((option) => (
      option.value === defaultModel("on_device", "asr")
      || (currentAsrProvider === "on_device" && option.value === savedAsrModel)
    ));
  const cloudAsrOptions = asrOptions.filter((provider) => provider.id !== "custom" && provider.id !== "on_device" && provider.id !== "local_whisper");
  const localAsrOptions = asrOptions.filter((provider) => provider.id === "on_device" || provider.id === "local_whisper");
  const customAsrOptions = asrOptions.filter((provider) => provider.id === "custom");
  const localCleanupModelOptions: ModelOption[] = selectableLocalCleanupModels.map((model) => ({
    value: model.id,
    label: model.label,
    note: formatModelBytes(model.bytes),
  }));
  const selectableMlxCleanupModels = mlxCleanupModels.filter((model) => model.selectable);
  const visibleLocalCleanupModels = localCleanupModels.filter((model) => (
    model.download_available === true
    || model.managed
    || model.partial_managed
    || Boolean(model.operation)
    || ["downloading", "canceling", "deleting"].includes(model.state)
    || (currentCleanupProvider === "on_device_llama_cpp" && model.id === savedCleanupModel)
    || (draft.cleanupProvider === "on_device_llama_cpp" && model.id === draft.cleanupModel)
  ));
  const localCleanupManagementRequired = visibleLocalCleanupModels.length > 0
    || localCleanupModelsReadError !== null
    || currentCleanupProvider === "on_device_llama_cpp"
    || draft.cleanupProvider === "on_device_llama_cpp";
  const visibleMlxCleanupModels = mlxCleanupModels.filter((model) => (
    model.download_available === true
    || model.managed
    || model.partial_managed
    || isMlxCleanupModelOperationActive(model)
    || (currentCleanupProvider === "on_device_mlx" && model.id === savedCleanupModel)
  ));
  const mlxCleanupModelOptions: ModelOption[] = [
    ...(providerById("on_device_mlx")?.llmModels ?? []),
    ...selectableMlxCleanupModels.map((model) => ({
      value: model.id,
      label: model.label,
      note: model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
    })),
  ];
  if (draft.cleanupProvider === "on_device_mlx" && draft.cleanupModel && !mlxCleanupModelOptions.some((option) => option.value === draft.cleanupModel)) {
    const unavailableModel = mlxCleanupModels.find((model) => model.id === draft.cleanupModel);
    mlxCleanupModelOptions.push({
      value: draft.cleanupModel,
      label: unavailableModel?.label ?? draft.cleanupModel,
      note: unavailableModel ? mlxCleanupModelStateCopy(unavailableModel, t) : t("模型状态暂不可用"),
      retiredForNewSelection: true,
    });
  }
  if (draft.cleanupProvider === "on_device_llama_cpp"
    && currentCleanupProvider === "on_device_llama_cpp"
    && draft.cleanupModel === savedCleanupModel
    && !localCleanupModelOptions.some((option) => option.value === draft.cleanupModel)) {
    const savedModel = localCleanupModels.find((model) => model.id === draft.cleanupModel);
    if (savedModel) {
      localCleanupModelOptions.push({
        value: savedModel.id,
        label: savedModel.label,
        note: localCleanupModelStateCopy(savedModel.state, savedModel.verified, t, savedModel.partial_managed),
        retiredForNewSelection: true,
      });
    }
  }
  const cloudCleanupOptions = cleanupOptions.filter((provider) => !["custom", "ollama", "on_device_llama_cpp", "on_device_mlx"].includes(provider.id));
  const localCleanupOptions = cleanupOptions.filter((provider) => ["ollama", "on_device_llama_cpp", "on_device_mlx"].includes(provider.id));
  const customCleanupOptions = cleanupOptions.filter((provider) => provider.id === "custom");
  const renderAsrOption = (provider: typeof PROVIDERS[number]) => (
    <option
      key={provider.id}
      value={provider.id}
      disabled={provider.id === "on_device" && onDeviceModels.length > 0 && !hasSupportedDeviceModel && draft.asrProvider !== "on_device"}
    >{provider.id === "custom" ? t("自定义语音转写") : provider.id === "on_device" ? t("本机模型") : provider.label}</option>
  );
  const renderCleanupOption = (provider: typeof PROVIDERS[number]) => (
    <option
      key={provider.id}
      value={provider.id}
      disabled={provider.id === "on_device_llama_cpp"
        && selectableLocalCleanupModels.length === 0
        && !(currentCleanupProvider === "on_device_llama_cpp" && draft.cleanupModel === savedCleanupModel)}
    >{provider.id === "custom" ? t("自定义 AI 整理") : provider.label}</option>
  );
  const customRecoveryPurpose: ProviderCredentialPurpose | null = settings.asr_provider === "custom"
    ? "asr"
    : settings.cleanup_enabled && settings.cleanup_provider === "custom"
      ? "cleanup"
      : null;
  const customReadState = settings.provider_keys?.custom?.keychain_state;
  const groqReadState = settings.provider_keys?.groq?.keychain_state;
  const groqLatestReadStatus = credentialReadTarget === "groq" ? credentialReadStatus : null;
  const customLatestReadStatus = credentialReadTarget === "custom" ? credentialReadStatus : null;
  const asrProfile = asrModelProfile(draft.asrProvider, draft.asrModel);
  const hasSupportedDeviceModel = onDeviceModels.some((model) => model.download_available === true && model.platform_supported);
  const savedOllamaBaseUrl = settings.ollama_base_url || providerById("ollama")?.defaultBaseUrl || "";
  const localCleanupRouteSaved = draft.ollamaBaseUrl.trim() === savedOllamaBaseUrl.trim();
  const savedAsrBlocked = settings.strict_offline_enabled === true && currentAsrProvider !== "on_device";
  const savedCleanupBlocked = settings.strict_offline_enabled === true && settings.cleanup_enabled
    && !(currentCleanupProvider === "ollama" && isLoopbackUrl(savedOllamaBaseUrl))
    && currentCleanupProvider !== "on_device_mlx"
    && !(currentCleanupProvider === "on_device_llama_cpp" && savedLocalCleanupModelReady);
  const asrFailLabel = failKind(stageFail.asr);
  const credentialStatus = (provider: ProviderId, tested: boolean) => tested
    ? { label: t("服务检查通过"), tone: "success" as const }
    : draft.providerKeys[provider]?.trim()
      ? { label: t("密钥待保存"), tone: "warning" as const }
      : providerConfigured(settings, provider)
        ? { label: t("凭据已保存 · 本次尚未检查"), tone: "neutral" as const }
        : { label: t("本机服务尚未检查"), tone: "neutral" as const };
  const asrStatus = draft.asrProvider === "on_device"
    ? {
        label: selectedOnDeviceModel?.runtime_status === "loading"
          ? t("本机模型正在加载")
          : onDeviceReady
          ? selectedOnDeviceModel?.loaded ? t("模型已加载") : t("本机运行时握手通过")
          : selectedOnDeviceModel?.platform_supported === false
            ? t("本机模型不适用于当前设备")
            : selectedOnDeviceModel?.state === "ready"
              ? onDeviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t)
              : onDeviceStateCopy(selectedOnDeviceModel?.state ?? "missing", t),
        tone: onDeviceReady && selectedOnDeviceModel?.runtime_status !== "loading" ? "success" as const : "warning" as const,
      }
    : settings.strict_offline_enabled && !selectedAsrIsLocal
    ? { label: t("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用"), tone: "warning" as const }
    : !canCoerceAsrLanguageOnSave
      ? {
          label: draftAsrLanguageSupport.allowedCodes.includes("zh")
            ? t("此模型要求固定使用中文或 English")
            : t("此模型仅支持英文；当前识别语言不兼容"),
          tone: "warning" as const,
        }
    : asrMissing
    ? { label: t("未配置密钥"), tone: "warning" as const }
    : draft.asrProvider === "soniox"
      ? { label: t(draft.providerKeys.soniox?.trim() || !providerConfigured(settings, "soniox") ? "密钥待保存" : "凭据已保存 · 听写时连接"), tone: draft.providerKeys.soniox?.trim() || !providerConfigured(settings, "soniox") ? "warning" as const : "neutral" as const }
    : asrFailLabel
      ? { label: asrFailLabel, tone: "error" as const }
    : credentialStatus(draft.asrProvider, testedAsrRoute === asrRoute);
  const cleanupFailLabel = failKind(stageFail.cleanup);
  const mlxCleanupRouteStatus = (() => {
    if (draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID) {
      if (!cleanupMlxPlatformSupported) {
        return { label: cleanupMlxPlatformUnsupported ? t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。") : t("无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。"), tone: "warning" as const };
      }
      if (cleanupModelStoreStatus?.operation) {
        return { label: cleanupModelStoreStatus.operation === "download" ? t("正在下载本机模型") : cleanupModelStoreStatus.operation === "import" ? t("正在导入本机模型") : t("正在删除本机模型"), tone: "warning" as const };
      }
      if (cleanupMlxInstalled && cleanupModelStoreStatus?.state === "ready") return { label: t("本机 MLX 模型已验证"), tone: "success" as const };
      if (cleanupMlxInstalled) return { label: t("已安装；首次使用时会重新校验"), tone: "neutral" as const };
      return { label: t("先导入或下载本机模型"), tone: "warning" as const };
    }
    if (!selectedMlxCleanupModel) {
      return mlxCleanupModelsReadError
        ? { label: t("本机整理模型状态读取失败"), tone: "error" as const }
        : { label: t("先下载并验证兼容的本机整理模型"), tone: "warning" as const };
    }
    if (selectedMlxCleanupModel.selectable) return { label: t("本机 MLX 模型文件已校验"), tone: "success" as const };
    if (isMlxCleanupModelOperationActive(selectedMlxCleanupModel)) {
      return { label: selectedMlxCleanupModel.operation === "verify"
        ? t("正在校验模型文件")
        : selectedMlxCleanupModel.operation === "delete" || selectedMlxCleanupModel.state === "deleting"
          ? t("正在删除")
          : t("正在下载 MLX 整理模型"), tone: "warning" as const };
    }
    if (!selectedMlxCleanupModel.compatible) {
      return { label: mlxCleanupCompatibilityCopy(selectedMlxCleanupModel.compatibility_error, t), tone: "warning" as const };
    }
    return { label: mlxCleanupModelStateCopy(selectedMlxCleanupModel, t), tone: "warning" as const };
  })();
  const cleanupStatus = !draftCleanupEnabled
    ? { label: t("关闭 · 只用本地规则"), tone: "unused" as const }
    : draft.cleanupProvider === "on_device_llama_cpp"
      ? draftLocalCleanupModelReady
        ? { label: t("本机整理模型已下载、校验并兼容"), tone: "success" as const }
        : selectedLocalCleanupModel?.operation === "download" || selectedLocalCleanupModel?.state === "downloading"
          ? { label: t("正在下载本机整理模型"), tone: "warning" as const }
          : selectedLocalCleanupModel?.compatible === false
            ? { label: localCleanupCompatibilityCopy(selectedLocalCleanupModel.compatibility_error, t), tone: "warning" as const }
            : selectedLocalCleanupModel
              ? { label: localCleanupModelStateCopy(selectedLocalCleanupModel.state, selectedLocalCleanupModel.verified, t, selectedLocalCleanupModel.partial_managed), tone: "warning" as const }
              : localCleanupModelsReadError
                ? { label: t("本机整理模型状态读取失败"), tone: "error" as const }
                : { label: selectableLocalCleanupModels.length > 0 ? t("请选择已下载并验证的模型") : t("先下载并验证兼容的本机整理模型"), tone: "warning" as const }
    : draft.cleanupProvider === "on_device_mlx"
      ? mlxCleanupRouteStatus
    : settings.strict_offline_enabled && !["ollama", "on_device_llama_cpp", "on_device_mlx"].includes(draft.cleanupProvider)
      ? { label: t("严格离线模式会阻止此云端整理服务"), tone: "warning" as const }
      : draft.cleanupProvider === "ollama" && localCleanupStatus && !localCleanupStatus.available
        ? { label: t("本机整理服务不可用"), tone: "warning" as const }
        : draft.asrProvider === "assemblyai"
          ? cleanupMissing
            ? { label: t("120 秒内的融合 Dictation 候选不需要此密钥；长录音和候选失败后可选用共同整理，无密钥时使用本地回退"), tone: "warning" as const }
            : cleanupFailLabel
              ? { label: cleanupFailLabel, tone: "error" as const }
              : credentialStatus(draft.cleanupProvider, testedCleanupRoute === cleanupRoute)
          : cleanupMissing
            ? { label: t("未配置密钥"), tone: "warning" as const }
            : cleanupFailLabel
              ? { label: cleanupFailLabel, tone: "error" as const }
              : credentialStatus(draft.cleanupProvider, testedCleanupRoute === cleanupRoute);

  const activePage = ENGINE_PAGES.find((item) => item.id === page) ?? ENGINE_PAGES[0];
  const onDeviceModelActivationBlock = (model: OnDeviceModelStatus): string | null => {
    if (model.state !== "ready" || model.inference_ready !== true || model.platform_supported !== true
      || model.runtime_status === "loading" || model.runtime_status === "unsupported_platform"
      || model.runtime_status === "runtime_failed" || model.runtime_status === "sidecar_missing") {
      return t("此模型尚未校验就绪，或本机运行时不可用。");
    }
    if (onDeviceMemoryCompatibilityCopy(model.memory_compatibility_error, t)) {
      return t("此模型超出当前设备的内存准入预算。");
    }
    if (!hasSupportedAsrLanguage(asrModelProfile("on_device", model.id), settings.language, "on_device", model.id)
      && !modelLanguageSupport("on_device", model.id).autoAllowed) {
      return t("当前识别语言不适用于此模型；请先调整语言设置。");
    }
    if (settings.strict_offline_enabled === true && settings.cleanup_enabled && !savedCleanupIsLocal) {
      return t("严格离线模式要求整理路线也使用本机模型。");
    }
    return null;
  };
  return (
    <SettingsShell className={showEngineFooter ? "vf-settings-shell--flush-bottom" : ""}>
      <div className={`vf-engine-settings${showEngineFooter ? " vf-engine-settings--with-footer" : ""}`} onFocusCapture={(event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) return;
        window.requestAnimationFrame(() => {
          if (document.activeElement === target) keepEngineControlInsideRails(target);
        });
      }}>
      <SettingsPageHeader
        title={t(activePage.label)}
        description={t(activePage.description)}
        saveHint={page === "service" ? t("转写和整理按阶段显示当前路线与检查状态；草稿在本窗口切页后保留。") : undefined}
      />
      {settings.strict_offline_enabled && (
        <div role="status" className="flex flex-wrap items-center justify-between gap-2 border-b border-warning/30 bg-warning/5 px-5 py-2 text-xs leading-5 text-warning-ink">
          <span>{t("严格离线模式已开启")}</span>
          <button type="button" className={compactButtonClass} onClick={() => navigateToPage("advanced", "strict-offline-toggle")}>{t("查看策略")}</button>
        </div>
      )}
      <EngineServiceTab
        page={page}
        pendingChanges={pendingChanges}
        probing={probing}
        rowProbing={rowProbing}
        rowProbingStage={rowProbingStage}
        removing={removing}
        strictOfflineSaving={strictOfflineSaving}
        settings={settings}
        draft={draft}
        draftCleanupEnabled={draftCleanupEnabled}
        onDraftCleanupEnabledChange={setDraftCleanupEnabled}
        currentAsrProvider={currentAsrProvider}
        currentCleanupProvider={currentCleanupProvider}
        savedAsrBlocked={savedAsrBlocked}
        savedCleanupBlocked={savedCleanupBlocked}
        asrStatus={asrStatus}
        cleanupStatus={cleanupStatus}
        rowProbeFeedback={rowProbeFeedback}
        probeSuccess={probeSuccess}
        onProbeSuccessClear={() => setProbeSuccess("")}
        onDeviceModels={onDeviceModels}
        selectedOnDeviceModel={selectedOnDeviceModel}
        onDeviceModelsLoading={onDeviceModelsLoading}
        onDeviceModelReadError={onDeviceModelReadError}
        asrProfile={asrProfile}
        cloudAsrOptions={cloudAsrOptions}
        localAsrOptions={localAsrOptions}
        customAsrOptions={customAsrOptions}
        cloudCleanupOptions={cloudCleanupOptions}
        localCleanupOptions={localCleanupOptions}
        customCleanupOptions={customCleanupOptions}
        renderAsrOption={renderAsrOption}
        renderCleanupOption={renderCleanupOption}
        onChangeAsrProvider={changeAsrProvider}
        onChangeAsrModel={changeAsrModel}
        onChangeCleanupProvider={changeCleanupProvider}
        onChangeCleanupModel={changeCleanupModel}
        onDeviceAsrModelOptions={onDeviceAsrModelOptions}
        localCleanupModelOptions={localCleanupModelOptions}
        mlxCleanupModelOptions={mlxCleanupModelOptions}
        onNavigateToPage={navigateToPage}
        onTestConnection={testConnection}
        strictOfflineBlocksProbe={strictOfflineBlocksProbe}
        selectedLocalCleanupModel={selectedLocalCleanupModel}
        localCleanupModelsReadError={localCleanupModelsReadError}
        localCleanupStatus={localCleanupStatus}
        localCleanupError={localCleanupError}
        cleanupModelStoreStatus={cleanupModelStoreStatus}
        selectedMlxCleanupModel={selectedMlxCleanupModel}
        selectedMlxCleanupLanguage={selectedMlxCleanupLanguage}
        mlxCleanupModelsReadError={mlxCleanupModelsReadError}
        mlxCleanupManageTarget={mlxCleanupManageTarget}
        customParametersOk={customParametersParse.ok}
        draftLocalCleanupModelReady={draftLocalCleanupModelReady}
      />
      <EngineApiKeyTab
        page={page}
        settings={settings}
        draft={draft}
        savedDraft={savedDraft}
        setDraft={setDraft}
        draftCleanupEnabled={draftCleanupEnabled}
        currentAsrProvider={currentAsrProvider}
        currentCleanupProvider={currentCleanupProvider}
        stageFail={stageFail}
        setStageFail={setStageFail}
        failKind={failKind}
        expanded={expanded}
        setExpanded={setExpanded}
        onToggleExpanded={toggleExpanded}
        rowProbeFeedback={rowProbeFeedback}
        onClearRowProbeFeedback={() => setRowProbeFeedback(null)}
        onClearProbeSuccess={() => setProbeSuccess("")}
        onResetTestedAsrRoute={() => setTestedAsrRoute(null)}
        onResetTestedCleanupRoute={() => setTestedCleanupRoute(null)}
        testedAsrRoute={testedAsrRoute}
        testedCleanupRoute={testedCleanupRoute}
        asrRoute={asrRoute}
        cleanupRoute={cleanupRoute}
        onDeviceReady={onDeviceReady}
        probing={probing}
        rowProbing={rowProbing}
        rowProbingStage={rowProbingStage}
        removing={removing}
        strictOfflineSaving={strictOfflineSaving}
        strictOfflineBlocksProbe={strictOfflineBlocksProbe}
        customParametersOk={customParametersParse.ok}
        customRecoveryPurpose={customRecoveryPurpose}
        customReadState={customReadState}
        groqReadState={groqReadState}
        customLatestReadStatus={customLatestReadStatus}
        groqLatestReadStatus={groqLatestReadStatus}
        credentialReadPending={credentialReadPending}
        credentialReadTarget={credentialReadTarget}
        credentialReadMessage={credentialReadMessage}
        hasReadProviderCredential={Boolean(readProviderCredential)}
        onReadCredential={readCredential}
        onRequestRemoveKey={setConfirmRemove}
        onTestConnection={testConnection}
        onNavigateToPage={navigateToPage}
      />
      <LocalModelsTab
        page={page}
        probing={probing}
        rowProbing={rowProbing}
        removing={removing}
        strictOfflineSaving={strictOfflineSaving}
        settings={settings}
        draft={draft}
        draftCleanupEnabled={draftCleanupEnabled}
        currentAsrProvider={currentAsrProvider}
        currentCleanupProvider={currentCleanupProvider}
        savedAsrModel={savedAsrModel}
        savedCleanupModel={savedCleanupModel}
        deviceMemoryBudget={deviceMemoryBudget}
        onDeviceModels={onDeviceModels}
        onDeviceModelsLoading={onDeviceModelsLoading}
        onDeviceModelReadError={onDeviceModelReadError}
        onDeviceActionId={onDeviceActionId}
        onDeviceActionError={onDeviceActionError}
        legacyModelsOpen={legacyModelsOpen}
        onLegacyModelsOpenChange={setLegacyModelsOpen}
        legacyModel={legacyModel}
        legacyModelSavedSelected={legacyModelSavedSelected}
        legacyModelDraftSelected={legacyModelDraftSelected}
        legacyModelShouldOpen={legacyModelShouldOpen}
        onRefreshOnDeviceModels={refreshOnDeviceModels}
        onRunOnDeviceAction={runOnDeviceAction}
        onCommitOnDeviceModel={commitOnDeviceModel}
        onRequestDeleteModel={setConfirmDeleteModel}
        onDeviceModelActivationBlock={onDeviceModelActivationBlock}
        visibleMlxCleanupModels={visibleMlxCleanupModels}
        mlxCleanupModelsLoading={mlxCleanupModelsLoading}
        mlxCleanupModelsReadError={mlxCleanupModelsReadError}
        mlxCleanupModelBusy={mlxCleanupModelBusy}
        mlxCleanupModelActionErrors={mlxCleanupModelActionErrors}
        onRefreshMlxCleanupModels={refreshMlxCleanupModels}
        onRunMlxCleanupModelAction={runMlxCleanupModelAction}
        onCommitMlxCleanupModel={commitMlxCleanupModel}
        onRequestDeleteMlxCleanupModel={setConfirmDeleteMlxCleanupModel}
        cleanupModelStoreStatus={cleanupModelStoreStatus}
        cleanupModelStoreBusy={cleanupModelStoreBusy}
        cleanupModelStoreReadError={cleanupModelStoreReadError}
        cleanupModelStoreActionError={cleanupModelStoreActionError}
        cleanupMlxPlatformSupported={cleanupMlxPlatformSupported}
        cleanupModelStoreIdle={cleanupModelStoreIdle}
        cleanupModelStoreNeedsRefresh={cleanupModelStoreNeedsRefresh}
        legacyMlxActivationBlock={legacyMlxActivationBlock}
        onRunCleanupModelStoreAction={runCleanupModelStoreAction}
        onRefreshCleanupModelStore={refreshCleanupModelStore}
        onCommitLegacyMlxModel={commitLegacyMlxModel}
        onRequestDeleteCleanupModel={() => setConfirmDeleteCleanupModel(true)}
        localCleanupManagementRequired={localCleanupManagementRequired}
        visibleLocalCleanupModels={visibleLocalCleanupModels}
        localCleanupModelsLoading={localCleanupModelsLoading}
        localCleanupModelsReadError={localCleanupModelsReadError}
        localCleanupModelBusy={localCleanupModelBusy}
        localCleanupModelActionErrors={localCleanupModelActionErrors}
        onRefreshLocalCleanupModels={refreshLocalCleanupModels}
        onRunLocalCleanupModelAction={runLocalCleanupModelAction}
        onCommitLocalCleanupModel={commitLocalCleanupModel}
        onRequestDeleteLocalCleanupModel={setConfirmDeleteLocalCleanupModel}
        cleanupActivationBlock={cleanupActivationBlock}
      />
      <EngineAdvancedTab
        page={page}
        pendingChanges={pendingChanges}
        saveBlock={saveBlock}
        onNavigateToPage={navigateToPage}
        settings={settings}
        draft={draft}
        setDraft={setDraft}
        draftCleanupEnabled={draftCleanupEnabled}
        probing={probing}
        rowProbing={rowProbing}
        removing={removing}
        strictOfflineSaving={strictOfflineSaving}
        strictOfflineSaveError={strictOfflineSaveError}
        onUpdateStrictOfflineMode={updateStrictOfflineMode}
        advancedConnectionsOpen={advancedConnectionsOpen}
        onAdvancedConnectionsOpenChange={setAdvancedConnectionsOpen}
        onClearRowProbeFeedback={() => setRowProbeFeedback(null)}
        onClearProbeSuccess={() => setProbeSuccess("")}
        onResetTestedAsrRoute={() => setTestedAsrRoute(null)}
        onResetTestedCleanupRoute={() => setTestedCleanupRoute(null)}
        onChangeDashscopeRegion={changeDashscopeRegion}
        localCleanupChecking={localCleanupChecking}
        localCleanupRouteSaved={localCleanupRouteSaved}
        localCleanupStatus={localCleanupStatus}
        onResetLocalCleanupStatus={() => setLocalCleanupStatus(null)}
        localCleanupError={localCleanupError}
        onCheckLocalCleanupStatus={checkLocalCleanupStatus}
        onChooseLocalCleanup={chooseLocalCleanup}
        metricsError={metricsError}
        onMetricsErrorChange={setMetricsError}
      />
      {showEngineFooter && <div className="vf-engine-footer vf-save-bar flex flex-wrap items-center justify-end gap-3 px-5 py-4">
        {pendingChanges && (
          <button type="button" disabled={probing || rowProbing !== null || removing || strictOfflineSaving} onClick={() => {
            setDraft(draftFromSettings(settings));
            setDraftCleanupEnabled(settings.cleanup_enabled);
            setStageFail({});
            setCommitError(null);
            setProbeSuccess("");
            setTestedAsrRoute(null);
            setTestedCleanupRoute(null);
            navigateToPage("service", "engine-asr-provider");
          }} className={ghostButtonClass}>{t("放弃更改")}</button>
        )}
        {saveBlock && pendingChanges && <p role="status" className="flex w-full flex-wrap items-center gap-2 text-xs leading-5 text-warning-ink"><span>{saveBlock.message}</span><button type="button" className={compactButtonClass} onClick={() => navigateToPage(saveBlock.page, saveBlock.target)}>{saveBlock.action}</button></p>}
        {commitError && <p role="alert" className="w-full text-xs leading-5 text-error-ink">{commitError}</p>}
        <button type="button" onClick={() => void applyDraft()} aria-busy={probing} disabled={probing || rowProbing !== null || removing || strictOfflineSaving || !ready} className={`${buttonClass} min-w-36`}>
          {probing ? t("保存中…") : t("保存并使用")}
        </button>
      </div>}
      </div>
      <ConfirmDialog
        open={confirmRemove !== null}
        title={t("删除密钥")}
        description={t("删除后需要重新填写这个服务商的密钥。确定删除吗？")}
        confirmLabel={t("删除密钥")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmRemove(null)}
        onConfirm={() => confirmRemove && void removeKey(confirmRemove)}
      />
      <ConfirmDialog
        open={confirmDeleteModel !== null}
        title={t("删除本机模型")}
        description={t("这会从本机删除所选模型文件。下载仍需你之后手动启动。确定删除吗？")}
        confirmLabel={t("删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteModel(null)}
        onConfirm={() => {
          if (!confirmDeleteModel) return;
          const id = confirmDeleteModel;
          setConfirmDeleteModel(null);
          void runOnDeviceAction(id, "delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteCleanupModel}
        title={t("删除 VoiceFlow MLX 模型")}
        description={t("VoiceFlow 会先请求 cleanup runtime 卸载模型，再删除它管理的文件。原始导入文件夹不会更改。确定删除吗？")}
        confirmLabel={t("删除模型文件")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteCleanupModel(false)}
        onConfirm={() => {
          setConfirmDeleteCleanupModel(false);
          void runCleanupModelStoreAction("delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteMlxCleanupModel !== null}
        title={t(cleaningOnlyPartialMlxCleanupModel ? "清理未完成模型文件" : "删除本机整理模型")}
        description={t(cleaningOnlyPartialMlxCleanupModel
          ? "这会从本机删除此模型尚未完成的下载文件；之后可以重新启动下载。确定清理吗？"
          : "这会从本机删除 VoiceFlow 管理的 MLX 整理模型文件；之后可以重新下载。确定删除吗？")}
        confirmLabel={t(cleaningOnlyPartialMlxCleanupModel ? "清理文件" : "删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteMlxCleanupModel(null)}
        onConfirm={() => {
          const id = confirmDeleteMlxCleanupModel;
          if (!id) return;
          const savedSelected = settings.cleanup_enabled
            && currentCleanupProvider === "on_device_mlx"
            && savedCleanupModel === id;
          if (savedSelected) {
            setMlxCleanupModelActionErrors((current) => ({
              ...current,
              [id]: t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。"),
            }));
            setConfirmDeleteMlxCleanupModel(null);
            return;
          }
          setConfirmDeleteMlxCleanupModel(null);
          void runMlxCleanupModelAction(id, "delete");
        }}
      />
      <ConfirmDialog
        open={confirmDeleteLocalCleanupModel !== null}
        title={t(cleaningOnlyPartialModel ? "清理未完成模型文件" : "删除本机整理模型")}
        description={t(cleaningOnlyPartialModel
          ? "这会从本机删除此模型尚未完成的下载文件；之后可以重新启动下载。确定清理吗？"
          : "这会从本机删除 VoiceFlow 管理的模型文件；之后需要你再次手动下载。确定删除吗？")}
        confirmLabel={t(cleaningOnlyPartialModel ? "清理文件" : "删除模型")}
        cancelLabel={t("取消")}
        onCancel={() => setConfirmDeleteLocalCleanupModel(null)}
        onConfirm={() => {
          const id = confirmDeleteLocalCleanupModel;
          if (!id) return;
          const savedSelected = settings.cleanup_enabled
            && currentCleanupProvider === "on_device_llama_cpp"
            && savedCleanupModel === id;
          const draftSelected = draftCleanupEnabled
            && draft.cleanupProvider === "on_device_llama_cpp"
            && draft.cleanupModel === id;
          if (savedSelected || draftSelected) {
            setLocalCleanupModelActionErrors((current) => ({
              ...current,
              [id]: t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。"),
            }));
            setConfirmDeleteLocalCleanupModel(null);
            return;
          }
          setConfirmDeleteLocalCleanupModel(null);
          void runLocalCleanupModelAction(id, "delete");
        }}
      />
    </SettingsShell>
  );
}
