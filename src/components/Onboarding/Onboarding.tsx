import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { type ActivationMode } from "../../lib/activationCopy";
import { DEFAULT_DICTATION_HOTKEY, DEFAULT_SELECTED_ACTION_HOTKEY, formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { EngineConfigStep } from "./EngineConfigStep";
import { PermissionsStep } from "./PermissionsStep";
import { OnboardingFooter } from "./OnboardingFooter";
import { OnboardingSidebar } from "./OnboardingSidebar";
import { WelcomeStep } from "./WelcomeStep";
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
  providerById,
  type DashscopeRegion,
  type ProviderId,
} from "../../lib/providers";
import type { OnDeviceModelStatus, Settings as EngineSettingsSnapshot } from "../../types/settings";

type Permissions = { microphone: boolean; microphone_status: string; accessibility: boolean };
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
  onSkipToSettings: () => void;
}) {
  const { t } = useI18n();
  const [step, setStep] = useState(0);
  const contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => { if (contentRef.current) contentRef.current.scrollTop = 0; }, [step]);
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [asrProvider, setAsrProvider] = useState<ProviderId>(() => savedAsrProvider(settings));
  const [asrModel, setAsrModel] = useState(() => {
    const provider = savedAsrProvider(settings);
    return provider === settings.asr_provider
      ? String(settings.asr_model || defaultModel(provider, "asr"))
      : defaultModel(provider, "asr");
  });
  const [language, setLanguage] = useState(() => String(settings.language ?? "auto"));
  const [dashscopeRegion, setDashscopeRegion] = useState<DashscopeRegion>(() => dashscopeRegionFromEndpoint(settings.asr_base_url));
  const [onDeviceModels, setOnDeviceModels] = useState<OnDeviceModelStatus[]>([]);
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
  const [hotkeyError, setHotkeyError] = useState<string | null>(settings.hotkey_error ?? null);
  const [submitting, setSubmitting] = useState(false);
  const [selectedActionTrial, setSelectedActionTrial] = useState(false);
  const testModeQueue = useRef<Promise<void>>(Promise.resolve());
  const selectedOnDeviceModelIdRef = useRef(asrModel);
  const onDeviceComponentActiveRef = useRef(false);
  const onDeviceStatusRefreshSequenceRef = useRef(0);
  const onDeviceStatusRequestRef = useRef<{
    sequence: number;
    promise: Promise<OnDeviceModelStatus[]>;
  } | null>(null);
  const configuredAsrKey = asrProvider === "groq"
    ? Boolean(settings.api_key_configured)
    : Boolean(settings.provider_keys?.[asrProvider]?.configured || (asrProvider === "custom" && settings.asr_api_key_configured));
  const asrKeyHint = asrProvider === "groq"
    ? String(settings.api_key_hint ?? "")
    : String(settings.provider_keys?.[asrProvider]?.hint ?? (asrProvider === "custom" ? settings.asr_api_key_hint : "") ?? "");
  const selectedOnDeviceModel = onDeviceModels.find((model) => model.id === asrModel);
  const onDeviceReady = asrProvider === "on_device" && selectedOnDeviceModel?.inference_ready === true;
  const currentAsrLanguageSupported = hasSupportedAsrLanguage(asrModelProfile(asrProvider, asrModel), language);
  const strictOfflineEnabled = settings.strict_offline_enabled === true;
  const strictOfflineAsrAllowed = asrProvider === "on_device";
  const strictOfflineBlocked = strictOfflineEnabled && !strictOfflineAsrAllowed;
  const setOnboardingTestMode = useCallback((enabled: boolean) => {
    const request = testModeQueue.current
      .catch(() => undefined)
      .then(async () => {
        await invoke("set_onboarding_test_mode", { enabled });
      });
    testModeQueue.current = request.catch(() => undefined);
    return request;
  }, []);

  useEffect(() => {
    if (step !== 1) return;
    const poll = () => void invoke<Permissions>("check_permissions").then((next) => {
      setPermissions((current) => current
        && current.microphone === next.microphone
        && current.microphone_status === next.microphone_status
        && current.accessibility === next.accessibility
        ? current
        : next);
    }).catch(() => setSettingsError(t("无法检测系统权限，请重试")));
    poll();
    const timer = window.setInterval(poll, 1000);
    return () => window.clearInterval(timer);
  }, [step]);

  useEffect(() => {
    const enabled = isHotkeyTrialStep(step);
    void setOnboardingTestMode(enabled).catch(() => {
      if (enabled) setHotkeyError(t("无法启动录音测试，请重试"));
    });
  }, [setOnboardingTestMode, step, t]);

  useEffect(() => () => {
    void setOnboardingTestMode(false);
  }, [setOnboardingTestMode]);

  useEffect(() => {
    let active = true;
    const subscription = listen<string>("dictation://error", (event) => {
      if (active) setHotkeyError(event.payload);
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const validate = async () => {
    if (strictOfflineBlocked) {
      setValid("invalid");
      setEngineError(t("严格离线模式仅支持本机 On Device 转写；HTTP 与 loopback 服务均不可用。请更换模型，或关闭严格离线模式。"));
      return;
    }
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
        setValid(result);
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
        if (result.asr.ok) {
          setValid("valid");
          setEngineError(null);
        } else {
          setValid("invalid");
          setEngineError(t("所选转写服务检查失败，请检查语音服务设置。"));
        }
      }
    } catch {
      setValid("ipc_error");
      setEngineError(asrProvider === "groq"
        ? t("无法验证访问密钥，请检查网络或稍后重试")
        : t("服务检查失败，请检查语音服务设置。"));
    }
    setValidating(false);
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
      setOnDeviceActionError(null);
    } catch {
      if (
        onDeviceComponentActiveRef.current
        && request.sequence === onDeviceStatusRefreshSequenceRef.current
        && selectedOnDeviceModelIdRef.current === selectedModelAtStart
        && (!expectedModelId || selectedOnDeviceModelIdRef.current === expectedModelId)
      ) {
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

  const refreshPermissions = async () => {
    setSettingsError(null);
    try {
      setPermissions(await invoke<Permissions>("check_permissions"));
    } catch {
      setSettingsError(t("无法检测系统权限，请重试"));
    }
  };

  const enableAccessibility = async () => {
    setSettingsError(null);
    try {
      const granted = await invoke<boolean>("request_accessibility_permission");
      if (!granted) {
        await openPrivacySettings("accessibility");
      }
      setPermissions(await invoke<Permissions>("check_permissions"));
    } catch {
      setSettingsError(t("无法打开系统设置，请在“隐私与安全性”中手动开启权限"));
      await openPrivacySettings("accessibility");
    }
  };

  const requestMicrophone = async () => {
    setSettingsError(null);
    if (permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted") {
      await openPrivacySettings("microphone");
      return;
    }
    try {
      await invoke("request_microphone_permission");
      setPermissions(await invoke<Permissions>("check_permissions"));
    } catch {
      setSettingsError(t("无法请求麦克风权限，请重试"));
    }
  };

  useEffect(() => {
    if (!isHotkeyTrialStep(step)) return undefined;

    let active = true;
    const subscription = listen<{ state: string }>("dictation://state", (event) => {
      if (!active) return;
      const next = event.payload.state;
      if (next === "recording") {
        setRecording(true);
        setProcessing(false);
        return;
      }
      if (next === "processing") {
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
    });

    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [step]);

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
      return true;
    } catch (error) {
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
      return true;
    } catch {
      setHotkeyError(t("快捷键保存失败，请换一个快捷键后重试"));
      return false;
    }
  };

  const next = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      if (step === 2) {
        try {
          if (asrProvider === "on_device") {
            await invoke("update_settings_patch", {
              patch: {
                asr_provider: "on_device",
                asr_model: asrModel.trim() || defaultModel("on_device", "asr"),
                cleanup_enabled: false,
                ...(language !== String(settings.language ?? "auto") ? { language } : {}),
              },
            });
          } else if (asrProvider === "groq") {
            await invoke("update_settings_patch", {
              patch: {
                asr_provider: "groq",
                asr_model: asrModel.trim() || defaultModel("groq", "asr"),
                ...(key.trim() ? { api_key: key.trim() } : {}),
                ...(language !== String(settings.language ?? "auto") ? { language } : {}),
              },
            });
          } else {
            await invoke("update_settings_patch", {
              patch: {
                asr_provider: asrProvider,
                asr_model: asrModel.trim() || defaultModel(asrProvider, "asr"),
                ...asrEndpointPatch(asrProvider, dashscopeRegion),
                ...(key.trim() ? { provider_keys: { [asrProvider]: key.trim() } } : {}),
                ...(language !== String(settings.language ?? "auto") ? { language } : {}),
              },
            });
          }
        } catch (reason) {
          setEngineError(
            friendlySettingsError(reason, t)
              || (asrProvider === "on_device" ? t("设置保存失败，请重试") : t("访问密钥保存失败，请重试")),
          );
          return;
        }
      }
      if (step === DICTATION_STEP) {
        const saved = await saveHotkeySettings();
        if (!saved) return;
        try {
          await setOnboardingTestMode(true);
        } catch (reason) {
          setHotkeyError(t("无法启动录音测试，请重试"));
          return;
        }
      }
      if (step === SELECTED_ACTION_STEP) {
        const saved = await saveSelectedActionHotkey();
        if (!saved) return;
        try {
          await setOnboardingTestMode(true);
        } catch {
          setHotkeyError(t("无法启动录音测试，请重试"));
          return;
        }
      }
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
      } else if (valid !== "valid" && !(asrProvider === "soniox" && valid === "configured")) {
        setStep(2);
        setEngineError(t("请先手动测试所选转写服务。"));
        return;
      }
      const patch = {
        hotkey,
        activation_mode: activationMode,
        ...(selectedActionTrial ? { selected_action_hotkey: selectedActionHotkey, selected_actions_enabled: true } : {}),
        ...(settings.ui_language ? { ui_language: settings.ui_language } : {}),
        ...(language !== String(settings.language ?? "auto") ? { language } : {}),
        onboarded: true,
        ...(asrProvider === "on_device"
          ? { asr_provider: "on_device", asr_model: asrModel.trim() || defaultModel("on_device", "asr"), cleanup_enabled: false }
          : asrProvider === "groq"
            ? {
              asr_provider: "groq",
              asr_model: asrModel.trim() || defaultModel("groq", "asr"),
              ...(key.trim() ? { api_key: key.trim() } : {}),
            }
            : {
              asr_provider: asrProvider,
              asr_model: asrModel.trim() || defaultModel(asrProvider, "asr"),
              ...asrEndpointPatch(asrProvider, dashscopeRegion),
              ...(key.trim() ? { provider_keys: { [asrProvider]: key.trim() } } : {}),
            }),
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
      ? !validating && !strictOfflineBlocked && currentAsrLanguageSupported && (asrProvider === "on_device" ? onDeviceReady : valid === "valid" || (asrProvider === "soniox" && valid === "configured"))
      : true;
  const hotkeyDisplay = formatHotkeyDisplay(hotkey);
  return (
    <main className="vf-settings vf-onboarding flex h-screen overflow-hidden bg-base text-primary">
      <OnboardingSidebar step={step} selectedActionTrial={selectedActionTrial} />
      <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div ref={contentRef} className={`vf-onboarding-content flex min-h-0 w-full flex-1 overflow-y-auto py-8 ${step === 0 ? "vf-onboarding-content--welcome items-center" : "items-start"}`}>
          <AnimatedContent key={step} className="w-full">
              {step === 0 && <WelcomeStep />}
              {step === 1 && (
                <PermissionsStep
                  permissions={permissions}
                  settingsError={settingsError}
                  onRefresh={refreshPermissions}
                  onRequestMicrophone={() => void requestMicrophone()}
                  onEnableAccessibility={() => void enableAccessibility()}
                />
              )}
              {step === 2 && (
                <EngineConfigStep
                  asrProvider={asrProvider}
                  asrModel={asrModel}
                  dashscopeRegion={dashscopeRegion}
                  onDashscopeRegionChange={(value) => {
                    setDashscopeRegion(value);
                    setValid(null);
                    setEngineError(null);
                  }}
                  onAsrModelChange={(value) => {
                    selectedOnDeviceModelIdRef.current = value;
                    setAsrModel(value);
                    setValid(null);
                    setEngineError(null);
                  }}
                  language={language}
                  onLanguageChange={(value) => {
                    setLanguage(value);
                    setValid(null);
                    setEngineError(null);
                  }}
                  keyHint={asrKeyHint}
                  configuredAsrKey={configuredAsrKey}
                  onAsrProviderChange={(value) => {
                    setAsrProvider(value);
                    const nextModel = defaultModel(value, "asr");
                    selectedOnDeviceModelIdRef.current = nextModel;
                    setAsrModel(nextModel);
                    setValid(null);
                    setEngineError(null);
                    setKey("");
                  }}
                  onDeviceReady={onDeviceReady}
                  onDeviceModels={onDeviceModels}
                  onDeviceActionId={onDeviceActionId}
                  onDeviceActionError={onDeviceActionError}
                  onDeviceAction={(id, action) => void runOnDeviceAction(id, action)}
                  onRefreshOnDeviceModels={refreshOnDeviceModelsFromUi}
                  strictOfflineBlocked={strictOfflineBlocked}
                  keyValue={key}
                  onKeyChange={(value) => {
                    setKey(value);
                    setValid(null);
                    setEngineError(null);
                  }}
                  valid={valid}
                  validating={validating}
                  onValidate={validate}
                  error={engineError}
                />
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
                  modeSaving={modeSaving}
                  onHotkeyChange={(value) => {
                    setHotkey(value);
                    setHotkeyError(null);
                  }}
                  onActivationModeChange={(mode) => {
                    setModeSaving(true);
                    void invoke("update_settings_patch", { patch: { activation_mode: mode } })
                      .then(() => { setActivationMode(mode); setHotkeyError(null); })
                      .catch(() => setHotkeyError(t("录音方式保存失败，请重试。")))
                      .finally(() => setModeSaving(false));
                  }}
                  onSelectedActionHotkeyChange={(value) => {
                    setSelectedActionHotkey(value);
                    setHotkeyError(null);
                  }}
                />
              )}
              {step === FINISH_STEP && (
                <>
                  <FinishStep hotkeyDisplay={hotkeyDisplay} activationMode={activationMode} />
                  <div className="mt-6 border-t border-border pt-4 text-sm text-secondary">
                    <p>{t("词典、语气和翻译快捷键可在设置中逐步配置。")}</p>
                    <button type="button" disabled={submitting} onClick={() => { if (submitting) return; setSelectedActionTrial(true); setStep(SELECTED_ACTION_STEP); }} className={`mt-2 ${ghostButtonClass}`}>{t("试用选中文本操作（可选）")}</button>
                  </div>
                </>
              )}
          </AnimatedContent>
        </div>
        <OnboardingFooter
          step={step}
          canNext={canNext}
        busy={submitting || recording || processing}
        busyLabel={t(submitting ? "保存中…" : recording ? "录音中…" : "处理中…")}
          onBack={() => setStep((value) => value === FINISH_STEP && !selectedActionTrial ? DICTATION_STEP : Math.max(0, value - 1))}
          onSkip={onSkipToSettings}
          onNext={() => void next()}
          onFinish={() => void finish()}
        />
      </section>
    </main>
  );
}
