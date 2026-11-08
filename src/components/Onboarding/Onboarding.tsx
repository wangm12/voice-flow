import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { cleanupIntentForRoute, cleanupIntentPatch, initialCleanupIntent, type OnboardingCleanupIntent } from "../../lib/onboardingCleanup";
import { CleanupGuide } from "./CleanupGuide";
import { DataChoices, type OnboardingSettingsPage } from "./DataChoices";
import type { MicrophoneStatus } from "../MicrophoneCheck";
import { usePermissionGuide } from "./usePermissionGuide";
import { useOnboardingTrialLease } from "./useOnboardingTrialLease";
import { currentReadiness, isNativeGeneration, matchesTrialIdentity, matchesTrialMetadata, readinessFact, parseOnboardingPermissions, sameOnboardingPermissions, invalidatePermissionReadiness, type OnboardingPermissions, type PermissionReadback, type ReadinessFact, type TrialReceipt } from "../../lib/onboardingReadiness";
import { type ActivationMode } from "../../lib/activationCopy";
import { DEFAULT_DICTATION_HOTKEY, DEFAULT_SELECTED_ACTION_HOTKEY, formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { EngineConfigStep } from "./EngineConfigStep";
import { PermissionsStep } from "./PermissionsStep";
import { OnboardingFooter } from "./OnboardingFooter";
import { OnboardingSidebar } from "./OnboardingSidebar";
import { WelcomeStep } from "./WelcomeStep";
import { ExternalDeliveryTrial, type ExternalReceipt } from "./ExternalDeliveryTrial";
import type { TrialLease } from "../../lib/onboardingReadiness";
import { FinishStep } from "./FinishStep";
import { HotkeyStep } from "./HotkeyStep";
import { AnimatedContent } from "../ReactBits/AnimatedContent";
import { useI18n } from "../../lib/i18n";
import { friendlySettingsError } from "../../lib/settingsError";
import { ghostButtonClass } from "../../lib/theme";
import { draftFromSettings, hasSupportedAsrLanguage, probePayload } from "../../lib/engineWizard";
import {
  asrModelProfile,
  dashscopeEndpointForRegion,
  dashscopeRegionFromEndpoint,
  defaultModel,
  isProviderId,
  isLoopbackUrl,
  providerById,
  type DashscopeRegion,
  type ProviderId,
} from "../../lib/providers";
import type { OnDeviceModelStatus, Settings as EngineSettingsSnapshot } from "../../types/settings";

type Permissions = OnboardingPermissions;
type ProbeStage = { ok: boolean; error_kind?: string | null };
type ProbeResult = { asr: ProbeStage; cleanup: ProbeStage };
type Settings = {
  api_key_configured?: boolean;
  api_key_hint?: string | null;
  asr_provider?: string;
  asr_model?: string;
  asr_api_key_configured?: boolean;
  asr_api_key_hint?: string | null;
  asr_base_url?: string;
  cleanup_provider?: ProviderId;
  cleanup_model?: string;
  cleanup_base_url?: string;
  cleanup_enabled?: boolean;
  custom_base_url?: string;
  custom_asr?: boolean;
  custom_llm?: boolean;
  ollama_base_url?: string;
  local_whisper_base_url?: string;
  provider_keys?: Partial<Record<ProviderId, { configured: boolean; hint?: string | null }>>;
  language?: string;
  hotkey?: string;
  activation_mode?: string;
  selected_action_hotkey?: string;
  hotkey_error?: string | null;
  ui_language?: "system" | "zh" | "en";
  onboarded?: boolean;
  [key: string]: unknown;
};

const DICTATION_STEP = 3;
const SELECTED_ACTION_STEP = 4;
const FINISH_STEP = 5;

function isHotkeyTrialStep(step: number): boolean {
  return step === DICTATION_STEP || step === SELECTED_ACTION_STEP;
}

function savedAsrProvider(settings: Settings): ProviderId {
  const value = String(settings.asr_provider ?? "groq");
  if (!isProviderId(value)) return "groq";
  if (value === "custom" && settings.custom_asr === false) return "groq";
  if (!providerById(value)?.capabilities.includes("asr") && value !== "custom") return "groq";
  return value;
}

function asrEndpointPatch(provider: ProviderId, region: DashscopeRegion): Record<string, string> {
  if (provider === "assemblyai") {
    return { asr_base_url: providerById(provider)?.defaultBaseUrl ?? "https://dictation.assemblyai.com" };
  }
  if (provider === "dashscope") {
    return { asr_base_url: dashscopeEndpointForRegion(region) };
  }
  return {};
}

export function Onboarding({
  settings,
  onFinish,
  onSkipToSettings,
}: {
  settings: Settings;
  onFinish: (settings: Settings) => void;
  onSkipToSettings: (page?: OnboardingSettingsPage) => void;
}) {
  const { t } = useI18n();
  const [step, setStep] = useState(0);
  const [savedSettings, setSavedSettings] = useState(settings);
  const [inputDevice, setInputDevice] = useState(String(settings.input_device ?? ""));
  const [guideOpen, setGuideOpen] = useState(false);
  const [facts, setFacts] = useState<ReadinessFact[]>([]);
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const permissionReadback = useRef<PermissionReadback>({ state: "unknown", permissions: null, source_value: null, reason: "unavailable" });
  const permissionRequest = useRef(0);
  const permissionOwnerActive = useRef(false);
  // A presentation/readback window only: never a native configuration identity.
  // Reusing the existing lease key makes old native trial IDs ineligible.
  const [permissionWindow, setPermissionWindow] = useState(0);
  const [externalOpen, setExternalOpen] = useState(false);
  const externalContinuation = useRef<TrialLease | null>(null);
  const setExternalContinuation = useCallback((lease: TrialLease | null) => { externalContinuation.current = lease; }, []);
  const [sessionId] = useState(() => crypto.randomUUID());
  const [configurationRevision, setConfigurationRevision] = useState(0);
  const draftKey = `${sessionId}:${configurationRevision}`;
  const generationRef = useRef(draftKey);
  generationRef.current = draftKey;
  const [nativeGeneration, setNativeGeneration] = useState<string | null>(null);
  const configurationGeneration = nativeGeneration ?? "";
  const { lease: trialLease, releaseAll: releaseTrial, releaseFailed } = useOnboardingTrialLease(isHotkeyTrialStep(step) && permissions?.microphone === true ? `${draftKey}:${step}:permission-window:${permissionWindow}` : null, setNativeGeneration);
  const trialReady = trialLease !== null;
  const currentTrialLease = useRef(trialLease);
  currentTrialLease.current = trialLease;
  const recordingGeneration = useRef<string | null>(null);
  const [hotkeyRegistered, setHotkeyRegistered] = useState(false);
  const invalidateConfiguration = useCallback(() => {
    externalContinuation.current = null;
    setConfigurationRevision((value) => value + 1); setHotkeyRegistered(false); setNativeGeneration(null); setFacts([]); recordingGeneration.current = null;
  }, []);
  const recordFact = useCallback((fact: ReadinessFact) => {
    setFacts((current) => [...current.filter((old) => old.stage !== fact.stage), fact]);
  }, []);
  const onMicrophoneStatus = (status: MicrophoneStatus) => {
    if (!nativeGeneration || permissionReadback.current.permissions?.microphone !== true) return;
    recordFact(readinessFact("microphone_frames", status.received_frames ? "passed" : status.state === "error" ? "failed" : "not_tested", `microphone-check://state:${status.session_id}`, configurationGeneration, sessionId));
    if (status.device_name) setSavedSettings((current) => ({ ...current, effective_input_device: status.device_name }));
  };
  const leaveForSettings = async (page?: OnboardingSettingsPage) => {
    try { await releaseOwnedTrial(); onSkipToSettings(page); }
    catch { setHotkeyError(t("无法结束试用，请取消录音后重试。")); }
  };
  const contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => { if (contentRef.current) contentRef.current.scrollTop = 0; }, [step]);
  const { status: nativeGuide, failed: guideFailed, reopen: reopenPermissionGuide } = usePermissionGuide(step === 1 && guideOpen && permissions?.accessibility !== true);
  const [asrProvider, setAsrProvider] = useState<ProviderId>(() => savedAsrProvider(settings));
  const [cleanupIntent, setCleanupIntent] = useState<OnboardingCleanupIntent>(() => initialCleanupIntent(settings.onboarded === true, savedAsrProvider(settings)));
  const [asrModel, setAsrModel] = useState(() => {
    const provider = savedAsrProvider(settings);
    return provider === settings.asr_provider
      ? String(settings.asr_model || defaultModel(provider, "asr"))
      : defaultModel(provider, "asr");
  });
  const [language, setLanguage] = useState(() => String(settings.language ?? "auto"));
  const [dashscopeRegion, setDashscopeRegion] = useState<DashscopeRegion>(() => dashscopeRegionFromEndpoint(settings.asr_base_url));
  const [onDeviceModels, setOnDeviceModels] = useState<OnDeviceModelStatus[]>([]);
  const [modelReceipt, setModelReceipt] = useState<OnDeviceModelStatus | null>(null);
  const [onDeviceActionId, setOnDeviceActionId] = useState<string | null>(null);
  const [onDeviceActionError, setOnDeviceActionError] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [hotkey, setHotkey] = useState(String(settings.hotkey ?? DEFAULT_DICTATION_HOTKEY));
  const [activationMode, setActivationMode] = useState<ActivationMode | string>(
    String(settings.activation_mode ?? "tap"),
  );
  const [selectedActionHotkey, setSelectedActionHotkey] = useState(
    String(settings.selected_action_hotkey ?? DEFAULT_SELECTED_ACTION_HOTKEY).trim() || DEFAULT_SELECTED_ACTION_HOTKEY,
  );
  const [validating, setValidating] = useState(false);
  const [valid, setValid] = useState<string | null>(null);
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [modeSaving, setModeSaving] = useState(false);
  const [hotkeyError, setHotkeyErrorState] = useState<string | null>(settings.hotkey_error ?? null);
  const [trialErrorDetail, setTrialErrorDetail] = useState<string | null>(null);
  const setHotkeyError = useCallback((error: string | null) => {
    setHotkeyErrorState(error);
    if (error) setTrialErrorDetail(null);
  }, []);
  useEffect(() => {
    if (!hotkeyError) setTrialErrorDetail(null);
  }, [hotkeyError]);
  const [submitting, setSubmitting] = useState(false);
  const [selectedActionTrial, setSelectedActionTrial] = useState(false);
  const selectedOnDeviceModelIdRef = useRef(asrModel);
  const onDeviceComponentActiveRef = useRef(false);
  const onDeviceStatusRefreshSequenceRef = useRef(0);
  const onDeviceStatusRequestRef = useRef<{
    sequence: number;
    promise: Promise<OnDeviceModelStatus[]>;
  } | null>(null);
  const configuredAsrKey = asrProvider === "groq"
    ? Boolean(savedSettings.api_key_configured)
    : Boolean(savedSettings.provider_keys?.[asrProvider]?.configured || (asrProvider === "custom" && savedSettings.asr_api_key_configured));
  const asrKeyHint = asrProvider === "groq"
    ? String(settings.api_key_hint ?? "")
    : String(settings.provider_keys?.[asrProvider]?.hint ?? (asrProvider === "custom" ? settings.asr_api_key_hint : "") ?? "");
  const selectedOnDeviceModel = onDeviceModels.find((model) => model.id === asrModel);
  const onDeviceReady = asrProvider === "on_device" && selectedOnDeviceModel?.inference_ready === true;
  const asrConfigured = Boolean(asrModel.trim()) && (Boolean(key.trim()) || configuredAsrKey || Boolean(providerById(asrProvider)?.allowsEmptyKey && isLoopbackUrl(String(asrProvider === "local_whisper" ? savedSettings.local_whisper_base_url ?? providerById(asrProvider)?.defaultBaseUrl : savedSettings.custom_base_url ?? savedSettings.asr_base_url ?? providerById(asrProvider)?.defaultBaseUrl))));
  const currentAsrLanguageSupported = hasSupportedAsrLanguage(asrModelProfile(asrProvider, asrModel), language);
  const strictOfflineEnabled = settings.strict_offline_enabled === true;
  const strictOfflineAsrAllowed = asrProvider === "on_device";
  const strictOfflineBlocked = strictOfflineEnabled && !strictOfflineAsrAllowed;
  const releaseOwnedTrial = async () => {
    await releaseTrial();
    setRecording(false); setProcessing(false); recordingGeneration.current = null;
  };
  useEffect(() => { setRecording(false); setProcessing(false); recordingGeneration.current = null; }, [trialLease?.trial_id]);

  useEffect(() => {
    let active = true;
    const readGeneration = async () => {
      try {
        const value = await invoke<unknown>("get_onboarding_configuration_generation");
        if (active && isNativeGeneration(value)) setNativeGeneration(value);
      } catch { if (active) setNativeGeneration(null); }
    };
    void readGeneration();
    return () => { active = false; };
  }, [configurationRevision]);

  useEffect(() => {
    let active = true;
    const subscription = listen<Settings>("settings://changed", (event) => {
      if (!active) return;
      setSavedSettings(event.payload);
      invalidateConfiguration();
      setValid(null);
    });
    return () => { active = false; void subscription.then((unlisten) => unlisten()).catch(() => undefined); };
  }, [invalidateConfiguration]);

  const acceptPermissionReadback = useCallback((value: unknown, windowBoundary: "left" | "reentered" | null = null) => {
    const observation = parseOnboardingPermissions(value);
    const next = observation.permissions;
    if (windowBoundary || !sameOnboardingPermissions(permissionReadback.current.permissions, next)) {
      externalContinuation.current = null;
      currentTrialLease.current = null;
      recordingGeneration.current = null;
      setFacts((current) => invalidatePermissionReadiness(current, {
        source: windowBoundary ? "onboarding_window" : "check_permissions",
        reason: windowBoundary ?? (observation.state === "unknown" ? observation.reason : "permission_changed"),
      }));
      setPermissionWindow((window) => window + 1);
      setRecording(false); setProcessing(false);
    }
    permissionReadback.current = observation;
    setPermissions(next);
    return next;
  }, []);
  const refreshPermissions = useCallback(async () => {
    if (!permissionOwnerActive.current) return;
    const request = ++permissionRequest.current;
    setSettingsError(null);
    try {
      const value = await invoke<unknown>("check_permissions");
      if (request !== permissionRequest.current) return;
      const accepted = acceptPermissionReadback(value);
      if (!accepted) setSettingsError(t("无法检测系统权限，请重试"));
      return accepted;
    } catch {
      if (request !== permissionRequest.current) return;
      acceptPermissionReadback(null);
      setSettingsError(t("无法检测系统权限，请重试"));
      return null;
    }
  }, [acceptPermissionReadback, t]);

  useEffect(() => {
    permissionOwnerActive.current = true;
    let active = true;
    let pending = false;
    let attempts = 0;
    const poll = async () => {
      if (!active || pending || document.visibilityState !== "visible" || attempts >= 60) return;
      pending = true; attempts += 1;
      try { await refreshPermissions(); } finally { pending = false; }
    };
    const reenter = () => {
      ++permissionRequest.current; // A prior window's in-flight readback is stale.
      // Only a native-prepared controlled external continuation may cross
      // this expected focus edge. Actual unknown/revoked readback still uses
      // acceptPermissionReadback and rotates the existing permission window.
      if (!externalContinuation.current) acceptPermissionReadback(null, "reentered");
      attempts = 0;
      if (document.visibilityState === "visible") void poll();
    };
    const leave = () => {
      ++permissionRequest.current;
      if (externalContinuation.current) {
        setFacts((current) => invalidatePermissionReadiness(current, { source: "onboarding_window", reason: "controlled_external_switch" }));
      } else acceptPermissionReadback(null, "left");
    };
    const visibility = () => { if (document.visibilityState === "visible") reenter(); else leave(); };
    void poll();
    const timer = window.setInterval(() => void poll(), 1000);
    window.addEventListener("focus", reenter);
    window.addEventListener("blur", leave);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      active = false;
      permissionOwnerActive.current = false;
      ++permissionRequest.current;
      window.clearInterval(timer);
      window.removeEventListener("focus", reenter);
      window.removeEventListener("blur", leave);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [step, acceptPermissionReadback, refreshPermissions]);

  useEffect(() => {
    let active = true;
    const handleError = (event: { payload: unknown }) => {
      const lease = currentTrialLease.current;
      if (!active || !matchesTrialMetadata(event.payload, lease)) return;
      if (recordingGeneration.current && event.payload.recording_generation !== recordingGeneration.current) return;
      const receipt = event.payload as TrialReceipt & { code?: unknown; message?: unknown };
      if (step === DICTATION_STEP) {
        recordFact(readinessFact("in_app_trial", "failed", "onboarding://error", receipt.config_generation, sessionId));
      }
      const code = typeof receipt.code === "string" ? receipt.code : "";
      const copyByCode: Record<string, string> = {
        microphone_required: "需要麦克风权限才能试用语音输入。",
        closed_lid_builtin_input_unavailable: "当前输入设备不可用；请检查输入设备后重试。",
        api_key_required: "请先配置语音服务密钥，再试用语音输入。",
        onboarding_required: "请先完成语音输入设置，再开始试用。",
        no_speech: "没有检测到语音；请检查麦克风后重试。",
        onboarding_selection_missing: "请先在试用输入框中选中要修改的文字。",
        processing_timeout: "语音处理超时；请重试。",
        audio_error: "语音输入设备或音频发生错误；请检查设备后重试。",
        dictation_error: "语音试用未完成；请检查设备或语音服务后重试。",
        paste_failed: "选中文本操作未能完成；请检查输入内容后重试。",
      };
      const message = (event.payload as { message?: unknown }).message;
      setTrialErrorDetail(null);
      setHotkeyError(t(copyByCode[code] ?? "语音试用未完成；请检查设备或语音服务后重试。"));
      setTrialErrorDetail(typeof message === "string" && message.length > 0 ? message : null);
      setRecording(false); setProcessing(false);
    };
    const subscriptions = [listen<unknown>("onboarding://error", handleError), listen<unknown>("selected-action://error", handleError)];
    return () => { active = false; for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten()).catch(() => undefined); };
  }, [recordFact, sessionId, step, t]);

  const validate = async () => {
    if (strictOfflineBlocked) {
      setValid("invalid");
      setEngineError(t("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用。请更换模型，或关闭严格离线模式。"));
      return;
    }
    const testedGeneration = draftKey;
    const testedNativeGeneration = nativeGeneration;
    setValidating(true);
    try {
      if (asrProvider === "soniox") {
        if (configuredAsrKey || key.trim()) {
          setValid("configured");
          setEngineError(null);
        } else {
          setValid(null);
          setEngineError(t("请先填写服务商 API Key。"));
        }
      } else if (asrProvider === "groq") {
        const result = key.trim()
          ? await invoke<string>("validate_api_key", { key: key.trim() })
          : await invoke<string>("validate_configured_api_key");
        if (generationRef.current !== testedGeneration) return;
        setValid(result);
        if (testedNativeGeneration) recordFact(readinessFact("provider_probe", result === "valid" ? "passed" : "failed", "validate_api_key:authentication_only", testedNativeGeneration, sessionId));
      } else if (asrProvider !== "on_device") {
        const engineSettings = settings as unknown as EngineSettingsSnapshot;
        const draft = draftFromSettings(engineSettings);
        draft.asrProvider = asrProvider;
        draft.asrModel = asrModel;
        draft.dashscopeRegion = dashscopeRegion;
        if (key.trim()) draft.providerKeys[asrProvider] = key.trim();
        const result = await invoke<ProbeResult>("probe_engine_draft", {
          draft: probePayload(draft, engineSettings, {
            asrProvider,
            cleanupEnabled: false,
          }),
        });
        if (generationRef.current !== testedGeneration) return;
        if (testedNativeGeneration) recordFact(readinessFact("provider_probe", result.asr.ok ? "passed" : "failed", "probe_engine_draft:asr", testedNativeGeneration, sessionId));
        if (result.asr.ok) {
          setValid("valid");
          setEngineError(null);
        } else {
          setValid("invalid");
          setEngineError(t("所选转写服务检查失败，请检查语音服务设置。"));
        }
      }
    } catch {
      if (generationRef.current !== testedGeneration) return;
      if (testedNativeGeneration) recordFact(readinessFact("provider_probe", "unknown", "provider_probe:ipc_error", testedNativeGeneration, sessionId));
      setValid("ipc_error");
      setEngineError(asrProvider === "groq"
        ? t("无法验证访问密钥，请检查网络或稍后重试")
        : t("服务检查失败，请检查语音服务设置。"));
    }
    if (generationRef.current === testedGeneration) setValidating(false);
  };

  const refreshOnDeviceModels = useCallback(async (expectedModelId?: string, authoritativeAfterDownload = false) => {
    const selectedModelAtStart = selectedOnDeviceModelIdRef.current;
    if (!onDeviceComponentActiveRef.current || (expectedModelId && selectedModelAtStart !== expectedModelId)) return;

    let request = onDeviceStatusRequestRef.current;
    if (!request || authoritativeAfterDownload) {
      const sequence = ++onDeviceStatusRefreshSequenceRef.current;
      const promise = invoke<OnDeviceModelStatus[]>("list_on_device_models");
      request = { sequence, promise };
      onDeviceStatusRequestRef.current = request;
      void promise.then(
        () => {
          if (onDeviceStatusRequestRef.current?.sequence === sequence) onDeviceStatusRequestRef.current = null;
        },
        () => {
          if (onDeviceStatusRequestRef.current?.sequence === sequence) onDeviceStatusRequestRef.current = null;
        },
      );
    }

    try {
      const models = await request.promise;
      if (
        !onDeviceComponentActiveRef.current
        || request.sequence !== onDeviceStatusRefreshSequenceRef.current
        || selectedOnDeviceModelIdRef.current !== selectedModelAtStart
        || (expectedModelId && selectedOnDeviceModelIdRef.current !== expectedModelId)
      ) return;
      setOnDeviceModels(Array.isArray(models) ? models : []);
      setModelReceipt(Array.isArray(models) ? models.find((model) => model.id === selectedModelAtStart) ?? null : null);
      setOnDeviceActionError(null);
    } catch {
      if (
        onDeviceComponentActiveRef.current
        && request.sequence === onDeviceStatusRefreshSequenceRef.current
        && selectedOnDeviceModelIdRef.current === selectedModelAtStart
        && (!expectedModelId || selectedOnDeviceModelIdRef.current === expectedModelId)
      ) {
        setModelReceipt(null);
        setOnDeviceActionError(t("无法读取本机模型状态，请重试。"));
      }
    }
  }, [t]);

  const refreshOnDeviceModelsFromUi = useCallback(() => {
    void refreshOnDeviceModels();
  }, [refreshOnDeviceModels]);

  useEffect(() => {
    let active = true;
    onDeviceComponentActiveRef.current = true;
    void refreshOnDeviceModels();
    const subscription = listen<OnDeviceModelStatus>("ondevice://download", (event) => {
      if (!active || !onDeviceComponentActiveRef.current) return;
      const downloadStatus = event.payload;
      if (selectedOnDeviceModelIdRef.current === downloadStatus.id) setModelReceipt(null);
      setOnDeviceModels((current) => {
        const previous = current.find((model) => model.id === downloadStatus.id);
        const nextStatus = previous
          ? {
              ...downloadStatus,
              runtime_status: previous.runtime_status,
              inference_ready: downloadStatus.state === "ready" && previous.inference_ready,
              loaded: downloadStatus.state === "ready" && previous.loaded,
            }
          : downloadStatus;
        return [...current.filter((model) => model.id !== downloadStatus.id), nextStatus];
      });
      if (downloadStatus.state !== "downloading" && selectedOnDeviceModelIdRef.current === downloadStatus.id) {
        void refreshOnDeviceModels(downloadStatus.id, true);
      }
    });
    return () => {
      active = false;
      onDeviceComponentActiveRef.current = false;
      onDeviceStatusRefreshSequenceRef.current += 1;
      onDeviceStatusRequestRef.current = null;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [refreshOnDeviceModels]);

  const runOnDeviceAction = async (id: string, action: "download" | "cancel") => {
    setOnDeviceActionId(id);
    setOnDeviceActionError(null);
    try {
      await invoke(action === "download" ? "download_on_device_model" : "cancel_on_device_download", { id });
      await refreshOnDeviceModels();
    } catch {
      if (onDeviceComponentActiveRef.current) setOnDeviceActionError(t("本机模型操作失败，请重试。"));
      await refreshOnDeviceModels();
    } finally {
      if (onDeviceComponentActiveRef.current) setOnDeviceActionId(null);
    }
  };

  const openPrivacySettings = async (pane: "accessibility" | "microphone") => {
    setSettingsError(null);
    try {
      await invoke("open_privacy_settings", { pane });
    } catch {
      setSettingsError(t("无法打开系统设置，请在“隐私与安全性”中手动开启权限"));
    }
  };

  const enableAccessibility = async () => {
    setGuideOpen(true);
    setSettingsError(null);
    await refreshPermissions();
  };

  const requestMicrophone = async () => {
    setSettingsError(null);
    if (permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted") {
      await openPrivacySettings("microphone");
      return;
    }
    try {
      await invoke("request_microphone_permission");
      if (!permissionOwnerActive.current) return;
      await refreshPermissions();
    } catch {
      if (!permissionOwnerActive.current) return;
      ++permissionRequest.current;
      acceptPermissionReadback(null);
      setSettingsError(t("无法请求麦克风权限，请重试"));
    }
  };

  useEffect(() => {
    if (!isHotkeyTrialStep(step)) return undefined;

    let active = true;
    const handleState = (event: { payload: unknown }) => {
      if (!active || !matchesTrialIdentity(event.payload, currentTrialLease.current)) return;
      if (!event.payload.trial_current) {
        currentTrialLease.current = null; invalidateConfiguration();
        setTrialErrorDetail(null);
        setHotkeyError(t("试用已失效，请确认当前权限和配置后重新准备。"));
        return;
      }
      const receipt = event.payload as TrialReceipt & { state?: string };
      const next = receipt.state;
      if (next === "starting" || next === "recording") {
        if (recordingGeneration.current && BigInt(receipt.recording_generation) < BigInt(recordingGeneration.current)) return;
        recordingGeneration.current = receipt.recording_generation;
        if (receipt.trigger_source === "native_hotkey") recordFact(readinessFact("native_hotkey", "passed", "dictation://state:native_hotkey", receipt.config_generation, sessionId));
      } else if (recordingGeneration.current && recordingGeneration.current !== receipt.recording_generation) return;
      if (next === "recording" || next === "listening") {
        setRecording(true);
        setProcessing(false);
        return;
      }
      if (next === "processing" || next === "preparing_rewrite") {
        setRecording(false);
        setProcessing(true);
        return;
      }
      if (next === "done" || next === "copied" || next === "degraded") {
        setRecording(false);
        setProcessing(false);
        return;
      }
      if (next === "idle" || next === "error") {
        setRecording(false);
        setProcessing(false);
      }
    };
    const subscriptions = [listen<unknown>("dictation://state", handleState), listen<unknown>("selected-action://state", handleState)];

    return () => {
      active = false;
      for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [step, sessionId, recordFact, invalidateConfiguration, t]);

  useEffect(() => {
    if (!isHotkeyTrialStep(step)) {
      setRecording(false);
      setProcessing(false);
    }
  }, [step]);

  const saveHotkeySettings = async () => {
    setHotkeyError(null);
    const patch = {
      hotkey,
      activation_mode: activationMode,
      ...(asrProvider === "groq" && key.trim() ? { api_key: key.trim() } : {}),
    };
    try {
      await invoke("update_settings_patch", { patch });
      const current = await invoke<Settings>("get_settings");
      if (current && typeof current === "object") setSavedSettings(current);
      if (!current || current.hotkey_error || current.hotkey !== hotkey || current.activation_mode !== activationMode) {
        setHotkeyRegistered(false);
        setHotkeyError(String(current?.hotkey_error || t("快捷键注册尚未确认，请重新保存。")));
        return false;
      }
      setHotkeyRegistered(true);
      const generation = await invoke<unknown>("get_onboarding_configuration_generation");
      if (isNativeGeneration(generation)) { setNativeGeneration(generation); recordFact(readinessFact("hotkey_registered", "passed", "get_settings:hotkey_error", generation, sessionId)); }
      return true;
    } catch {
      setHotkeyError(t("快捷键保存失败，请换一个快捷键后重试"));
      return false;
    }
  };

  const saveSelectedActionHotkey = async () => {
    setHotkeyError(null);
    try {
      await invoke("update_settings_patch", {
        patch: { selected_action_hotkey: selectedActionHotkey, selected_actions_enabled: true },
      });
      const current = await invoke<Settings>("get_settings");
      if (!current || current.selected_action_hotkey !== selectedActionHotkey || current.selected_actions_enabled !== true) {
        setHotkeyError(t("快捷键注册尚未确认，请重新保存。"));
        return false;
      }
      setSavedSettings(current); setHotkeyRegistered(true);
      return true;
    } catch {
      setHotkeyError(t("快捷键保存失败，请换一个快捷键后重试"));
      return false;
    }
  };

  // Both save points consume this owner-defined patch. ASR locality alone never
  // rewrites a previously saved independent cleanup choice.
  const engineSettingsPatch = () => ({
    asr_provider: asrProvider,
    asr_model: asrModel.trim() || defaultModel(asrProvider, "asr"),
    ...asrEndpointPatch(asrProvider, dashscopeRegion),
    ...(asrProvider === "groq" && key.trim() ? { api_key: key.trim() } : {}),
    ...(asrProvider !== "groq" && asrProvider !== "on_device" && key.trim() ? { provider_keys: { [asrProvider]: key.trim() } } : {}),
    ...(language !== String(settings.language ?? "auto") ? { language } : {}),
    ...cleanupIntentPatch(cleanupIntent),
  });

  const next = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      if (step === 2) {
        try {
          await invoke("update_settings_patch", { patch: engineSettingsPatch() });
          setSavedSettings((current) => ({ ...current, ...cleanupIntentPatch(cleanupIntent) }));
          setCleanupIntent({ kind: "preserve" });
          const refreshed = await invoke<Settings>("get_settings");
          if (refreshed && typeof refreshed === "object") setSavedSettings(refreshed);
        } catch (reason) {
          setEngineError(
            friendlySettingsError(reason, t)
              || (asrProvider === "on_device" ? t("设置保存失败，请重试") : t("访问密钥保存失败，请重试")),
          );
          return;
        }
      }
      if (step === DICTATION_STEP && !await saveHotkeySettings()) return;
      if (step === SELECTED_ACTION_STEP && !await saveSelectedActionHotkey()) return;
      await releaseOwnedTrial();
      setStep((value) => value === DICTATION_STEP ? FINISH_STEP : Math.min(FINISH_STEP, value + 1));
    } finally {
      setSubmitting(false);
    }
  };

  const finish = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      if (strictOfflineBlocked) {
        setEngineError(t("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用。请更换模型，或关闭严格离线模式。"));
        setStep(2);
        return;
      }
      if (asrProvider === "on_device") {
        if (!onDeviceReady) {
          setEngineError(t("所选本机模型尚未通过运行时能力检查；请查看模型状态或选择云端服务。"));
          setStep(2);
          return;
        }
        if (!currentAsrLanguageSupported) {
          setEngineError(t("此模型要求固定使用中文或 English；自动语言检测不可用。请在录音设置中更改识别语言。"));
          setStep(2);
          return;
        }
      } else if (!asrConfigured) {
        setStep(2);
        setEngineError(t("请先配置所选转写服务的密钥和模型。"));
        return;
      }
      const patch = {
        hotkey,
        activation_mode: activationMode,
        ...(selectedActionTrial ? { selected_action_hotkey: selectedActionHotkey, selected_actions_enabled: true } : {}),
        ...(settings.ui_language ? { ui_language: settings.ui_language } : {}),
        onboarded: true,
        ...engineSettingsPatch(),
      };
      try {
        await invoke("update_settings_patch", { patch });
        onFinish(await invoke<Settings>("get_settings"));
      } catch (reason) {
        setEngineError(t("设置保存失败，请重试"));
        setStep(2);
      }
    } finally {
      setSubmitting(false);
    }
  };

  const canNext = step === 1
    ? Boolean(permissions?.microphone)
    : step === 2
      ? !validating && !strictOfflineBlocked && currentAsrLanguageSupported && (asrProvider === "on_device" ? onDeviceReady : asrConfigured)
      : true;
  useEffect(() => {
    if (!nativeGeneration) return;
    const receipt = (stage: ReadinessFact["stage"], state: ReadinessFact["state"], source: string) => recordFact(readinessFact(stage, state, source, configurationGeneration, sessionId));
    const provider = savedAsrProvider(savedSettings);
    const credentials = provider === "on_device" || providerById(provider)?.allowsEmptyKey || (provider === "groq" ? savedSettings.api_key_configured : savedSettings.provider_keys?.[provider]?.configured || savedSettings.asr_api_key_configured);
    receipt("credential_configured", credentials ? "passed" : "not_tested", "get_settings:credential_flags");
    if (asrProvider === "on_device") {
      receipt("files_validated", modelReceipt?.id === asrModel && modelReceipt.state === "ready" ? "passed" : modelReceipt?.id === asrModel && modelReceipt.state === "corrupt" ? "failed" : onDeviceActionError ? "unknown" : "not_tested", "list_on_device_models:state");
      receipt("runtime_capability", modelReceipt?.id === asrModel && modelReceipt.inference_ready ? "passed" : modelReceipt?.id === asrModel && modelReceipt.runtime_status === "runtime_failed" ? "failed" : onDeviceActionError ? "unknown" : "not_tested", "list_on_device_models:inference_ready");
      receipt("model_loaded", modelReceipt?.id === asrModel && modelReceipt.loaded ? "passed" : onDeviceActionError ? "unknown" : "not_tested", "list_on_device_models:loaded");
    }
  }, [nativeGeneration, configurationGeneration, sessionId, asrProvider, asrModel, modelReceipt, onDeviceActionError, savedSettings, recordFact]);

  const hotkeyDisplay = formatHotkeyDisplay(hotkey);
  const onboardingStepLabels = selectedActionTrial
    ? ["开始", "权限", "连接服务", "语音输入", "选中文本", "完成"]
    : ["开始", "权限", "连接服务", "语音输入", "完成"];
  const announcedStepIndex = step === FINISH_STEP && !selectedActionTrial ? onboardingStepLabels.length - 1 : step;
  return (
    <main className="vf-settings vf-onboarding flex h-screen overflow-hidden bg-base text-primary">
      <p className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {t("新手设置进度")}：{t(onboardingStepLabels[announcedStepIndex] ?? onboardingStepLabels[onboardingStepLabels.length - 1])}
      </p>
      <OnboardingSidebar step={step} selectedActionTrial={selectedActionTrial} />
      <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div ref={contentRef} className={`vf-onboarding-content flex min-h-0 w-full flex-1 overflow-y-auto py-8 ${step === 0 ? "vf-onboarding-content--welcome items-center" : "items-start"}`}>
          <AnimatedContent key={step} className="w-full">
              {step === 0 && <WelcomeStep settings={savedSettings} route={asrProvider === "on_device" ? "local" : "cloud"} onRoute={(route) => {
                if ((route === "local") === (asrProvider === "on_device")) return;
                const provider = route === "local" ? "on_device" : savedAsrProvider(savedSettings) === "on_device" ? "groq" : savedAsrProvider(savedSettings);
                const model = provider === savedSettings.asr_provider && savedSettings.asr_model ? savedSettings.asr_model : defaultModel(provider, "asr");
                setCleanupIntent((current) => cleanupIntentForRoute(current, asrProvider, provider));
                setAsrProvider(provider); setAsrModel(model); selectedOnDeviceModelIdRef.current = model; setValid(null); setKey(""); invalidateConfiguration();
              }} onSettings={(page) => void leaveForSettings(page)} />}
              {step === 1 && (
                <PermissionsStep
                  permissions={permissions}
                  settingsError={settingsError}
                  onRefresh={refreshPermissions}
                  onRequestMicrophone={() => void requestMicrophone()}
                  onEnableAccessibility={() => void enableAccessibility()}
                  onRetryGuide={reopenPermissionGuide}
                  guideOpen={guideOpen} nativeGuide={nativeGuide} guideFailed={guideFailed} onCloseGuide={() => setGuideOpen(false)}
                  onOpenAccessibilitySettings={() => void openPrivacySettings("accessibility")}
                  inputDevice={inputDevice} configurationKey={`${draftKey}:permission-window:${permissionWindow}`}
                  onMicrophoneStatus={onMicrophoneStatus}
                  onInputDeviceChange={async (value) => {
                    await invoke("update_settings_patch", { patch: { input_device: value } });
                    setInputDevice(value); const refreshed = await invoke<Settings>("get_settings");
          if (refreshed && typeof refreshed === "object") setSavedSettings(refreshed); invalidateConfiguration();
                  }}
                />
              )}
              {step === 2 && (
                <>
                <EngineConfigStep
                  asrProvider={asrProvider}
                  asrModel={asrModel}
                  dashscopeRegion={dashscopeRegion}
                  onDashscopeRegionChange={(value) => {
                    setDashscopeRegion(value);
                    invalidateConfiguration(); setValidating(false);
                    setValid(null);
                    setEngineError(null);
                  }}
                  onAsrModelChange={(value) => {
                    selectedOnDeviceModelIdRef.current = value;
                    setAsrModel(value);
                    invalidateConfiguration(); setValidating(false);
                    setValid(null);
                    setEngineError(null);
                  }}
                  language={language}
                  onLanguageChange={(value) => {
                    setLanguage(value);
                    invalidateConfiguration(); setValidating(false);
                    setValid(null);
                    setEngineError(null);
                  }}
                  keyHint={asrKeyHint}
                  configuredAsrKey={configuredAsrKey}
                  onAsrProviderChange={(value) => {
                    setCleanupIntent((current) => cleanupIntentForRoute(current, asrProvider, value));
                    setAsrProvider(value);
                    invalidateConfiguration(); setValidating(false);
                    const nextModel = defaultModel(value, "asr");
                    selectedOnDeviceModelIdRef.current = nextModel;
                    setAsrModel(nextModel);
                    setValid(null);
                    setEngineError(null);
                    setKey("");
                  }}
                  onDeviceReady={onDeviceReady}
                  localCleanupDefault={cleanupIntent.kind === "initialize_local_off"}
                  onDeviceModels={onDeviceModels}
                  onDeviceActionId={onDeviceActionId}
                  onDeviceActionError={onDeviceActionError}
                  onDeviceAction={(id, action) => void runOnDeviceAction(id, action)}
                  onRefreshOnDeviceModels={refreshOnDeviceModelsFromUi}
                  strictOfflineBlocked={strictOfflineBlocked}
                  keyValue={key}
                  onKeyChange={(value) => {
                    setKey(value);
                    invalidateConfiguration(); setValidating(false);
                    setValid(null);
                    setEngineError(null);
                  }}
                  valid={valid}
                  validating={validating}
                  onValidate={validate}
                  error={engineError}
                />
                <CleanupGuide settings={savedSettings} intent={cleanupIntent} onSave={async (patch) => {
                  await invoke("update_settings_patch", { patch });
                  setCleanupIntent({ kind: "explicit", ...patch });
                  setSavedSettings((current) => ({ ...current, ...patch }));
                  invalidateConfiguration();
                  try {
                    const current = await invoke<Settings>("get_settings");
                    if (current && typeof current === "object") setSavedSettings(current);
                  } catch { setEngineError(t("整理选择已保存，但状态读取未完成，请重新检查。")); }
                }} />
                </>
              )}
              {isHotkeyTrialStep(step) && asrProvider === "on_device" && !onDeviceReady && (
                <p role="status" className="mb-3 text-sm text-secondary">{t("所选本机模型尚未通过运行时能力检查；请在模型设置中选择可用模型。")}</p>
              )}
              {isHotkeyTrialStep(step) && (
                <HotkeyStep
                  trial={step === DICTATION_STEP ? "dictation" : "selected_action"}
                  hotkey={hotkey}
                  activationMode={activationMode}
                  selectedActionHotkey={selectedActionHotkey}
                  recording={recording}
                  processing={processing}
                  error={hotkeyError}
                  errorDetail={trialErrorDetail}
                  modeSaving={modeSaving}
                  trialReady={trialReady} trialLease={trialLease} configurationGeneration={draftKey}
                  hotkeyRegistered={hotkeyRegistered}
                  onSaveHotkey={() => void (step === DICTATION_STEP ? saveHotkeySettings() : saveSelectedActionHotkey())}
                  onTrialResult={(receipt) => {
                    if (step === DICTATION_STEP && permissionReadback.current.permissions?.microphone === true && matchesTrialMetadata(receipt, currentTrialLease.current)) {
                      recordFact(readinessFact("in_app_trial", "passed", "dictation://onboarding-result", receipt.config_generation, sessionId));
                    }
                  }}
                  onRetry={() => { setHotkeyError(null); invalidateConfiguration(); }}
                  onCancel={() => releaseOwnedTrial()}
                  onSettings={(page) => void leaveForSettings(page)}
                  onHotkeyChange={(value) => {
                    setHotkey(value);
                    invalidateConfiguration(); setValidating(false);
                    setHotkeyError(null);
                  }}
                  onActivationModeChange={(mode) => {
                    setModeSaving(true);
                    void invoke("update_settings_patch", { patch: { activation_mode: mode } })
                      .then(() => { setActivationMode(mode); invalidateConfiguration(); setHotkeyError(null); })
                      .catch(() => setHotkeyError(t("录音方式保存失败，请重试。")))
                      .finally(() => setModeSaving(false));
                  }}
                  onSelectedActionHotkeyChange={(value) => {
                    setSelectedActionHotkey(value);
                    invalidateConfiguration(); setValidating(false);
                    setHotkeyError(null);
                  }}
                />
              )}
              {step === FINISH_STEP && (
                <>
                  {!externalOpen && <FinishStep hotkeyDisplay={hotkeyDisplay} activationMode={activationMode}
                    settings={savedSettings} facts={currentReadiness(facts, configurationGeneration, sessionId)}
                    onExternal={() => setExternalOpen(true)} onRetry={() => setStep(DICTATION_STEP)} onSettings={(page) => void leaveForSettings(page)} />}
                  {externalOpen && <ExternalDeliveryTrial configurationKey={`${draftKey}:permission-window:${permissionWindow}`}
                    permissions={permissions} onGeneration={setNativeGeneration} onContinuation={setExternalContinuation}
                    continuationIsCurrent={(lease) => externalContinuation.current?.trial_id === lease.trial_id && externalContinuation.current.config_generation === lease.config_generation}
                    refreshPermissions={refreshPermissions} onClose={() => setExternalOpen(false)}
                    onConfirmed={(receipt: ExternalReceipt) => {
                      // The child rechecks its current lease and native owner;
                      // parent additionally binds presentation to this session.
                      if (receipt.lease.config_generation !== nativeGeneration || !permissionReadback.current.permissions
                        || externalContinuation.current?.trial_id !== receipt.lease.trial_id
                        || externalContinuation.current.config_generation !== receipt.lease.config_generation) return;
                      recordFact(readinessFact("external_delivery", receipt.trial_current && receipt.target && receipt.readback === "exact_once" ? "passed" : "unknown",
                        receipt.source, receipt.lease.config_generation, sessionId));
                      recordFact(readinessFact("clipboard_restoration", receipt.clipboard_restoration === "restored" || receipt.clipboard_restoration === "not_needed" ? "passed" : "unknown",
                        `paste_owner:${receipt.clipboard_restoration}`, receipt.lease.config_generation, sessionId));
                    }} />}
                  {!externalOpen && <DataChoices settings={savedSettings} onSettings={(page) => void leaveForSettings(page)} />}
                  {!externalOpen && <div className="mt-6 border-t border-border pt-4 text-sm text-secondary">
                    <p>{t("词典、语气和翻译快捷键可在设置中逐步配置。")}</p>
                    <button type="button" disabled={submitting} onClick={() => { if (submitting) return; setSelectedActionTrial(true); setStep(SELECTED_ACTION_STEP); }} className={`mt-2 ${ghostButtonClass}`}>{t("试用选中文本操作（可选）")}</button>
                  </div>}
                </>
              )}
          </AnimatedContent>
        </div>
        {guideFailed && <p role="alert" className="px-6 py-2 text-xs text-error-ink">{t("权限提示窗释放或打开状态无法确认，请重试。")}</p>}
        {releaseFailed && <p role="alert" className="px-6 py-2 text-xs text-error-ink">{t("试用资源释放尚未确认；请重试，旧会话不会作为当前成功。")}</p>}
        {!externalOpen && <OnboardingFooter
          step={step}
          canNext={canNext}
        busy={submitting || recording || processing || externalOpen}
        busyLabel={t(submitting ? "保存中…" : recording ? "录音中…" : "处理中…")}
          onBack={() => { void releaseOwnedTrial().then(() => setStep((value) => value === FINISH_STEP && !selectedActionTrial ? DICTATION_STEP : Math.max(0, value - 1))).catch(() => setHotkeyError(t("无法结束试用，请取消录音后重试。"))); }}
          onSkip={() => void leaveForSettings()}
          onNext={() => void next()}
          onFinish={() => void finish()}
        />}
      </section>
    </main>
  );
}
