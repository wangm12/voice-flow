import "./App.css";
import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { AudioLines, BookMarked, ChevronDown, CloudCog, FileText, History as HistoryIcon, KeyRound, Languages, Monitor, Moon, ShieldCheck, Sparkles, Sun, Upload } from "lucide-react";
import { History, type HistoryItem } from "./components/History/History";
import { Onboarding } from "./components/Onboarding/Onboarding";
import { PasswordInput } from "./components/PasswordInput";
import { ValidationStatus } from "./components/Onboarding/ValidationStatus";
import { HotkeyRecorder } from "./components/HotkeyRecorder";
import { HotkeyUsageGuide } from "./components/HotkeyUsageGuide";
import { ContextSettings, type WritingMode } from "./components/ContextSettings";
import { AnimatedContent } from "./components/ReactBits/AnimatedContent";
import { Toggle } from "./components/Toggle";
import { SettingsAlert, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "./components/SettingsLayout";
import { PermissionsSettings, type Permissions } from "./components/PermissionsSettings";
import { SnippetsSettings, type Snippet } from "./components/SnippetsSettings";
import { MAX_DICTIONARY_FILE_BYTES, mergeDictionary, parseDictionaryText } from "./lib/dictionaryImport";
import { resolveUiLanguage, type UiLanguagePreference, useI18n } from "./lib/i18n";
import { colors, radius, buttonClass, secondaryButtonClass } from "./lib/theme";

type Settings = {
  schema_version: number;
  api_key_configured: boolean;
  api_key_hint: string | null;
  asr_model: string;
  cleanup_model: string;
  language: string;
  ui_language: UiLanguagePreference;
  theme: "system" | "light" | "dark";
  dictionary: string[];
  chunk_threshold_secs: number;
  chunk_length_secs: number;
  long_output_mode: string;
  keep_audio_days: number;
  keep_history_days: number;
  onboarded: boolean;
  cleanup_enabled: boolean;
  show_tray_icon: boolean;
  hotkey: string;
  activation_mode: string;
  hotkey_error?: string | null;
  context_enabled: boolean;
  browser_access_enabled: boolean;
  context_mappings: unknown[];
  writing_modes: WritingMode[];
  snippets: Snippet[];
  output_mode: string;
  translation_target_language: string;
  selected_action_hotkey?: string;
  selected_actions_enabled?: boolean;
  input_device: string;
};
type HistoryPage = { items: HistoryItem[]; has_more: boolean };
type AudioInputDevice = { name: string; is_default: boolean };
type View = "general" | "engine" | "dictionary" | "history" | "smart" | "permissions" | "system" | "snippets";

function friendlySettingsError(reason: unknown, translate: (source: string) => string): string {
  const message = reason instanceof Error ? reason.message : String(reason);
  const normalized = message.toLowerCase();
  if (
    normalized.includes("credential_storage")
    || normalized.includes("failed to store api key securely")
    || normalized.includes("credential write")
    || normalized.includes("keychain")
  ) {
    return translate("无法保存到这台 Mac 的钥匙串。请重启 VoiceFlow 后再试。");
  }
  if (normalized.includes("api key validation failed") || normalized.includes("invalid")) {
    return translate("这个 Groq API Key 无效，请检查后重试。");
  }
  if (normalized.includes("rate_limited") || normalized.includes("请求过频")) {
    return translate("验证请求过频，请稍后再试。");
  }
  if (normalized.includes("network") || normalized.includes("timeout")) {
    return translate("暂时无法连接 Groq，请检查网络后重试。");
  }
  return translate(message);
}
const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent`;
const selectClass = `${controlClass} w-32`;
const brandIconSrc = "/voiceflow-icon-ui.svg";
const retentionOptions = [
  { value: 0, label: "立即清理" },
  { value: 1, label: "1 天" },
  { value: 7, label: "1 周" },
  { value: 30, label: "1 月" },
  { value: 365, label: "1 年" },
] as const;
const historyRetentionOptions = [
  { value: 7, label: "1 周" },
  { value: 30, label: "1 月" },
  { value: 90, label: "3 月" },
  { value: 365, label: "1 年" },
  { value: 3650, label: "10 年" },
  { value: 0, label: "永久" },
] as const;
const cleanupModelOptions = [
  { value: "openai/gpt-oss-20b", label: "GPT-OSS 20B", note: "默认 · 更快" },
  { value: "openai/gpt-oss-120b", label: "GPT-OSS 120B", note: "质量更高 · 较慢" },
] as const;
const navigationGroups: { label: string; items: { id: View; label: string; icon: typeof AudioLines }[] }[] = [
  {
    label: "核心设置",
    items: [
      { id: "general", label: "录音与输出", icon: AudioLines },
      { id: "smart", label: "智能整理", icon: Sparkles },
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
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [audioInputDevice, setAudioInputDevice] = useState<string | null>(null);
  const [audioInputDevices, setAudioInputDevices] = useState<AudioInputDevice[]>([]);
  const [loadingError, setLoadingError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const saveRevision = useRef(0);
  const pendingPatch = useRef<Partial<Settings>>({});
  const permissionCheckInFlight = useRef(false);
  const audioDeviceCheckInFlight = useRef(false);

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

  const persistPatch = useCallback((patch: Partial<Settings>, revision: number) => {
    if (Object.keys(patch).length === 0) return;
    saveQueue.current = saveQueue.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await invoke("update_settings_patch", { patch });
          if (saveRevision.current === revision && Object.keys(pendingPatch.current).length === 0) {
            setSaveError(null);
          }
        } catch (reason) {
          // Requeue even when a newer patch was scheduled while this request
          // was in flight; the newer patch may not contain every field from
          // the failed request.
          pendingPatch.current = { ...patch, ...pendingPatch.current };
          setSaveError(friendlySettingsError(reason, t));
        }
      });
  }, [t]);

  const flushPendingSave = useCallback(() => {
    if (saveTimer.current) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const patch = pendingPatch.current;
    pendingPatch.current = {};
    persistPatch(patch, saveRevision.current);
    return saveQueue.current;
  }, [persistPatch]);

  const retryPendingSave = useCallback(() => {
    const patch = pendingPatch.current;
    if (Object.keys(patch).length === 0) return;
    pendingPatch.current = {};
    saveRevision.current += 1;
    persistPatch(patch, saveRevision.current);
  }, [persistPatch]);

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
      setPermissions(nextPermissions);
    } catch (reason) {
      setLoadingError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  useEffect(() => {
    void load();
    return () => {
      void flushPendingSave();
    };
  }, [flushPendingSave]);

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

  useEffect(() => {
    const flushWhenHidden = () => {
      if (document.visibilityState === "hidden") void flushPendingSave();
    };
    window.addEventListener("pagehide", flushWhenHidden);
    document.addEventListener("visibilitychange", flushWhenHidden);
    return () => {
      window.removeEventListener("pagehide", flushWhenHidden);
      document.removeEventListener("visibilitychange", flushWhenHidden);
    };
  }, [flushPendingSave]);

  const reloadHistory = useCallback(() => {
    setHistoryError(null);
    setHistoryLoading(true);
    void invoke<HistoryPage>("get_history", { limit: 50 })
      .then((page) => {
        setHistory(page.items);
        setHistoryHasMore(page.has_more);
      })
      .catch((reason) => {
        setHistoryError(t("历史记录加载失败：") + String(reason));
      })
      .finally(() => setHistoryLoading(false));
  }, [t]);

  const loadMoreHistory = useCallback(() => {
    if (historyLoading || !historyHasMore) return;
    const beforeId = history[history.length - 1]?.id;
    if (beforeId === undefined) return;
    setHistoryError(null);
    setHistoryLoading(true);
    void invoke<HistoryPage>("get_history", { before_id: beforeId, limit: 50 })
      .then((page) => {
        setHistory((current) => [...current, ...page.items]);
        setHistoryHasMore(page.has_more);
      })
      .catch((reason) => {
        setHistoryError(t("加载更早记录失败：") + String(reason));
      })
      .finally(() => setHistoryLoading(false));
  }, [history, historyHasMore, historyLoading, t]);

  useEffect(() => {
    if (!settings || showOnboarding || view !== "history") return;
    reloadHistory();
  }, [reloadHistory, settings, showOnboarding, view]);

  useEffect(() => {
    if (showOnboarding) return;
    let active = true;
    const subscription = listen<{ state?: string }>("dictation://state", (event) => {
      if (!active || view !== "history") return;
      if (!event.payload.state || !["done", "copied", "degraded", "error"].includes(event.payload.state)) return;
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
  if (!settings) return <main className={`min-h-screen ${colors.bg.base} p-12 ${colors.text.secondary}`}>{t("正在加载 VoiceFlow…")}</main>;
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

  const save = (patch: Partial<Settings>, options?: { persist?: boolean }) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    if (options?.persist === false) return;
    setSaveError(null);
    saveRevision.current += 1;
    const revision = saveRevision.current;
    pendingPatch.current = { ...pendingPatch.current, ...patch };
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      saveTimer.current = null;
      const patchToPersist = pendingPatch.current;
      pendingPatch.current = {};
      persistPatch(patchToPersist, revision);
    }, 300);
  };

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
  return (
    <main className={`h-screen overflow-hidden ${colors.bg.base} ${colors.text.primary}`}>
      <div className="mx-auto flex h-full w-full max-w-6xl">
        <aside className={`flex w-56 shrink-0 flex-col border-r ${colors.border} px-3 py-6`}>
          <div className="flex items-center gap-2.5 px-2">
            <img src={brandIconSrc} alt="VoiceFlow" className="h-8 w-8 shrink-0" />
            <p className="truncate text-sm font-semibold tracking-tight text-primary">VoiceFlow</p>
          </div>

          {!settings.onboarded && (
            <button type="button" onClick={() => setShowOnboarding(true)} className={`mt-6 w-full ${buttonClass} text-xs`}>
              {t("继续完成设置")}
            </button>
          )}

          <nav aria-label={t("设置页面")} className="mt-8 space-y-6">
            {navigationGroups.map((group) => (
              <div key={group.label}>
                <p className="mb-2 px-3 text-[10px] font-semibold uppercase tracking-[0.12em] text-tertiary">{t(group.label)}</p>
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
                        <Icon size={16} strokeWidth={1.7} className={active ? "text-primary" : "text-tertiary"} />
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

          <div className="mt-auto border-t border-border px-1 pt-3">
            <div className="space-y-0.5">
              <label htmlFor="settings-theme" className="group relative flex min-h-8 cursor-pointer items-center gap-2 rounded-md px-2 text-xs text-secondary transition-colors hover:bg-card hover:text-primary focus-within:bg-card">
                {settings.theme === "light" ? <Sun size={13} aria-hidden="true" /> : settings.theme === "dark" ? <Moon size={13} aria-hidden="true" /> : <Monitor size={13} aria-hidden="true" />}
                {t("主题")}
                <span className="ml-auto flex items-center gap-1 text-xs text-secondary">
                  {settings.theme === "system" ? t("跟随系统") : settings.theme === "light" ? t("浅色") : t("深色")}
                  <ChevronDown size={13} strokeWidth={1.8} aria-hidden="true" />
                </span>
                <select id="settings-theme" aria-label={t("主题")} value={settings.theme} onChange={(event) => save({ theme: event.target.value as Settings["theme"] })} className="absolute inset-0 z-10 h-full w-full cursor-pointer opacity-0">
                  <option value="system">{t("跟随系统")}</option>
                  <option value="light">{t("浅色")}</option>
                  <option value="dark">{t("深色")}</option>
                </select>
              </label>
            </div>
            <div className="space-y-0.5">
              <label htmlFor="settings-ui-language" className="group relative flex min-h-8 cursor-pointer items-center gap-2 rounded-md px-2 text-xs text-secondary transition-colors hover:bg-card hover:text-primary focus-within:bg-card">
                <Languages size={13} aria-hidden="true" />
                {t("语言")}
                <span className="ml-auto flex items-center gap-1 text-xs text-secondary">
                  {settings.ui_language === "system" ? t("跟随系统") : settings.ui_language === "zh" ? t("中文") : t("English")}
                  <ChevronDown size={13} strokeWidth={1.8} aria-hidden="true" />
                </span>
                <select id="settings-ui-language" aria-label={t("语言")} value={settings.ui_language} onChange={(event) => changeInterfaceLanguage(event.target.value as UiLanguagePreference)} className="absolute inset-0 z-10 h-full w-full cursor-pointer opacity-0">
                  <option value="system">{t("跟随系统")}</option>
                  <option value="zh">{t("中文")}</option>
                  <option value="en">{t("English")}</option>
                </select>
              </label>
            </div>
          </div>
        </aside>

        <section className="settings-scrollbar min-h-0 min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto min-h-screen w-full max-w-3xl px-6 py-9 sm:px-10 sm:py-12">
            {saveError && (
              <SettingsAlert onRetry={retryPendingSave} retryLabel={t("重试")}>{t("设置保存失败：")}{saveError}</SettingsAlert>
            )}
            {runtimeError && <SettingsAlert>{t("语音输入失败：")}{runtimeError}</SettingsAlert>}
            <AnimatedContent key={view} className="w-full">
              {view === "history" ? <History items={history} reload={reloadHistory} hasMore={historyHasMore} loading={historyLoading} onLoadMore={loadMoreHistory} error={historyError} onRetry={reloadHistory} /> : view === "smart" ? <ContextSettings combined writingModes={settings.writing_modes} onWritingModesChange={(writingModes) => save({ writing_modes: writingModes })} outputMode={settings.output_mode} translationTargetLanguage={settings.translation_target_language} onOutputModeChange={(outputMode) => save({ output_mode: outputMode })} onTranslationTargetLanguageChange={(language) => save({ translation_target_language: language })} /> : view === "snippets" ? <SnippetsSettings snippets={settings.snippets} onChange={(snippets) => save({ snippets })} /> : <SettingsView view={view} settings={settings} permissions={permissions} audioInputDevice={audioInputDevice} audioInputDevices={audioInputDevices} refreshPermissions={refreshPermissions} save={save} saveApiKey={saveApiKey} removeApiKey={removeApiKey} />}
            </AnimatedContent>
          </div>
        </section>
      </div>
    </main>
  );
}

function DictionarySettings({ settings, save }: { settings: Settings; save: (patch: Partial<Settings>, options?: { persist?: boolean }) => void }) {
  const { t } = useI18n();
  const [newWord, setNewWord] = useState("");
  const [dictionaryMessage, setDictionaryMessage] = useState<string | null>(null);
  const [dictionaryDragging, setDictionaryDragging] = useState(false);
  const [dictionaryImporting, setDictionaryImporting] = useState(false);
  const dictionaryFileInput = useRef<HTMLInputElement | null>(null);
  const dictionaryDropZone = useRef<HTMLDivElement | null>(null);
  const importingRef = useRef(false);
  const settingsRef = useRef(settings);
  const saveRef = useRef(save);
  settingsRef.current = settings;
  saveRef.current = save;

  const importText = useCallback(async (text: string, fileName: string) => {
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (importingRef.current) return;

    importingRef.current = true;
    setDictionaryImporting(true);
    setDictionaryMessage(null);
    try {
      const result = mergeDictionary(settingsRef.current.dictionary, parseDictionaryText(text, fileName));
      if (result.added > 0) saveRef.current({ dictionary: result.words });
      const details = [
        `${t("已导入")} ${result.added} ${t("条")}`,
        result.duplicates > 0 ? `${t("重复")} ${result.duplicates} ${t("条")}` : null,
        result.invalid > 0 ? `${t("无效")} ${result.invalid} ${t("条")}` : null,
        result.limited > 0 ? `${t("超出上限")} ${result.limited} ${t("条")}` : null,
      ].filter(Boolean).join(" · ");
      setDictionaryMessage(details || t("没有找到可导入的新词条。"));
    } catch (reason) {
      setDictionaryMessage(t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    } finally {
      importingRef.current = false;
      setDictionaryImporting(false);
    }
  }, [t]);

  const importFile = useCallback(async (file: File) => {
    const extension = file.name.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    if (file.size > MAX_DICTIONARY_FILE_BYTES) {
      setDictionaryMessage(t("词典文件不能超过 1 MB。"));
      return;
    }
    try {
      const decoder = new TextDecoder("utf-8", { fatal: true });
      await importText(decoder.decode(await file.arrayBuffer()), file.name);
    } catch (reason) {
      setDictionaryMessage(reason instanceof TypeError ? t("导入失败：词典文件必须使用 UTF-8 编码。") : t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    }
  }, [importText]);

  const importNativeFile = useCallback(async (path: string) => {
    const fileName = path.split(/[\\/]/).pop() || "dictionary.csv";
    const extension = fileName.toLowerCase().split(".").pop();
    if (!extension || !["csv", "txt", "tsv"].includes(extension)) {
      setDictionaryMessage(t("请选择 CSV、TXT 或 TSV 文件。"));
      return;
    }
    try {
      const text = await invoke<string>("read_dictionary_file", { path });
      await importText(text, fileName);
    } catch (reason) {
      setDictionaryMessage(t("导入失败：") + (reason instanceof Error ? reason.message : String(reason)));
    }
  }, [importText]);

  const isInsideDropZone = useCallback((position: { x: number; y: number }) => {
    const element = dictionaryDropZone.current;
    if (!element) return false;
    const rect = element.getBoundingClientRect();
    const scale = window.devicePixelRatio || 1;
    const x = position.x / scale;
    const y = position.y / scale;
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void getCurrentWebview().onDragDropEvent((event) => {
      if (disposed) return;
      const { payload } = event;
      if (payload.type === "leave") {
        setDictionaryDragging(false);
        return;
      }

      const overDropZone = isInsideDropZone(payload.position);
      if (payload.type === "enter" || payload.type === "over") {
        setDictionaryDragging(overDropZone);
        return;
      }

      setDictionaryDragging(false);
      if (overDropZone && payload.paths[0]) void importNativeFile(payload.paths[0]);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [importNativeFile, isInsideDropZone]);

  const addWord = () => {
    const word = newWord.trim();
    if (!word) return;
    const result = mergeDictionary(settings.dictionary, [word]);
    if (result.added === 0) {
      setDictionaryMessage(result.duplicates > 0 ? t("这个词条已经存在。") : t("词条无效或已达到上限。"));
      return;
    }
    save({ dictionary: result.words });
    setNewWord("");
    setDictionaryMessage(null);
  };

  const openDictionaryFilePicker = () => dictionaryFileInput.current?.click();
  const handleDictionaryDrop = (event: React.DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDictionaryDragging(false);
    const file = event.dataTransfer.files[0];
    if (file) void importFile(file);
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("个人词典")} description={t("把人名、产品名和专业术语添加到这里，识别时会优先保留正确拼写。")} />
      <SettingsGroup title={t("添加词条")}>
        <SettingsRow title={t("添加个人词典词条")} description={t("输入一个词条，或从 CSV、TXT、TSV 文件导入。")}>
          <div className="flex w-full min-w-0 gap-2 sm:w-auto">
            <input aria-label={t("添加个人词典词条")} value={newWord} onChange={(event) => setNewWord(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") addWord(); }} placeholder={t("添加人名或术语")} className={"min-w-0 flex-1 sm:w-48 " + controlClass} />
            <button type="button" onClick={addWord} disabled={!newWord.trim()} className={buttonClass}>{t("添加")}</button>
          </div>
        </SettingsRow>
        <div
          ref={dictionaryDropZone}
          role="button"
          tabIndex={0}
          aria-label={t("导入个人词典文件")}
          onClick={openDictionaryFilePicker}
          onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); openDictionaryFilePicker(); } }}
          onDragEnter={(event) => { event.preventDefault(); setDictionaryDragging(true); }}
          onDragOver={(event) => event.preventDefault()}
          onDragLeave={(event) => { if (event.currentTarget === event.target) setDictionaryDragging(false); }}
          onDrop={handleDictionaryDrop}
          className={`mx-4 my-3 flex min-h-16 cursor-pointer items-center gap-3 rounded-lg border border-dashed px-3 py-2.5 transition-colors sm:mx-5 ${dictionaryDragging ? "border-accent bg-elevated" : "border-border bg-elevated/40 hover:border-accent/50 hover:bg-elevated/70"} ${dictionaryImporting ? "pointer-events-none opacity-60" : ""}`}
        >
          <Upload size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-primary">{t("拖入词典文件，或点击选择")}</span>
            <span className="mt-0.5 block text-xs text-tertiary">{t("支持 CSV、TXT、TSV；CSV 默认读取第一列，每行一个词条。")} </span>
          </span>
          <FileText size={16} className="shrink-0 text-tertiary" aria-hidden="true" />
          <input ref={dictionaryFileInput} type="file" accept=".csv,.txt,.tsv,text/csv,text/plain,text/tab-separated-values" className="sr-only" onChange={(event) => { const file = event.target.files?.[0]; event.target.value = ""; if (file) void importFile(file); }} />
        </div>
        {dictionaryMessage && <p role="status" className="px-4 pb-4 text-xs text-secondary sm:px-5">{dictionaryMessage}</p>}
      </SettingsGroup>
      <SettingsGroup title={t("词条列表")} description={`${settings.dictionary.length} ${t("条")}`}>
        {settings.dictionary.length > 0 ? settings.dictionary.map((word) => (
          <div key={word} className="flex items-center gap-3 px-4 py-3.5 text-sm sm:px-5">
            <span className="flex-1 text-primary">{word}</span>
            <button type="button" aria-label={`${t("删除")} ${word}`} onClick={() => save({ dictionary: settings.dictionary.filter((item) => item !== word) })} className="rounded-lg px-2 py-1 text-xs text-tertiary transition-colors hover:bg-error/10 hover:text-error">{t("删除")}</button>
          </div>
        )) : <p className="px-4 py-5 text-sm text-tertiary sm:px-5">{t("还没有词条。添加后，VoiceFlow 会更准确地识别人名和专业术语。")}</p>}
      </SettingsGroup>
    </SettingsShell>
  );
}

function SettingsView({ view, settings, permissions, audioInputDevice, audioInputDevices, refreshPermissions, save, saveApiKey, removeApiKey }: { view: View; settings: Settings; permissions: Permissions | null; audioInputDevice: string | null; audioInputDevices: AudioInputDevice[]; refreshPermissions: () => Promise<void>; save: (patch: Partial<Settings>, options?: { persist?: boolean }) => void; saveApiKey: (apiKey: string) => Promise<void>; removeApiKey: () => Promise<void> }) {
  const { t } = useI18n();
  const [apiKeyDraft, setApiKeyDraft] = useState("");
  const [apiKeySaveError, setApiKeySaveError] = useState<string | null>(null);
  const [validating, setValidating] = useState(false);
  const [valid, setValid] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const selectedInputDevice = settings.input_device?.trim() ?? "";
  const selectedInputDeviceAvailable = !selectedInputDevice || audioInputDevices.some((device) => device.name === selectedInputDevice);

  const validate = async () => {
    setValidating(true);
    try {
      setValid(apiKeyDraft.trim() ? await invoke<string>("validate_api_key", { key: apiKeyDraft.trim() }) : await invoke<string>("validate_configured_api_key"));
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
    if (!window.confirm(t("删除后需要重新配置 API Key 才能使用语音输入。确定删除吗？"))) return;
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

  if (view === "dictionary") {
    return <DictionarySettings settings={settings} save={save} />;
  }

  if (view === "permissions") {
    return <PermissionsSettings permissions={permissions} onRefresh={refreshPermissions} />;
  }

  if (view === "system") {
    return (
      <SettingsShell>
        <SettingsPageHeader title={t("系统设置")} description={t("菜单栏图标、音频输入和系统级应用行为。")} />
        <SettingsGroup title={t("音频输入")} description={t("选择录音使用的麦克风；默认跟随 macOS 系统设置。")}>
          <SettingsRow title={t("输入设备")} description={t("默认会跟随 macOS 当前输入设备；选择具体设备后，录音会固定使用它。")}>
            <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
              <select aria-label={t("输入设备")} value={selectedInputDevice} onChange={(event) => save({ input_device: event.target.value })} className={`${controlClass} w-56 max-w-full`}>
                <option value="">{t("默认（跟随系统）")}{audioInputDevice ? ` · ${audioInputDevice}` : ""}</option>
                {selectedInputDevice && !selectedInputDeviceAvailable && <option value={selectedInputDevice}>{`${selectedInputDevice} · ${t("设备不可用")}`}</option>}
                {audioInputDevices.map((device) => <option key={device.name} value={device.name}>{device.name}</option>)}
              </select>
              {selectedInputDevice && !selectedInputDeviceAvailable && <SettingsStatus label={t("设备不可用")} tone="warning" />}
            </div>
          </SettingsRow>
        </SettingsGroup>
        <SettingsGroup title={t("应用行为")}>
          <SettingsRow title={t("菜单栏图标")} description={t("关闭后隐藏 VoiceFlow 的菜单栏图标；你仍可以从应用窗口重新打开设置。")}>
            <Toggle checked={settings.show_tray_icon} onChange={(checked) => save({ show_tray_icon: checked })} label={t("显示菜单栏图标")} />
          </SettingsRow>
        </SettingsGroup>
      </SettingsShell>
    );
  }

  if (view === "engine") {
    return (
      <SettingsShell>
        <SettingsPageHeader title={t("语音服务")} description={`${t("连接 Groq：")}${modelLabel(settings.asr_model)}${t("负责语音转文字，")}${modelLabel(settings.cleanup_model)}${t("负责整理文字。密钥只保存在这台 Mac 上。")}`} />
        <SettingsGroup title={t("服务凭据")}>
          <div className="px-4 py-4 sm:px-5">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <p className="text-sm font-medium text-primary">Groq API Key</p>
                <p className="mt-1 text-xs leading-5 text-tertiary">{settings.api_key_configured ? t("当前已配置：") + (settings.api_key_hint ?? t("已隐藏")) : t("在 Groq Console 创建，通常以 gsk_ 开头。")}</p>
              </div>
              <SettingsStatus label={settings.api_key_configured ? t("已配置") : t("未配置")} tone={settings.api_key_configured ? "success" : "warning"} />
            </div>
            <div className="mt-4 max-w-xl">
              <PasswordInput
                id="settings-groq-api-key"
                ariaLabel={t("Groq API Key（访问密钥）")}
                value={apiKeyDraft}
                onChange={(value) => { setApiKeyDraft(value); setValid(null); setApiKeySaveError(null); }}
                placeholder={settings.api_key_configured ? t("留空保持当前密钥") : "gsk_…"}
                valid={valid === "valid"}
                monospace
              />
              <div className="mt-3 flex flex-wrap items-center gap-2">
                <button type="button" onClick={() => void commitApiKey()} disabled={!apiKeyDraft.trim()} className={buttonClass}>{t("验证并保存")}</button>
                <button type="button" onClick={() => void validate()} disabled={(!apiKeyDraft.trim() && !settings.api_key_configured) || validating} className={secondaryButtonClass}>{validating ? t("验证中…") : t("仅验证")}</button>
                {settings.api_key_configured && <button type="button" onClick={() => void commitRemoveApiKey()} disabled={removing} className="rounded-lg px-3 py-2 text-xs text-error transition-colors hover:bg-error/10 disabled:opacity-50">{removing ? t("删除中…") : t("删除本机密钥")}</button>}
                <ValidationStatus status={valid} validating={validating} />
              </div>
              {apiKeySaveError && <p role="alert" className="mt-3 text-xs leading-5 text-error">{apiKeySaveError}</p>}
              <p className="mt-3 flex items-center gap-1.5 text-xs leading-5 text-tertiary"><KeyRound size={14} aria-hidden="true" />{t("密钥仅保存在这台 Mac 的钥匙串中，验证时只发送到 Groq。")}</p>
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
      </SettingsShell>
    );
  }

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("录音与输出")} description={t("快捷键、识别语言、长录音和本地存储。")} />
      <SettingsGroup title={t("快捷键")}>
        <div className="px-4 py-4 sm:px-5">
          <p className="text-sm font-medium text-primary">{t("全局快捷键")}</p>
          <p className="mt-1 text-xs text-tertiary">{t("在 Cursor、浏览器、邮件等 App 中都能使用。")}</p>
          {settings.hotkey_error && <p role="alert" className="mt-3 rounded-lg bg-error/5 px-3 py-2 text-xs text-error">{t("快捷键注册失败：")}{settings.hotkey_error}。{t("请重新设置一个快捷键。")}</p>}
          <div className="mt-4"><HotkeyRecorder value={settings.hotkey} onChange={(hotkey, mode, options) => save(mode ? { hotkey, activation_mode: mode } : { hotkey }, options)} /></div>
          <HotkeyUsageGuide hotkey={settings.hotkey} activationMode={settings.activation_mode} />
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("选中文本操作")} description={t("先选中文本，再用独立快捷键说出改写、缩短、翻译或总结指令。") }>
        <SettingsRow
          title={t("启用选中文本操作")}
          description={
            <>
              {t("默认开启；使用独立快捷键，不会自动保存原选中文本。")}
              {settings.selected_actions_enabled && !settings.selected_action_hotkey?.trim() && (
                <span role="status" className="mt-1 block text-warning">{t("请先设置快捷键后才能触发")}</span>
              )}
            </>
          }
        >
          <Toggle checked={Boolean(settings.selected_actions_enabled)} onChange={(checked) => save({ selected_actions_enabled: checked })} label={t("启用选中文本操作")} />
        </SettingsRow>
        <div className="px-4 py-4 sm:px-5">
          <p className="text-sm font-medium text-primary">{t("选中文本快捷键")}</p>
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("选中文本后按它开始录音，再按一次结束；目标或选区变化时只复制结果，不会替换文字。")}</p>
          <div className="mt-4">
            <HotkeyRecorder
              value={settings.selected_action_hotkey ?? ""}
              captureTarget="selected_action"
              onChange={(hotkey, _mode, options) => save({ selected_action_hotkey: hotkey, selected_actions_enabled: true }, options)}
            />
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title={t("识别")}>
        <SettingsRow title={t("识别语言")} description={t("自动检测适合中文、English 和混合语音。只有在识别结果不稳定时，才建议手动指定。")}> 
          <select aria-label={t("识别语言")} value={settings.language} onChange={(event) => save({ language: event.target.value })} className={selectClass}><option value="auto">{t("自动检测")}</option><option value="zh">{t("中文")}</option><option value="en">English</option></select>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("长录音")} description={t("较长的录音会自动分段处理，避免一次请求过大。")}>
        <SettingsRow title={t("开始分段（秒）")} description={t("超过这个时长后开始分段。默认 25 秒。")}>
          <input aria-label={t("开始分段（秒）")} type="number" min={5} max={3600} step={1} value={settings.chunk_threshold_secs} onChange={(event) => save({ chunk_threshold_secs: Number(event.target.value) })} className={controlClass + " w-28 text-right"} />
        </SettingsRow>
        <SettingsRow title={t("每段长度（秒）")} description={t("每个语音请求的目标长度。默认 35 秒。")}>
          <input aria-label={t("每段长度（秒）")} type="number" min={15} max={60} step={1} value={settings.chunk_length_secs} onChange={(event) => save({ chunk_length_secs: Number(event.target.value) })} className={controlClass + " w-28 text-right"} />
        </SettingsRow>
        <SettingsRow title={t("长录音输出")} description={t("长录音完成后如何交付结果。")}>
          <select aria-label={t("长录音输出")} value={settings.long_output_mode} onChange={(event) => save({ long_output_mode: event.target.value })} className={selectClass}><option value="clipboard">{t("复制到剪贴板")}</option><option value="paste">{t("自动粘贴")}</option><option value="history">{t("仅保存到历史")}</option></select>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("本地存储")}>
        <div>
          <SettingsRow title={t("保留音频")} description={t("设置本机恢复音频的保留时间，过期后会自动清理。")}>
            <select
              aria-label={t("音频缓存保留时间")}
              value={String(settings.keep_audio_days)}
              onChange={(event) => save({ keep_audio_days: Number(event.target.value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {retentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </select>
          </SettingsRow>
          {settings.keep_audio_days === 365 && <p className="px-4 pb-4 text-xs text-warning sm:px-5">{t("较长时间保留可能占用更多磁盘空间。")} </p>}
        </div>
        <div>
          <SettingsRow title={t("保留历史文字")} description={t("自动清理本机历史记录中的原始文字、整理结果和上下文策略。")}>
            <select
              aria-label={t("历史文字保留时间")}
              value={String(settings.keep_history_days)}
              onChange={(event) => save({ keep_history_days: Number(event.target.value) })}
              className={`${controlClass} w-40 max-w-full`}
            >
              {historyRetentionOptions.map((option) => <option key={option.value} value={option.value}>{t(option.label)}</option>)}
            </select>
          </SettingsRow>
          {(settings.keep_history_days === 0 || settings.keep_history_days >= 3650) && <p className="px-4 pb-4 text-xs text-warning sm:px-5">{settings.keep_history_days === 0 ? t("历史记录会永久保留，除非你手动删除或清空。") : t("历史记录会长期保留，请定期清理。")} </p>}
        </div>
      </SettingsGroup>
    </SettingsShell>
  );
}

function modelLabel(model: string): string {
  if (model === "whisper-large-v3-turbo") return "Whisper Large v3 Turbo";
  if (model === "openai/gpt-oss-20b") return "GPT-OSS 20B";
  if (model === "openai/gpt-oss-120b") return "GPT-OSS 120B";
  return model;
}
