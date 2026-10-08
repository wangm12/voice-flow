import "./App.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { AudioLines, BookMarked, Bug, CloudCog, FileText, History as HistoryIcon, Monitor, PenLine, ShieldCheck, Sparkles } from "lucide-react";
import { History, type HistoryItem } from "./components/History/History";
import { Onboarding } from "./components/Onboarding/Onboarding";
import { ContextSettings } from "./components/ContextSettings";
import { AnimatedContent } from "./components/ReactBits/AnimatedContent";
import { SettingsAlert } from "./components/SettingsLayout";
import {
  isSelectedActionError,
  isSelectedActionLifecycle,
  isSelectedActionPreview,
  SelectedPreviewDialog,
  type SelectedActionError,
  type SelectedActionPreview,
  type TextActionErrorCode,
  type TextActionOutcome,
} from "./components/SelectedPreviewDialog";
import { PermissionsSettings, type Permissions } from "./components/PermissionsSettings";
import { SnippetsSettings } from "./components/SnippetsSettings";
import { DictionarySettings } from "./components/DictionarySettings";
import { EngineSettings } from "./components/settings/EngineSettings";
import { RecordingSettings } from "./components/settings/RecordingSettings";
import { DebugSettings } from "./components/settings/DebugSettings";
import { WhatsNewDialog, type WhatsNewPayload } from "./components/WhatsNewDialog";
import { SystemSettings } from "./components/settings/SystemSettings";
import { useSettingsPersistence } from "./hooks/useSettingsPersistence";
import { resolveUiLanguage, type UiLanguagePreference, useI18n } from "./lib/i18n";
import { friendlySettingsError } from "./lib/settingsError";
import { isEngineConnected } from "./lib/engineWizard";
import { colors, radius, buttonClass, compactButtonClass, focusRingClass } from "./lib/theme";
import type { AudioInputDevice, Settings } from "./types/settings";

type HistoryPage = { items: HistoryItem[]; has_more: boolean };
type View = "general" | "engine" | "dictionary" | "history" | "smart" | "writing" | "permissions" | "system" | "snippets" | "debug";
const brandIconSrc = "/voiceflow-icon-ui.svg";
const actionOutcomeLabels: Record<TextActionOutcome, string> = {
  replaced: "结果已替换并验证",
  unverified: "结果未能验证，请检查目标。",
  copied: "结果已复制",
  copied_target_changed: "目标或来源已变化，结果已复制",
};
const actionErrorLabels: Record<TextActionErrorCode | "failed", string> = {
  no_source: "没有可处理的来源文本。",
  unsupported_instruction: "无法完成这个文字操作。请重试或取消。",
  ambiguous_instruction: "指令不够明确，请说明要执行的操作。",
  source_too_long: "来源文本过长，无法安全处理。",
  reply_context_unavailable: "无法获取当前页面的授权回复上下文。",
  translation_unverifiable: "无法验证翻译内容，因此没有生成预览。",
  permission_required: "缺少执行此操作所需的权限。",
  vision_unavailable: "看屏幕功能尚未就绪。",
  provider_failed: "文字服务暂时无法完成操作。",
  failed: "操作未能完成。",
};
type DisplayedActionError = Omit<SelectedActionError, "code"> & { code: TextActionErrorCode | "failed" };
const navigationGroups: { label: string; items: { id: View; label: string; icon: typeof AudioLines }[] }[] = [
  {
    label: "核心设置",
    items: [
      { id: "general", label: "录音与输出", icon: AudioLines },
      { id: "smart", label: "智能整理", icon: Sparkles },
      { id: "writing", label: "语气", icon: PenLine },
      { id: "engine", label: "语音服务", icon: CloudCog },
    ],
  },
  {
    label: "数据",
    items: [
      { id: "history", label: "历史记录", icon: HistoryIcon },
      { id: "dictionary", label: "个人词典", icon: BookMarked },
      { id: "snippets", label: "语音片段", icon: FileText },
    ],
  },
  {
    label: "系统",
    items: [
      { id: "system", label: "系统设置", icon: Monitor },
      { id: "permissions", label: "系统权限", icon: ShieldCheck },
    ],
  },
];

