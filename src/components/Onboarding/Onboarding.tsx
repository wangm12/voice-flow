import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ActivationMode } from "../../lib/activationCopy";
import { formatHotkeyDisplay } from "../../lib/hotkeyFormat";
import { EngineConfigStep } from "./EngineConfigStep";
import { PermissionsStep } from "./PermissionsStep";
import { OnboardingFooter } from "./OnboardingFooter";
import { OnboardingSidebar } from "./OnboardingSidebar";
import { WelcomeStep } from "./WelcomeStep";
import { FinishStep } from "./FinishStep";
import { HotkeyStep } from "./HotkeyStep";
import { AnimatedContent } from "../ReactBits/AnimatedContent";
import { useI18n } from "../../lib/i18n";

type Permissions = { microphone: boolean; microphone_status: string; accessibility: boolean };
type Settings = {
  api_key_configured?: boolean;
  api_key_hint?: string | null;
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
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [key, setKey] = useState("");
  const [hotkey, setHotkey] = useState(String(settings.hotkey ?? "CmdOrControl+Shift+Space"));
  const [activationMode, setActivationMode] = useState<ActivationMode | string>(
    String(settings.activation_mode ?? "tap"),
  );
  const [selectedActionHotkey, setSelectedActionHotkey] = useState(
    String(settings.selected_action_hotkey ?? "CmdOrControl+Shift+Slash").trim() || "CmdOrControl+Shift+Slash",
  );
  const [validating, setValidating] = useState(false);
  const [valid, setValid] = useState<string | null>(null);
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [hotkeyError, setHotkeyError] = useState<string | null>(settings.hotkey_error ?? null);
  const [submitting, setSubmitting] = useState(false);
  const testModeQueue = useRef<Promise<void>>(Promise.resolve());

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
    setValidating(true);
    try {
      const result = key.trim()
        ? await invoke<string>("validate_api_key", { key: key.trim() })
        : await invoke<string>("validate_configured_api_key");
      setValid(result);
    } catch {
      setValid("ipc_error");
      setEngineError(t("无法验证访问密钥，请检查网络或稍后重试"));
    }
    setValidating(false);
  };

  useEffect(() => {
    if (step === 2 && settings.api_key_configured && !key && !valid) void validate();
  }, [step, settings.api_key_configured]);

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
    await openPrivacySettings("accessibility");
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
    const patch = { hotkey, activation_mode: activationMode, ...(key.trim() ? { api_key: key.trim() } : {}) };
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
      if (step === 2 && key.trim()) {
        try {
          await invoke("update_settings_patch", { patch: { api_key: key.trim() } });
        } catch (reason) {
          setEngineError(t("访问密钥保存失败，请重试"));
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
      setStep((value) => Math.min(FINISH_STEP, value + 1));
    } finally {
      setSubmitting(false);
    }
  };

  const finish = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      if (valid !== "valid") {
        setStep(2);
        await validate();
        return;
      }
      const patch = {
        hotkey,
        activation_mode: activationMode,
        selected_action_hotkey: selectedActionHotkey,
        selected_actions_enabled: true,
        ui_language: "system",
        onboarded: true,
        ...(key.trim() ? { api_key: key.trim() } : {}),
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
      ? valid === "valid"
      : true;
  const hotkeyDisplay = formatHotkeyDisplay(hotkey);
  return (
    <main className="flex h-screen overflow-hidden bg-base text-primary">
      <OnboardingSidebar step={step} />
      <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <div className={`mx-auto flex min-h-0 w-full flex-1 items-center overflow-y-auto px-8 py-8 ${step === 0 ? "max-w-[620px]" : "max-w-[480px]"}`}>
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
              {isHotkeyTrialStep(step) && (
                <HotkeyStep
                  trial={step === DICTATION_STEP ? "dictation" : "selected_action"}
                  hotkey={hotkey}
                  activationMode={activationMode}
                  selectedActionHotkey={selectedActionHotkey}
                  recording={recording}
                  processing={processing}
                  error={hotkeyError}
                  onHotkeyChange={(value, mode) => {
                    setHotkey(value);
                    setHotkeyError(null);
                    if (mode) setActivationMode(mode);
                  }}
                  onSelectedActionHotkeyChange={(value) => {
                    setSelectedActionHotkey(value);
                    setHotkeyError(null);
                  }}
                />
              )}
              {step === FINISH_STEP && (
                <FinishStep hotkeyDisplay={hotkeyDisplay} activationMode={activationMode} />
              )}
          </AnimatedContent>
        </div>
        <OnboardingFooter
          step={step}
          canNext={canNext}
          busy={submitting}
          onBack={() => setStep((value) => Math.max(0, value - 1))}
          onSkip={onSkipToSettings}
          onNext={() => void next()}
          onFinish={() => void finish()}
        />
      </section>
    </main>
  );
}
