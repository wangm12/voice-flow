import "./App.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { AudioLines, BookMarked, CloudCog, FileText, History as HistoryIcon, Monitor, PenLine, ShieldCheck, Sparkles } from "lucide-react";
import { History, type HistoryItem } from "./components/History/History";
import { Onboarding } from "./components/Onboarding/Onboarding";
import { ContextSettings } from "./components/ContextSettings";
import { AnimatedContent } from "./components/ReactBits/AnimatedContent";
import { SettingsAlert } from "./components/SettingsLayout";
import { SelectedPreviewDialog } from "./components/SelectedPreviewDialog";
import { PermissionsSettings, type Permissions } from "./components/PermissionsSettings";
import { SnippetsSettings } from "./components/SnippetsSettings";
import { DictionarySettings } from "./components/DictionarySettings";
import { EngineSettings } from "./components/settings/EngineSettings";
import { RecordingSettings } from "./components/settings/RecordingSettings";
import { SystemSettings } from "./components/settings/SystemSettings";
import { useSettingsPersistence } from "./hooks/useSettingsPersistence";
import { resolveUiLanguage, type UiLanguagePreference, useI18n } from "./lib/i18n";
import { friendlySettingsError } from "./lib/settingsError";
import { colors, radius, buttonClass } from "./lib/theme";
import type { AudioInputDevice, Settings } from "./types/settings";