export default function App() {
  const { language: interfaceLanguage, setLanguage, t } = useI18n();
  const [whatsNew, setWhatsNew] = useState<WhatsNewPayload | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [view, setView] = useState<View>("general");
  const settingsContentRef = useRef<HTMLElement>(null);
  useEffect(() => { if (settingsContentRef.current) settingsContentRef.current.scrollTop = 0; }, [view]);
  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [historyHasMore, setHistoryHasMore] = useState(false);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const historyQueryRef = useRef("");
  const historyRequestRef = useRef(0);
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [audioInputDevice, setAudioInputDevice] = useState<string | null>(null);
  const [audioInputDevices, setAudioInputDevices] = useState<AudioInputDevice[]>([]);
  const [loadingError, setLoadingError] = useState<string | null>(null);
  const [dictationBusy, setDictationBusy] = useState(false);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [learnToast, setLearnToast] = useState<{ pair_key: string; pair_keys?: string[]; before: string; after: string } | null>(null);
  const [learnUndoError, setLearnUndoError] = useState<string | null>(null);
  const [learnUndoBusy, setLearnUndoBusy] = useState(false);
  const learnToastRef = useRef(learnToast);
  learnToastRef.current = learnToast;
  const [selectedPreview, setSelectedPreview] = useState<SelectedActionPreview | null>(null);
  const [selectedPreviewDraft, setSelectedPreviewDraft] = useState("");
  const [selectedPreviewBusy, setSelectedPreviewBusy] = useState(false);
  const [selectedPreviewError, setSelectedPreviewError] = useState<string | null>(null);
  const [textActionOutcome, setTextActionOutcome] = useState<TextActionOutcome | null>(null);
  const [selectedActionError, setSelectedActionError] = useState<DisplayedActionError | null>(null);
  const selectedPreviewRef = useRef<SelectedActionPreview | null>(null);
  const selectedPreviewDraftRef = useRef("");
  const previewCommandTransactionRef = useRef<string | null>(null);
  const closedPreviewTransactionsRef = useRef(new Set<string>());
  const terminalActionTransactionsRef = useRef(new Set<string>());
  const latestActionSequenceRef = useRef(0);
  const latestActionTransactionRef = useRef<string | null>(null);
  const selectedActionErrorRef = useRef<DisplayedActionError | null>(null);
  const selectedPreviewRestoreRef = useRef<HTMLElement | null>(null);
  const permissionCheckInFlight = useRef(false);
  const audioDeviceCheckInFlight = useRef(false);
  const formatSettingsError = useCallback((reason: unknown) => friendlySettingsError(reason, t), [t]);
  const rememberClosedPreview = useCallback((transactionId: string) => {
    const closed = closedPreviewTransactionsRef.current;
    closed.add(transactionId);
    if (closed.size > 128) {
      const oldest = closed.values().next().value;
      if (oldest) closed.delete(oldest);
    }
  }, []);
  const rememberTerminalAction = useCallback((actionSequence: number, transactionId: string) => {
    const terminal = terminalActionTransactionsRef.current;
    terminal.add(`${actionSequence}\u0000${transactionId}`);
    if (terminal.size > 128) {
      const oldest = terminal.values().next().value;
      if (oldest) terminal.delete(oldest);
    }
    rememberClosedPreview(transactionId);
  }, [rememberClosedPreview]);
  const updateSelectedActionError = useCallback((error: DisplayedActionError | null) => {
    selectedActionErrorRef.current = error;
    setSelectedActionError(error);
  }, []);
  const {
    save,
    saveError,
    setSaveError,
    flushPendingSave,
    retryPendingSave,
    unsavedFields,
  } = useSettingsPersistence({ setSettings, formatError: formatSettingsError });

  useEffect(() => {
    if (settings?.ui_language) {
      const nextLanguage = resolveUiLanguage(settings.ui_language);
      if (nextLanguage !== interfaceLanguage) setLanguage(nextLanguage);
    }
  }, [interfaceLanguage, setLanguage, settings?.ui_language]);

  useEffect(() => {
    const root = document.documentElement;
    if (!settings?.theme || settings.theme === "system") {
      root.removeAttribute("data-theme");
    } else {
      root.dataset.theme = settings.theme;
    }
  }, [settings?.theme]);

  useEffect(() => {
    document.documentElement.lang = interfaceLanguage === "en" ? "en" : "zh-CN";
  }, [interfaceLanguage]);

  const refreshPermissions = useCallback(async () => {
    if (permissionCheckInFlight.current) return;
    permissionCheckInFlight.current = true;
    try {
      setPermissions(await invoke<Permissions>("check_permissions"));
    } catch {
      // Do not keep showing an old "enabled" value when the native check
      // failed or the window is being torn down.
      setPermissions(null);
    } finally {
      permissionCheckInFlight.current = false;
    }
  }, []);

  useEffect(() => {
    let active = true;
    const actionKey = (actionSequence: number, transactionId: string) => `${actionSequence}\u0000${transactionId}`;
    const clearVisiblePreview = () => {
      const current = selectedPreviewRef.current;
      if (!current) return;
      rememberClosedPreview(current.transaction_id);
      selectedPreviewRef.current = null;
      selectedPreviewDraftRef.current = "";
      setSelectedPreview(null);
      setSelectedPreviewDraft("");
      setSelectedPreviewError(null);
      setSelectedPreviewBusy(previewCommandTransactionRef.current !== null);
    };
    const acceptSequence = (actionSequence: number, transactionId: string) => {
      const latest = latestActionSequenceRef.current;
      if (actionSequence < latest) return false;
      if (actionSequence === latest) {
        const latestTransactionId = latestActionTransactionRef.current;
        if (latestTransactionId && latestTransactionId !== transactionId) return false;
        latestActionTransactionRef.current = transactionId;
        return true;
      }

      latestActionSequenceRef.current = actionSequence;
      latestActionTransactionRef.current = transactionId;
      clearVisiblePreview();
      setSelectedPreviewError(null);
      setTextActionOutcome(null);
      if (selectedActionErrorRef.current && selectedActionErrorRef.current.action_sequence < actionSequence) {
        updateSelectedActionError(null);
      }
      return true;
    };
    const previewSubscription = listen<unknown>("selected-action://preview", (event) => {
      if (!active || !isSelectedActionPreview(event.payload)) return;
      const nextPreview = event.payload;
      if (!acceptSequence(nextPreview.action_sequence, nextPreview.transaction_id)) return;
      if (closedPreviewTransactionsRef.current.has(nextPreview.transaction_id)
        || terminalActionTransactionsRef.current.has(actionKey(nextPreview.action_sequence, nextPreview.transaction_id))) return;
      const current = selectedPreviewRef.current;
      if (current?.transaction_id === nextPreview.transaction_id) return;
      if (!current) {
        selectedPreviewRestoreRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      } else {
        rememberClosedPreview(current.transaction_id);
      }
      selectedPreviewRef.current = nextPreview;
      selectedPreviewDraftRef.current = nextPreview.final_text;
      setSelectedPreview(nextPreview);
      setSelectedPreviewDraft(nextPreview.final_text);
      setSelectedPreviewError(null);
      setSelectedPreviewBusy(previewCommandTransactionRef.current !== null);
      setTextActionOutcome(null);
      updateSelectedActionError(null);
    });
    const lifecycleSubscription = listen<unknown>("selected-action://lifecycle", (event) => {
      if (!active || !isSelectedActionLifecycle(event.payload)) return;
      const lifecycle = event.payload;
      if (!acceptSequence(lifecycle.action_sequence, lifecycle.transaction_id)) return;
      const actionKeyValue = actionKey(lifecycle.action_sequence, lifecycle.transaction_id);
      if (lifecycle.state === "started") {
        if (terminalActionTransactionsRef.current.has(actionKeyValue)) return;
        const current = selectedPreviewRef.current;
        if (current && current.action_sequence < lifecycle.action_sequence) clearVisiblePreview();
        return;
      }

      rememberTerminalAction(lifecycle.action_sequence, lifecycle.transaction_id);
      const current = selectedPreviewRef.current;
      const supersedesCurrent = current !== null && lifecycle.action_sequence > current.action_sequence;
      const matchesCurrent = current?.action_sequence === lifecycle.action_sequence
        && current.transaction_id === lifecycle.transaction_id;
      if (supersedesCurrent || matchesCurrent) clearVisiblePreview();

      const displayedError = selectedActionErrorRef.current;
      const sameDisplayedError = displayedError?.action_sequence === lifecycle.action_sequence
        && displayedError.transaction_id === lifecycle.transaction_id;
      if (lifecycle.state === "failed") {
        if (!sameDisplayedError) {
          updateSelectedActionError({
            action_sequence: lifecycle.action_sequence,
            transaction_id: lifecycle.transaction_id,
            code: "failed",
          });
        }
      } else if (sameDisplayedError) {
        updateSelectedActionError(null);
      }
    });
    const errorSubscription = listen<unknown>("selected-action://error", (event) => {
      if (!active || !isSelectedActionError(event.payload)) return;
      const actionError = event.payload;
      if (!acceptSequence(actionError.action_sequence, actionError.transaction_id)) return;
      rememberTerminalAction(actionError.action_sequence, actionError.transaction_id);
      const current = selectedPreviewRef.current;
      if (current && (actionError.action_sequence > current.action_sequence
        || (current.action_sequence === actionError.action_sequence && current.transaction_id === actionError.transaction_id))) {
        clearVisiblePreview();
      }
      setTextActionOutcome(null);
      updateSelectedActionError(actionError);
    });
    return () => {
      active = false;
      for (const subscription of [previewSubscription, lifecycleSubscription, errorSubscription]) {
        void subscription.then((unlisten) => unlisten()).catch(() => undefined);
      }
    };
  }, [rememberClosedPreview, rememberTerminalAction, updateSelectedActionError]);

  useEffect(() => {
    let active = true;
    const subscription = listen<{ pair_key: string; pair_keys?: string[]; before: string; after: string }>("learn_pairs://promoted", (event) => {
      if (!active) return;
      learnToastRef.current = event.payload;
      setLearnToast(event.payload);
      setLearnUndoError(null);
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let active = true;
    const subscription = listen<Settings>("settings://changed", (event) => {
      if (!active) return;
      setSettings((current) => (current ? { ...current, ...event.payload } : event.payload));
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const refreshAudioInputDevice = useCallback(async (selectedDevice: string) => {
    if (audioDeviceCheckInFlight.current) return;
    audioDeviceCheckInFlight.current = true;
    try {
      const devices = await invoke<AudioInputDevice[]>("get_audio_input_devices");
      setAudioInputDevices(devices);
      const selected = selectedDevice.trim();
      setAudioInputDevice(selected || devices.find((device) => device.is_default)?.name || null);
    } catch {
      setAudioInputDevices([]);
      setAudioInputDevice(null);
    } finally {
      audioDeviceCheckInFlight.current = false;
    }
  }, []);

  useEffect(() => {
    const toggleDebug = (event: KeyboardEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (event.repeat || event.isComposing || target?.closest("input, textarea, select, [contenteditable]:not([contenteditable=false]), [role=textbox]")) return;
      if (event.metaKey && event.shiftKey && !event.altKey && !event.ctrlKey && event.key.toLowerCase() === "d" && settings) {
        event.preventDefault();
        const enabled = !settings.debug_mode;
        save({ debug_mode: enabled });
        setView(enabled ? "debug" : "general");
      }
    };
    window.addEventListener("keydown", toggleDebug);
    return () => window.removeEventListener("keydown", toggleDebug);
  }, [settings, save]);

  const load = async () => {
    setLoadingError(null);
    try {
      const [nextSettings, nextPermissions] = await Promise.all([
        invoke<Settings>("get_settings"),
        invoke<Permissions>("check_permissions"),
      ]);
      setSettings(nextSettings);
      setShowOnboarding(!nextSettings.onboarded);
      setHistory([]);
      setHistoryHasMore(false);
      setHistoryError(null);
      historyQueryRef.current = "";
      setPermissions(nextPermissions);
      // Optional notes never gate settings or onboarding.
      void invoke<{ should_show: boolean; version: string; notes: string | null }>("get_whats_new_status")
        .then((status) => { if (status?.should_show) setWhatsNew(status); })
        .catch(() => undefined);
    } catch (reason) {
      setLoadingError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    if (showOnboarding || view !== "permissions") return;

    const pollPermissions = () => {
      if (document.visibilityState === "visible") void refreshPermissions();
    };
    pollPermissions();
    const timer = window.setInterval(pollPermissions, 1000);
    window.addEventListener("focus", pollPermissions);
    document.addEventListener("visibilitychange", pollPermissions);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", pollPermissions);
      document.removeEventListener("visibilitychange", pollPermissions);
    };
  }, [refreshPermissions, showOnboarding, view]);

  useEffect(() => {
    if (!settings || showOnboarding || view !== "system") return;

    const pollAudioInputDevice = () => {
      if (document.visibilityState === "visible") void refreshAudioInputDevice(settings.input_device ?? "");
    };
    pollAudioInputDevice();
    const timer = window.setInterval(pollAudioInputDevice, 1000);
    window.addEventListener("focus", pollAudioInputDevice);
    document.addEventListener("visibilitychange", pollAudioInputDevice);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", pollAudioInputDevice);
      document.removeEventListener("visibilitychange", pollAudioInputDevice);
    };
  }, [refreshAudioInputDevice, settings, showOnboarding, view]);

  const reloadHistory = useCallback((query = historyQueryRef.current) => {
    const normalizedQuery = query.trim();
    const requestId = ++historyRequestRef.current;
    setHistoryError(null);
    setHistoryLoading(true);
    void invoke<HistoryPage>("get_history", {
      limit: 50,
      query: normalizedQuery || null,
    })
      .then((page) => {
        if (requestId !== historyRequestRef.current) return;
        setHistory(page.items);
        setHistoryHasMore(page.has_more);
      })
      .catch((reason) => {
        if (requestId !== historyRequestRef.current) return;
        setHistoryError(t("历史记录加载失败：") + String(reason));
      })
      .finally(() => {
        if (requestId === historyRequestRef.current) setHistoryLoading(false);
      });
  }, [t]);

  const loadMoreHistory = useCallback(() => {
    if (historyLoading || !historyHasMore) return;
    const beforeId = history[history.length - 1]?.id;
    if (beforeId === undefined) return;
    const requestId = ++historyRequestRef.current;
    setHistoryError(null);
    setHistoryLoading(true);
    void invoke<HistoryPage>("get_history", {
      before_id: beforeId,
      limit: 50,
      query: historyQueryRef.current.trim() || null,
    })
      .then((page) => {
        if (requestId !== historyRequestRef.current) return;
        setHistory((current) => [...current, ...page.items]);
        setHistoryHasMore(page.has_more);
      })
      .catch((reason) => {
        if (requestId !== historyRequestRef.current) return;
        setHistoryError(t("加载更早记录失败：") + String(reason));
      })
      .finally(() => {
        if (requestId === historyRequestRef.current) setHistoryLoading(false);
      });
  }, [history, historyHasMore, historyLoading, t]);

  const searchHistory = useCallback((query: string) => {
    historyQueryRef.current = query;
    reloadHistory(query);
  }, [reloadHistory]);

  useEffect(() => {
    if (view !== "history") historyQueryRef.current = "";
  }, [view]);

  useEffect(() => {
    if (!settings || showOnboarding || view !== "history") return;
    reloadHistory();
  }, [reloadHistory, settings, showOnboarding, view]);

  useEffect(() => {
    let active = true;
    let receivedEvent = false;
    const applyPhase = (phase: string) => setDictationBusy(["starting", "recording", "recording_limited", "stopping", "processing", "rate_limited"].includes(phase));
    const subscription = listen<{ state?: string }>("dictation://state", (event) => {
      if (!active) return;
      receivedEvent = true;
      applyPhase(event.payload.state ?? "idle");
    });
    void subscription.then(() => invoke<string>("get_dictation_phase")).then((phase) => {
      if (active && !receivedEvent && typeof phase === "string") applyPhase(phase);
    }).catch(() => undefined);
    return () => { active = false; void subscription.then((unlisten) => unlisten()).catch(() => undefined); };
  }, []);

  useEffect(() => {
    if (showOnboarding) return;
    let active = true;
    const subscription = listen<{ state?: string }>("dictation://state", (event) => {
      if (!active) return;
      if (view !== "history") return;
      if (!event.payload.state || !["done", "unverified", "copied", "degraded", "error"].includes(event.payload.state)) return;
      reloadHistory();
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [reloadHistory, showOnboarding, view]);

  useEffect(() => {
    let clearTimer: number | undefined;
    let active = true;
    const subscription = listen<string>("dictation://error", (event) => {
      if (!active) return;
      setRuntimeError(event.payload);
      if (clearTimer !== undefined) window.clearTimeout(clearTimer);
      clearTimer = window.setTimeout(() => setRuntimeError(null), 6_000);
    });
    return () => {
      active = false;
      if (clearTimer !== undefined) window.clearTimeout(clearTimer);
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  const closeSelectedPreview = useCallback((transactionId: string) => {
    if (selectedPreviewRef.current?.transaction_id !== transactionId) return false;
    selectedPreviewRef.current = null;
    selectedPreviewDraftRef.current = "";
    setSelectedPreview(null);
    setSelectedPreviewDraft("");
    setSelectedPreviewError(null);
    setSelectedPreviewBusy(false);
    return true;
  }, []);
  const cancelSelectedPreview = useCallback(async () => {
    const preview = selectedPreviewRef.current;
    if (!preview) return;
    const transactionId = preview.transaction_id;
    rememberTerminalAction(preview.action_sequence, transactionId);
    closeSelectedPreview(transactionId);
    try {
      await invoke(
        preview.kind === "screen" ? "cancel_screen_action_preview" : "cancel_selected_action_preview",
        { transaction_id: transactionId },
      );
    } catch {
      // The transaction is closed in the UI even when its cancellation reply is late.
    }
  }, [closeSelectedPreview, rememberTerminalAction]);

  if (loadingError) {
    return (
      <main className={`flex min-h-screen items-center justify-center ${colors.bg.base} p-8 ${colors.text.primary}`}>
        <div className={`max-w-md ${radius.card} border ${colors.border} ${colors.bg.card} p-6`}>
          <h1 className="text-lg font-semibold">{t("VoiceFlow 暂时无法启动")}</h1>
          <p className="mt-2 text-sm text-secondary">{t("初始化设置或权限失败，请重试。")}</p>
          <p role="alert" className="mt-3 break-words text-xs text-error-ink">{loadingError}</p>
          <button type="button" onClick={() => void load()} className={`mt-5 ${buttonClass}`}>{t("重新检测")}</button>
        </div>
      </main>
    );
  }
  if (!settings) return <main className={`min-h-screen ${colors.bg.base} p-12 ${colors.text.secondary}`} role="status" aria-live="polite">{t("正在加载 VoiceFlow…")}</main>;
  if (showOnboarding) {
    return (
      <Onboarding
        settings={settings}
        onFinish={(next) => {
          setSettings(next as Settings);
          setShowOnboarding(false);
        }}
        onSkipToSettings={() => setShowOnboarding(false)}
      />
    );
  }

  const changeInterfaceLanguage = (ui_language: UiLanguagePreference) => {
    setLanguage(resolveUiLanguage(ui_language));
    save({ ui_language });
    void emit("settings://ui-language", ui_language);
  };

  const saveApiKey = async (apiKey: string) => {
    try {
      await flushPendingSave();
      await invoke("update_settings_patch", { patch: { api_key: apiKey.trim() } });
      const next = await invoke<Settings>("get_settings");
      setSettings(next);
      setSaveError(null);
    } catch (reason) {
      setSaveError(friendlySettingsError(reason, t));
      throw reason;
    }
  };
  const removeApiKey = async () => {
    await flushPendingSave();
    const next = await invoke<Settings>("remove_api_key");
    setSettings(next);
    setSaveError(null);
  };
  const saveAsrApiKey = async (apiKey: string, asrBaseUrl?: string) => {
    try {
      await flushPendingSave();
      const patch: Record<string, string> = { asr_api_key: apiKey.trim() };
      if (asrBaseUrl !== undefined) patch.asr_base_url = asrBaseUrl;
      await invoke("update_settings_patch", { patch });
      const next = await invoke<Settings>("get_settings");
      setSettings(next);
      setSaveError(null);
    } catch (reason) {
      setSaveError(friendlySettingsError(reason, t));
      throw reason;
    }
  };
  const removeAsrApiKey = async () => {
    await flushPendingSave();
    const next = await invoke<Settings>("remove_asr_api_key");
    setSettings(next);
    setSaveError(null);
  };
  const removeCleanupApiKey = async () => {
    await flushPendingSave();
    const next = await invoke<Settings>("remove_cleanup_api_key");
    setSettings(next);
    setSaveError(null);
  };
  const commitEngine = async (patch: Record<string, unknown>) => {
    await flushPendingSave();
    await invoke("update_settings_patch", { patch });
    const next = await invoke<Settings>("get_settings");
    setSettings(next);
    setSaveError(null);
  };
  const removeProviderKey = async (provider: string) => {
    await flushPendingSave();
    const next = await invoke<Settings>("remove_provider_key", { provider });
    setSettings(next);
    setSaveError(null);
  };
  const submitSelectedPreview = async (action: "confirm" | "copy") => {
    const preview = selectedPreviewRef.current;
    if (!preview || !selectedPreviewDraftRef.current.trim() || previewCommandTransactionRef.current) return;
    const transactionId = preview.transaction_id;
    const command = action === "copy"
      ? (preview.kind === "screen" ? "copy_screen_action_preview" : "copy_selected_action_preview")
      : (preview.kind === "screen" ? "confirm_screen_action_preview" : "confirm_selected_action_preview");
    previewCommandTransactionRef.current = transactionId;
    setSelectedPreviewBusy(true);
    setSelectedPreviewError(null);
    try {
      const outcome = await invoke<TextActionOutcome>(command, {
        transaction_id: transactionId,
        final_text: selectedPreviewDraftRef.current,
      });
      const validOutcome = action === "copy"
        ? outcome === "copied"
        : outcome === "replaced" || outcome === "unverified" || outcome === "copied" || outcome === "copied_target_changed";
      if (!validOutcome) throw new Error("Unexpected text action result");

      const current = selectedPreviewRef.current;
      if (current?.transaction_id === transactionId) {
        rememberTerminalAction(preview.action_sequence, transactionId);
        closeSelectedPreview(transactionId);
        setTextActionOutcome(outcome);
      } else if (current === null
        && latestActionSequenceRef.current === preview.action_sequence
        && latestActionTransactionRef.current === transactionId) {
        // A late success after the dialog closes still needs a truthful outcome.
        setTextActionOutcome(outcome);
      }
    } catch {
      if (selectedPreviewRef.current?.transaction_id === transactionId) {
        setSelectedPreviewError(t("无法完成这个文字操作。请重试或取消。"));
      }
    } finally {
      if (previewCommandTransactionRef.current === transactionId) {
        previewCommandTransactionRef.current = null;
      }
      setSelectedPreviewBusy(false);
    }
  };
  const confirmSelectedPreview = () => void submitSelectedPreview("confirm");
  const copySelectedPreview = () => void submitSelectedPreview("copy");
  const undoLearning = async () => {
    if (!learnToast || learnUndoBusy) return;
    const toast = learnToast;
    setLearnUndoBusy(true);
    setLearnUndoError(null);
    try {
      await Promise.all((toast.pair_keys?.length ? toast.pair_keys : [toast.pair_key]).map((pairKey) => invoke("undo_learn_pair", { pairKey })));
      if (learnToastRef.current === toast) setLearnToast(null);
    } catch {
      if (learnToastRef.current === toast) setLearnUndoError(t("撤销未完成，请重试。"));
    } finally {
      setLearnUndoBusy(false);
    }
  };
  const saveFailure = saveError ? { fields: unsavedFields, message: saveError } : null;
  return (
    <main className={`vf-settings relative h-screen overflow-hidden ${colors.bg.base} ${colors.text.primary}`}>
      <div className="vf-settings-frame flex h-full w-full" aria-hidden={selectedPreview ? true : undefined} inert={selectedPreview ? true : undefined}>
        <aside className={`vf-settings-sidebar flex min-h-0 shrink-0 flex-col border-r ${colors.border} px-3 py-6`}>
          <div className="flex shrink-0 items-center gap-2.5 px-2">
            <img src={brandIconSrc} alt="VoiceFlow" className="h-8 w-8 shrink-0" />
            <p className="truncate text-sm font-semibold tracking-tight text-primary" translate="no">VoiceFlow</p>
          </div>

          {!settings.onboarded && (
            <button type="button" onClick={() => setShowOnboarding(true)} className={`mt-6 w-full shrink-0 ${buttonClass} text-xs`}>
              {t("继续完成设置")}
            </button>
          )}

          <nav aria-label={t("设置页面")} className="settings-scrollbar mt-6 min-h-0 flex-1 space-y-6 overflow-y-auto pb-1">
            {(settings.debug_mode ? [...navigationGroups, { label: "调试", items: [{ id: "debug" as View, label: "调试设置", icon: Bug }] }] : navigationGroups).map((group) => (
              <div key={group.label}>
                <h3 className="mb-2 px-3 text-xs font-semibold text-tertiary">{t(group.label)}</h3>
                <div className="space-y-0.5">
                  {group.items.map(({ id, label, icon: Icon }) => {
                    const active = view === id;
                    return (
                      <button
                        key={id}
                        type="button"
                        onClick={() => setView(id)}
                        aria-current={active ? "page" : undefined}
                        className={`flex min-h-9 w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm transition-colors duration-150 ${focusRingClass} ${active ? "bg-accent-soft font-medium text-accent" : "text-secondary hover:bg-card hover:text-primary"}`}
                      >
                        <Icon size={18} strokeWidth={1.7} aria-hidden="true" className={`shrink-0 ${active ? "text-accent" : "text-tertiary"}`} />
                        <span className="min-w-0 flex-1 leading-5">{t(label)}</span>
                        {id === "engine" && !isEngineConnected(settings) && (
                          <span role="img" aria-label={t("需要配置语音服务")} title={t("需要配置语音服务")} className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" />
                        )}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </nav>
        </aside>

        <section ref={settingsContentRef} className="vf-settings-scroll-pane settings-scrollbar min-h-0 min-w-0 flex-1 overflow-y-auto">
          <div className="vf-page-content min-h-full w-full">
            {saveError && (
              <SettingsAlert onRetry={retryPendingSave} retryLabel={t("重试")}>{t("设置保存失败：")}{saveError}</SettingsAlert>
            )}
            {runtimeError && (
              <SettingsAlert>
                {t("语音输入失败：")}
                {runtimeError.startsWith("Microphone permission is required")
                  ? t("请在系统权限中允许 VoiceFlow 使用麦克风后重试。")
                  : runtimeError}
                {runtimeError.startsWith("Microphone permission is required") && (
                  <button type="button" onClick={() => setView("permissions")} className="ml-2 font-medium underline underline-offset-2">
                    {t("打开系统权限")}
                  </button>
                )}
              </SettingsAlert>
            )}
            {selectedActionError && <p role="alert" className="mb-4 rounded-lg border border-error/30 bg-error/5 px-4 py-3 text-sm text-error-ink">{t(actionErrorLabels[selectedActionError.code])}</p>}
            {textActionOutcome && <p role="status" aria-live="polite" className="mb-4 rounded-lg border border-border bg-elevated px-4 py-3 text-sm text-primary">{t(actionOutcomeLabels[textActionOutcome])}</p>}
            {learnToast && (
              <div role="status" className="mb-4 flex items-center gap-3 rounded-lg border border-border bg-elevated px-4 py-3 text-sm text-primary">
                <span className="min-w-0 flex-1">{learnUndoError ?? `${t("已学")} ${learnToast.before}→${learnToast.after}`}</span>
                <button type="button" disabled={learnUndoBusy} className={buttonClass} onClick={() => void undoLearning()}>{t(learnUndoError ? "重试撤销" : "撤销")}</button>
                <button type="button" disabled={learnUndoBusy} className={compactButtonClass} onClick={() => { setLearnToast(null); setLearnUndoError(null); }}>{t("关闭")}</button>
              </div>
            )}
            <AnimatedContent key={view} className="w-full">
              {view === "history" ? <History items={history} reload={() => reloadHistory()} hasMore={historyHasMore} loading={historyLoading} onLoadMore={loadMoreHistory} error={historyError} onRetry={() => reloadHistory()} onQueryChange={searchHistory} />
                : view === "smart" ? <ContextSettings saveFailure={saveFailure} automationOnly writingModes={settings.writing_modes} onWritingModesChange={(writingModes) => save({ writing_modes: writingModes })} outputMode={settings.output_mode} translationTargetLanguage={settings.translation_target_language} onOutputModeChange={(outputMode) => save({ output_mode: outputMode })} onTranslationTargetLanguageChange={(language) => save({ translation_target_language: language })} cleanupIntensity={settings.cleanup_intensity ?? "auto"} onCleanupIntensityChange={(cleanup_intensity) => save({ cleanup_intensity })} windowOcrEnabled={settings.window_ocr_enabled ?? false} onWindowOcrEnabledChange={(window_ocr_enabled) => save({ window_ocr_enabled })} visionProvider={settings.vision_provider ?? ""} visionModel={settings.vision_model ?? ""} onVisionProviderChange={(vision_provider) => save({ vision_provider })} onVisionModelChange={(vision_model) => save({ vision_model })} accurateAsrProvider={settings.accurate_asr_provider ?? "groq"} accurateAsrModel={settings.accurate_asr_model ?? ""} accurateAsrBaseUrl={settings.accurate_asr_base_url ?? ""} onAccurateAsrProviderChange={(accurate_asr_provider) => save({ accurate_asr_provider })} onAccurateAsrModelChange={(accurate_asr_model) => save({ accurate_asr_model })} onAccurateAsrBaseUrlChange={(accurate_asr_base_url) => save({ accurate_asr_base_url })} />
                  : view === "writing" ? <ContextSettings saveFailure={saveFailure} writingModes={settings.writing_modes} onWritingModesChange={(writingModes) => save({ writing_modes: writingModes })} />
                    : view === "snippets" ? <SnippetsSettings snippets={settings.snippets} onChange={(snippets) => save({ snippets })} />
                      : view === "dictionary" ? <DictionarySettings settings={settings} save={save} />
                        : view === "permissions" ? <PermissionsSettings permissions={permissions} onRefresh={refreshPermissions} />
                          : view === "debug" ? <DebugSettings settings={settings} save={save} />
                          : view === "system" ? <SystemSettings settings={settings} audioInputDevice={audioInputDevice} audioInputDevices={audioInputDevices} save={save} onUiLanguageChange={changeInterfaceLanguage} />
                            : view === "engine" ? <EngineSettings settings={settings} save={save} saveApiKey={saveApiKey} removeApiKey={removeApiKey} saveAsrApiKey={saveAsrApiKey} removeAsrApiKey={removeAsrApiKey} removeCleanupApiKey={removeCleanupApiKey} removeProviderKey={removeProviderKey} commitEngine={commitEngine} />
                              : <RecordingSettings settings={settings} save={save} saveFailure={saveFailure} dictationBusy={dictationBusy} />}
            </AnimatedContent>
          </div>
        </section>
      </div>
      {whatsNew && !showOnboarding && <WhatsNewDialog payload={whatsNew} onDismiss={() => { save({ whats_new_last_seen_version: whatsNew.version }); setWhatsNew(null); }} />}
      {selectedPreview && (
        <SelectedPreviewDialog
          preview={selectedPreview}
          draft={selectedPreviewDraft}
          onDraftChange={(value) => {
            selectedPreviewDraftRef.current = value;
            setSelectedPreviewDraft(value);
          }}
          onCancel={() => void cancelSelectedPreview()}
          onCopy={() => void copySelectedPreview()}
          onConfirm={() => void confirmSelectedPreview()}
          busy={selectedPreviewBusy}
          error={selectedPreviewError}
          restoreFocusRef={selectedPreviewRestoreRef}
        />
      )}
    </main>
  );
}