type HistoryPage = { items: HistoryItem[]; has_more: boolean };
type SelectedActionPreview = { selected_text: string; transcript: string; final_text: string };
type View = "general" | "engine" | "dictionary" | "history" | "smart" | "writing" | "permissions" | "system" | "snippets";
const brandIconSrc = "/voiceflow-icon-ui.svg";
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
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [view, setView] = useState<View>("general");
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
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [learnToast, setLearnToast] = useState<{ pair_key: string; before: string; after: string } | null>(null);
  const [selectedPreview, setSelectedPreview] = useState<SelectedActionPreview | null>(null);
  const [selectedPreviewDraft, setSelectedPreviewDraft] = useState("");
  const selectedPreviewRestoreRef = useRef<HTMLElement | null>(null);
  const permissionCheckInFlight = useRef(false);
  const audioDeviceCheckInFlight = useRef(false);
  const formatSettingsError = useCallback((reason: unknown) => friendlySettingsError(reason, t), [t]);
  const {
    save,
    saveError,
    setSaveError,
    flushPendingSave,
    retryPendingSave,
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
    const subscription = listen<SelectedActionPreview>("selected-action://preview", (event) => {
      if (!active) return;
      selectedPreviewRestoreRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setSelectedPreview(event.payload);
      setSelectedPreviewDraft(event.payload.final_text);
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let active = true;
    const subscription = listen<{ pair_key: string; before: string; after: string }>("learn_pairs://promoted", (event) => {
      if (!active) return;
      setLearnToast(event.payload);
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
    if (showOnboarding) return;
    let active = true;
    const subscription = listen<{ state?: string }>("dictation://state", (event) => {
      if (!active || view !== "history") return;
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

  const closeSelectedPreview = useCallback(() => {
    setSelectedPreview(null);
    setSelectedPreviewDraft("");
  }, []);
  const cancelSelectedPreview = useCallback(async () => {
    try {
      await invoke("cancel_selected_action_preview");
    } finally {
      closeSelectedPreview();
    }
  }, [closeSelectedPreview]);

  if (loadingError) {
    return (
      <main className={`flex min-h-screen items-center justify-center ${colors.bg.base} p-8 ${colors.text.primary}`}>
        <div className={`max-w-md ${radius.card} border ${colors.border} ${colors.bg.card} p-6`}>
          <h1 className="text-lg font-semibold">{t("VoiceFlow 暂时无法启动")}</h1>
          <p className="mt-2 text-sm text-secondary">{t("初始化设置或权限失败，请重试。")}</p>
          <p role="alert" className="mt-3 break-words text-xs text-error">{loadingError}</p>
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
  const confirmSelectedPreview = async () => {
    try {
      await invoke("confirm_selected_action_preview", { final_text: selectedPreviewDraft });
      closeSelectedPreview();
    } catch (reason) {
      setRuntimeError(String(reason));
    }
  };
  const copySelectedPreview = async () => {
    try {
      await invoke("copy_selected_action_preview", { final_text: selectedPreviewDraft });
      closeSelectedPreview();
    } catch (reason) {
      setRuntimeError(String(reason));
    }
  };
  return (
    <main className={`relative h-screen overflow-hidden ${colors.bg.base} ${colors.text.primary}`}>
      <div className="mx-auto flex h-full w-full max-w-6xl" aria-hidden={selectedPreview ? true : undefined} inert={selectedPreview ? true : undefined}>
        <aside className={`flex w-56 shrink-0 flex-col border-r ${colors.border} px-3 py-6`}>
          <div className="flex items-center gap-2.5 px-2">
            <img src={brandIconSrc} alt="VoiceFlow" className="h-8 w-8 shrink-0" />
            <p className="truncate text-sm font-semibold tracking-tight text-primary" translate="no">VoiceFlow</p>
          </div>

          {!settings.onboarded && (
            <button type="button" onClick={() => setShowOnboarding(true)} className={`mt-6 w-full ${buttonClass} text-xs`}>
              {t("继续完成设置")}
            </button>
          )}

          <nav aria-label={t("设置页面")} className="mt-8 space-y-6">
            {navigationGroups.map((group) => (
              <div key={group.label}>
                <h3 className="mb-2 px-3 text-[10px] font-semibold uppercase tracking-[0.12em] text-tertiary">{t(group.label)}</h3>
                <div className="space-y-0.5">
                  {group.items.map(({ id, label, icon: Icon }) => {
                    const active = view === id;
                    return (
                      <button
                        key={id}
                        type="button"
                        onClick={() => setView(id)}
                        aria-current={active ? "page" : undefined}
                        className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition-colors duration-150 ${active ? "bg-elevated/60 font-medium text-primary" : "text-secondary hover:bg-card hover:text-primary"}`}
                      >
                        <Icon size={16} strokeWidth={1.7} aria-hidden="true" className={active ? "text-primary" : "text-tertiary"} />
                        <span className="min-w-0 flex-1 truncate">{t(label)}</span>
                        {id === "engine" && !settings.api_key_configured && (
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

        <section className="settings-scrollbar min-h-0 min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto min-h-screen w-full max-w-3xl px-6 py-9 sm:px-10 sm:py-12">
            {saveError && (
              <SettingsAlert onRetry={retryPendingSave} retryLabel={t("重试")}>{t("设置保存失败：")}{saveError}</SettingsAlert>
            )}
            {runtimeError && <SettingsAlert>{t("语音输入失败：")}{runtimeError}</SettingsAlert>}
            {learnToast && (
              <div role="status" className="mb-4 flex items-center gap-3 rounded-lg border border-border bg-elevated px-4 py-3 text-sm text-primary">
                <span className="min-w-0 flex-1">{t("已学")} {learnToast.before}→{learnToast.after}</span>
                <button type="button" className={buttonClass} onClick={() => {
                  void invoke("undo_learn_pair", { pairKey: learnToast.pair_key }).then(async () => {
                    const next = await invoke<Settings>("get_settings");
                    setSettings(next);
                    setLearnToast(null);
                  });
                }}>{t("撤销")}</button>
                <button type="button" className="rounded-lg px-2 py-1 text-xs text-tertiary" onClick={() => setLearnToast(null)}>{t("关闭")}</button>
              </div>
            )}
            <AnimatedContent key={view} className="w-full">
              {view === "history" ? <History items={history} reload={() => reloadHistory()} hasMore={historyHasMore} loading={historyLoading} onLoadMore={loadMoreHistory} error={historyError} onRetry={() => reloadHistory()} onQueryChange={searchHistory} />
                : view === "smart" ? <ContextSettings automationOnly writingModes={settings.writing_modes} onWritingModesChange={(writingModes) => save({ writing_modes: writingModes })} outputMode={settings.output_mode} translationTargetLanguage={settings.translation_target_language} onOutputModeChange={(outputMode) => save({ output_mode: outputMode })} onTranslationTargetLanguageChange={(language) => save({ translation_target_language: language })} />
                  : view === "writing" ? <ContextSettings writingModes={settings.writing_modes} onWritingModesChange={(writingModes) => save({ writing_modes: writingModes })} />
                    : view === "snippets" ? <SnippetsSettings snippets={settings.snippets} onChange={(snippets) => save({ snippets })} />
                      : view === "dictionary" ? <DictionarySettings settings={settings} save={save} />
                        : view === "permissions" ? <PermissionsSettings permissions={permissions} onRefresh={refreshPermissions} />
                          : view === "system" ? <SystemSettings settings={settings} audioInputDevice={audioInputDevice} audioInputDevices={audioInputDevices} save={save} onUiLanguageChange={changeInterfaceLanguage} />
                            : view === "engine" ? <EngineSettings settings={settings} save={save} saveApiKey={saveApiKey} removeApiKey={removeApiKey} saveAsrApiKey={saveAsrApiKey} removeAsrApiKey={removeAsrApiKey} removeCleanupApiKey={removeCleanupApiKey} removeProviderKey={removeProviderKey} commitEngine={commitEngine} />
                              : <RecordingSettings settings={settings} save={save} />}
            </AnimatedContent>
          </div>
        </section>
      </div>
      {selectedPreview && (
        <SelectedPreviewDialog
          preview={selectedPreview}
          draft={selectedPreviewDraft}
          onDraftChange={setSelectedPreviewDraft}
          onCancel={() => void cancelSelectedPreview()}
          onCopy={() => void copySelectedPreview()}
          onConfirm={() => void confirmSelectedPreview()}
          restoreFocusRef={selectedPreviewRestoreRef}
        />
      )}
    </main>
  );
}
