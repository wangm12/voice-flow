import { useSessionDraftState } from "../lib/sessionDraft";
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useI18n } from "../lib/i18n";
import { ConfirmDialog } from "./ConfirmDialog";
import { SettingsAlert, SettingsPageHeader, SettingsShell, type SettingsSaveFailure } from "./SettingsLayout";
import { type ProviderId } from "../lib/providers";
import { type WritingPreferenceCandidate, type WritingPreferenceDecisionItem, type WritingPreferenceDecisionOutcome } from "./WritingPreferenceReview";
import { settingsErrorMessage } from "../lib/settingsError";
import { SmartCleanupSettings } from "./context/SmartCleanupSettings";
import { WritingToneSettings } from "./context/WritingToneSettings";
import { AppMappingEditor } from "./context/AppMappingEditor";
import {
  applicationsFromMappings,
  contextPrivacyPreset,
  defaultWritingModes,
  emptyWritingPreferenceDraft,
  familyModeId,
  hasCloudSupportedActivePreference,
  knownNativeTaskBundleIds,
  mappingIntensityFrom,
  mappingSelectorsFromEditor,
  mergeApplicationOptions,
  newMappingId,
  normalizeBrowserHost,
  preferenceDraftFrom,
  preferencesFromDraft,
  samePreferenceDraftItem,
  sameWritingMode,
  selectedModeFamilyForEditor,
  selectorsEqual,
  sourcePermissionValue,
  writingPreferenceCandidateKey,
  writingPreferenceMappingScope,
  type ApplicationOption,
  type ContextFamily,
  type ContextMapping,
  type ContextPrivacyPreset,
  type ContextSnapshot,
  type ContextSourcePermissions,
  type ContextWritingPreference,
  type FocusKind,
  type PreferenceReviewConflict,
  type PreferenceReviewEditorState,
  type PreferenceReviewStartSnapshot,
  type WritingMode,
  type WritingPreferenceDimension,
  type WritingPreferenceDraft,
  type WritingPreferenceStatus,
  type WritingPreferenceValue,
  type WritingTaskId,
} from "./context/contextShared";

export type {
  ContextFamily,
  FocusKind,
  ContextSourcePermissions,
  WritingTaskId,
  WritingPreferenceDimension,
  WritingPreferenceValue,
  WritingPreferenceStatus,
  ContextWritingPreference,
  ContextMapping,
  WritingMode,
};

export function ContextSettings({
  writingModes: managedWritingModes = defaultWritingModes,
  onWritingModesChange,
  automationOnly = false,
  combined = false,
  saveFailure,
  onOpenPermissions,
  onOpenMappings,
  onOpenDictionary,
  outputMode,
  translationTargetLanguage,
  onOutputModeChange,
  onTranslationTargetLanguageChange,
  cleanupIntensity = "auto",
  onCleanupIntensityChange,
  windowOcrEnabled = false,
  onWindowOcrEnabledChange,
  visionProvider = "",
  visionModel = "",
  onVisionProviderChange,
  onVisionModelChange,
  accurateAsrProvider = "groq",
  accurateAsrModel = "",
  accurateAsrBaseUrl = "",
  onAccurateAsrProviderChange,
  onAccurateAsrModelChange,
  onAccurateAsrBaseUrlChange,
}: {
  writingModes?: WritingMode[];
  onWritingModesChange?: (writingModes: WritingMode[]) => void;
  automationOnly?: boolean;
  combined?: boolean;
  saveFailure?: SettingsSaveFailure | null;
  onOpenPermissions?: () => void;
  onOpenMappings?: () => void;
  onOpenDictionary?: () => void;
  outputMode?: string;
  translationTargetLanguage?: string;
  onOutputModeChange?: (outputMode: string) => void;
  onTranslationTargetLanguageChange?: (language: string) => void;
  cleanupIntensity?: "auto" | "off" | "light" | "standard" | "heavy";
  onCleanupIntensityChange?: (intensity: "auto" | "off" | "light" | "standard" | "heavy") => void;
  windowOcrEnabled?: boolean;
  onWindowOcrEnabledChange?: (enabled: boolean) => void;
  visionProvider?: string;
  visionModel?: string;
  onVisionProviderChange?: (provider: string) => void;
  onVisionModelChange?: (model: string) => void;
  accurateAsrProvider?: ProviderId;
  accurateAsrModel?: string;
  accurateAsrBaseUrl?: string;
  onAccurateAsrProviderChange?: (provider: ProviderId) => void;
  onAccurateAsrModelChange?: (model: string) => void;
  onAccurateAsrBaseUrlChange?: (url: string) => void;
}) {
  const { t } = useI18n();
  const draftKey = combined ? "writing" : automationOnly ? "smart" : "writing";
  const showAutomationSettings = automationOnly || combined;
  const showWritingSettings = !automationOnly || combined;
  const [snapshot, setSnapshot] = useState<ContextSnapshot | null>(null);
  const [mappings, setMappings] = useState<ContextMapping[]>([]);
  const [applications, setApplications] = useState<ApplicationOption[]>([]);
  const [selectedApplicationId, setSelectedApplicationId] = useSessionDraftState(`${draftKey}:selectedApplicationId`, "");
  const [selectedExecutable, setSelectedExecutable] = useSessionDraftState(`${draftKey}:selectedExecutable`, "");
  const [selectedWebsite, setSelectedWebsite] = useSessionDraftState(`${draftKey}:selectedWebsite`, "");
  const [browserPathPrefix, setBrowserPathPrefix] = useSessionDraftState(`${draftKey}:browserPathPrefix`, "");
  const [focusedField, setFocusedField] = useSessionDraftState<FocusKind | "">(`${draftKey}:focusedField`, "");
  const [editingMappingId, setEditingMappingId] = useSessionDraftState<string | null>(`${draftKey}:editingMappingId`, null);
  const [preferenceRevisionBaseline, setPreferenceRevisionBaseline] = useSessionDraftState<{ mappingId: string | null; revision: number | null }>(`${draftKey}:preferenceRevisionBaseline`, { mappingId: null, revision: null });
  const [sourcePermissions, setSourcePermissions] = useSessionDraftState<ContextSourcePermissions>(`${draftKey}:sourcePermissions`, {
    ax_text: false,
    local_ocr: false,
    cloud_vision: false,
    context_text_to_providers: false,
  });
  const [selectedModeId, setSelectedModeId] = useSessionDraftState(`${draftKey}:selectedModeId`, "general");
  const [selectedWritingTaskId, setSelectedWritingTaskId] = useSessionDraftState<WritingTaskId | "">(`${draftKey}:selectedWritingTaskId`, "");
  const [writingPreferenceDraft, setWritingPreferenceDraft] = useSessionDraftState<WritingPreferenceDraft>(`${draftKey}:writingPreferenceDraft`, emptyWritingPreferenceDraft);
  const [preferenceGuidanceProviderApproved, setPreferenceGuidanceProviderApproved] = useSessionDraftState(`${draftKey}:preferenceGuidanceProviderApproved`, false);
  const [styleExampleInput, setStyleExampleInput] = useSessionDraftState(`${draftKey}:styleExampleInput`, "");
  const [styleExampleOutput, setStyleExampleOutput] = useSessionDraftState(`${draftKey}:styleExampleOutput`, "");
  const [removeStyleExamples, setRemoveStyleExamples] = useSessionDraftState(`${draftKey}:removeStyleExamples`, false);
  const [styleExamplesApproved, setStyleExamplesApproved] = useSessionDraftState(`${draftKey}:styleExamplesApproved`, false);
  const [mappingIntensity, setMappingIntensity] = useSessionDraftState<"inherit" | "auto" | "off" | "light" | "standard" | "heavy">(`${draftKey}:mappingIntensity`, "inherit");
  const [mappingCleanupEnabled, setMappingCleanupEnabled] = useSessionDraftState(`${draftKey}:mappingCleanupEnabled`, true);
  const [mappingLearnEnabled, setMappingLearnEnabled] = useSessionDraftState(`${draftKey}:mappingLearnEnabled`, true);
  const [writingModes, setWritingModes] = useSessionDraftState<WritingMode[]>(`${draftKey}:writingModes`, managedWritingModes);
  const [draftModeId, setDraftModeId] = useSessionDraftState<string | null>(`${draftKey}:draftModeId`, null);
  const [editingModeId, setEditingModeId] = useSessionDraftState(`${draftKey}:editingModeId`, "general");
  const [allScenesOpen, setAllScenesOpen] = useSessionDraftState(`${draftKey}:allScenesOpen`, false);
  const [modeLabelDraft, setModeLabelDraft] = useSessionDraftState(`${draftKey}:modeLabelDraft`, "");
  const [modePromptDraft, setModePromptDraft, restoredPrompt] = useSessionDraftState(`${draftKey}:modePromptDraft`, "");
  const [applicationsLoading, setApplicationsLoading] = useState(true);
  const [enabled, setEnabled] = useState(true);
  const [overrideFamily, setOverrideFamily] = useState<ContextFamily | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preferenceScopeMigrationNotice, setPreferenceScopeMigrationNotice] = useState(false);
  const [preferenceReviewRefreshIds, setPreferenceReviewRefreshIds] = useState<Set<string>>(() => new Set());
  const [preferenceReviewScopeConflictId, setPreferenceReviewScopeConflictId] = useState<string | null>(null);
  const [preferenceReviewConflicts, setPreferenceReviewConflicts] = useState<PreferenceReviewConflict[]>([]);
  const [initializationError, setInitializationError] = useState<string | null>(null);
  const [errorScope, setErrorScope] = useState<"prompt" | "mapping" | "override" | "context" | "browser" | "applications" | null>(null);
  const [promptErrorPlacement, setPromptErrorPlacement] = useState<"preview" | "prompt">("preview");
  const mappingDetailsRef = useRef<HTMLDetailsElement>(null);
  const editingModeSelectRef = useRef<HTMLButtonElement>(null);
  const modePromptRef = useRef<HTMLTextAreaElement>(null);
  const confirmationFocusRef = useRef<HTMLElement | null>(null);
  const [managedModesSnapshot, setManagedModesSnapshot] = useSessionDraftState(`${draftKey}:savedModes`, managedWritingModes);
  const previousManagedModesRef = useRef(managedModesSnapshot);
  const [busyAction, setBusyAction] = useState<"context" | "browser" | "override" | "applications" | "mapping" | "delete_mapping" | null>(null);
  const busy = busyAction !== null;
  const lifecycleRef = useRef(0);
  const mappingsRef = useRef(mappings);
  mappingsRef.current = mappings;
  const applicationsRef = useRef(applications);
  applicationsRef.current = applications;
  const preferenceReviewEditorRef = useRef<PreferenceReviewEditorState>({
    editingMappingId,
    preferenceRevisionBaseline,
    selectedModeFamily: selectedModeFamilyForEditor(writingModes, selectedModeId, draftModeId),
    selectedWritingTaskId,
    selectedApplicationId,
    selectedExecutable,
    selectedWebsite,
    browserPathPrefix,
    focusedField,
    writingPreferenceDraft,
    preferenceGuidanceProviderApproved,
  });
  preferenceReviewEditorRef.current = {
    editingMappingId,
    preferenceRevisionBaseline,
    selectedModeFamily: selectedModeFamilyForEditor(writingModes, selectedModeId, draftModeId),
    selectedWritingTaskId,
    selectedApplicationId,
    selectedExecutable,
    selectedWebsite,
    browserPathPrefix,
    focusedField,
    writingPreferenceDraft,
    preferenceGuidanceProviderApproved,
  };
  const preferenceReviewStartRef = useRef<Map<string, PreferenceReviewStartSnapshot>>(new Map());
  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    return () => {
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
    };
  }, []);
  useEffect(() => {
    if (error && errorScope === "mapping" && mappingDetailsRef.current) mappingDetailsRef.current.open = true;
  }, [error, errorScope]);
  const [pendingConfirm, setPendingConfirm] = useState<{
    title: string;
    description: string;
    confirmLabel: string;
    action: { type: "select_mode"; modeId: string } | { type: "add_mode" } | { type: "delete_mode"; modeId: string } | { type: "delete_mapping"; mappingId: string } | { type: "forget_preference"; dimension: WritingPreferenceDimension };
  } | null>(null);

  const savedWritingModes = draftModeId ? writingModes.filter((mode) => mode.id !== draftModeId) : writingModes;
  const selectedMappingMode = savedWritingModes.find((mode) => mode.id === selectedModeId) ?? savedWritingModes[0];
  const editingMode = writingModes.find((mode) => mode.id === editingModeId) ?? writingModes[0];
  const editingModeLabel = editingMode?.builtin ? editingMode.label : modeLabelDraft;
  const modeIsDirty = Boolean(editingMode && (editingModeLabel !== editingMode.label || modePromptDraft !== editingMode.prompt));
  const guideDestinations = { permissions: onOpenPermissions, mappings: onOpenMappings, dictionary: onOpenDictionary };
  const openGuideDestination = (destination: keyof typeof guideDestinations) => {
    const navigate = guideDestinations[destination];
    if (!navigate || busy) return;
    navigate();
  };

  useEffect(() => {
    // Only reconcile an incoming settings snapshot. Local saves must not be
    // replaced by the previous prop value while persistence is pending.
    if (previousManagedModesRef.current === managedWritingModes) return;
    previousManagedModesRef.current = managedWritingModes;
    setManagedModesSnapshot(managedWritingModes);
    setWritingModes((current) => {
      const keepEditing = modeIsDirty || Boolean(draftModeId);
      const next = managedWritingModes.map((incoming) => {
        const existing = current.find((mode) => mode.id === incoming.id);
        return existing && ((keepEditing && existing.id === editingModeId) || sameWritingMode(existing, incoming))
          ? existing : incoming;
      });
      const protectedMode = keepEditing ? current.find((mode) => mode.id === editingModeId) : undefined;
      if (protectedMode && !next.some((mode) => mode.id === protectedMode.id)) next.push(protectedMode);
      return next.length === current.length && next.every((mode, index) => mode === current[index]) ? current : next;
    });
  }, [managedWritingModes, draftModeId, editingModeId, modeIsDirty, setManagedModesSnapshot]);

  const restorePromptOnMount = useRef(restoredPrompt);
  useEffect(() => {
    if (restorePromptOnMount.current) { restorePromptOnMount.current = false; return; }
    if (!editingMode) return;
    setEditingModeId(editingMode.id);
    setModeLabelDraft(editingMode.label);
    setModePromptDraft(editingMode.prompt);
  }, [editingMode]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      invoke<ContextSnapshot>("get_context_snapshot"),
      invoke<ContextMapping[]>("get_context_mappings"),
      invoke<{ context_enabled: boolean }>("get_settings"),
      invoke<ContextFamily | null>("get_context_override"),
    ])
      .then(([nextSnapshot, nextMappings, settings, nextOverride]) => {
        if (cancelled) return;
        setSnapshot(nextSnapshot);
        setMappings(nextMappings);
        setApplications((current) => mergeApplicationOptions(current, applicationsFromMappings(nextMappings)));
        setEnabled(settings.context_enabled);
        setOverrideFamily(nextOverride);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setInitializationError(settingsErrorMessage(reason));
      });

    void invoke<ApplicationOption[]>("get_available_applications")
      .then((nextApplications) => {
        if (!cancelled) setApplications((current) => mergeApplicationOptions(nextApplications, current));
      })
      .catch((reason: unknown) => {
        if (!cancelled) setInitializationError(settingsErrorMessage(reason));
      })
      .finally(() => {
        if (!cancelled) setApplicationsLoading(false);
      });

    let cancelledListener = false;
    let unlisten: (() => void) | undefined;
    void listen<ContextSnapshot>("context://changed", (event) => setSnapshot(event.payload))
      .then((cleanup) => {
        if (cancelledListener) {
          cleanup();
        } else {
          unlisten = cleanup;
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) setInitializationError(settingsErrorMessage(reason));
      });
    return () => {
      cancelled = true;
      cancelledListener = true;
      unlisten?.();
    };
  }, []);

  const refreshApplications = async () => {
    setBusyAction("applications");
    setApplicationsLoading(true);
    setErrorScope("applications");
    setError(null);
    try {
      const nextApplications = await invoke<ApplicationOption[]>("get_available_applications");
      setApplications((current) => mergeApplicationOptions(nextApplications, current));
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setApplicationsLoading(false);
      setBusyAction(null);
    }
  };

  const addApplicationFromDisk = async () => {
    setBusyAction("applications");
    setErrorScope("applications");
    setError(null);
    try {
      const selectedPath = await open({
        title: t("选择应用程序"),
        defaultPath: "/Applications",
        multiple: false,
        directory: false,
        filters: [{ name: t("应用程序"), extensions: ["app"] }],
      });
      if (typeof selectedPath !== "string") return;

      const application = await invoke<ApplicationOption>("get_application_from_path", { path: selectedPath });
      setApplications((current) => mergeApplicationOptions([application], current));
      selectApplication(application.bundle_id);
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const publishWritingModes = (nextWritingModes: WritingMode[]) => {
    setWritingModes(nextWritingModes);
    onWritingModesChange?.(nextWritingModes);
  };

  const withoutEditingDraft = () => writingModes.flatMap((mode) => {
    if (mode.id === draftModeId) return [];
    if (mode.id !== editingModeId || !modeIsDirty) return [mode];
    const saved = managedWritingModes.find((item) => item.id === mode.id);
    return saved ? [saved] : [];
  });

  const applySelectEditingMode = (modeId: string) => {
    const nextWritingModes = draftModeId && draftModeId !== modeId
      ? withoutEditingDraft() : modeIsDirty ? withoutEditingDraft() : writingModes;
    setWritingModes(nextWritingModes);
    if (draftModeId !== modeId) setDraftModeId(null);
    const mode = nextWritingModes.find((item) => item.id === modeId);
    if (!mode) return;
    setEditingModeId(mode.id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
    setErrorScope(null);
  };

  const selectEditingMode = (modeId: string) => {
    if (modeId === editingModeId) return;
    if (modeIsDirty) {
      setAllScenesOpen(true);
      confirmationFocusRef.current = editingModeSelectRef.current;
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前语气有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: { type: "select_mode", modeId },
      });
      return;
    }
    applySelectEditingMode(modeId);
  };

  const applyAddCustomMode = () => {
    const id = `custom.${Date.now()}.${Math.random().toString(36).slice(2, 8)}`;
    const mode: WritingMode = {
      id,
      label: t("自定义语气"),
      family: "general",
      prompt: t("根据我说的内容整理文字，保留事实、语气和具体信息，不要添加我没有说过的内容。"),
      builtin: false,
    };
    const baseWritingModes = withoutEditingDraft();
    const next = [...baseWritingModes, mode];
    setWritingModes(next);
    setDraftModeId(id);
    setEditingModeId(id);
    setModeLabelDraft(mode.label);
    setModePromptDraft(mode.prompt);
    setError(null);
    setErrorScope(null);
  };

  const addCustomMode = () => {
    if (modeIsDirty) {
      confirmationFocusRef.current = editingModeSelectRef.current;
      setPendingConfirm({
        title: t("放弃未保存修改"),
        description: t("当前语气有未保存修改，确定放弃吗？"),
        confirmLabel: t("放弃修改"),
        action: { type: "add_mode" },
      });
      return;
    }
    applyAddCustomMode();
  };

  const saveEditingMode = (placement: "preview" | "prompt") => {
    setErrorScope("prompt");
    setPromptErrorPlacement(placement);
    if (!editingMode) return;
    const label = editingMode.builtin ? editingMode.label : modeLabelDraft.trim();
    const prompt = modePromptDraft.trim();
    if (!label) {
      setError(t("请填写语气名称"));
      return;
    }
    if (!prompt) {
      setError(t("请填写 Prompt"));
      return;
    }
    publishWritingModes(writingModes.map((mode) => mode.id === editingMode.id
      ? { ...mode, label, prompt }
      : mode));
    setDraftModeId(null);
    setModeLabelDraft(label);
    setModePromptDraft(prompt);
    setError(null);
    setErrorScope(null);
  };

  const cancelModeEditing = (origin: "preview" | "prompt") => {
    const mode = managedWritingModes.find((item) => item.id === editingModeId) ?? managedWritingModes[0];
    setWritingModes(managedWritingModes); setDraftModeId(null);
    if (mode) { setEditingModeId(mode.id); setModeLabelDraft(mode.label); setModePromptDraft(mode.prompt); }
    setError(null); setErrorScope(null);
    (origin === "prompt" ? modePromptRef.current : editingModeSelectRef.current)?.focus({ preventScroll: true });
  };

  const deleteEditingMode = () => {
    setErrorScope("prompt");
    setPromptErrorPlacement("preview");
    if (!editingMode || editingMode.builtin) return;
    if (mappings.some((mapping) => mapping.mode_id === editingMode.id)) {
      setError(t("请先删除使用这个语气的 App 映射"));
      return;
    }
    confirmationFocusRef.current = editingModeSelectRef.current;
    setPendingConfirm({
      title: t("删除自定义语气"),
      description: t("确定删除“{name}”吗？").replace("{name}", editingMode.label),
      confirmLabel: t("删除自定义语气"),
      action: { type: "delete_mode", modeId: editingMode.id },
    });
  };

  const applyDeleteMode = (modeId: string) => {
    const mode = writingModes.find((item) => item.id === modeId);
    if (!mode || mode.builtin) return;
    if (mappings.some((mapping) => mapping.mode_id === modeId)) {
      setErrorScope("prompt");
      setPromptErrorPlacement("preview");
      setError(t("请先删除使用这个语气的 App 映射"));
      return;
    }
    const next = writingModes.filter((item) => item.id !== modeId);
    if (modeId === draftModeId) {
      setWritingModes(next);
      setDraftModeId(null);
    } else {
      publishWritingModes(next);
    }
    const fallback = next[0];
    if (fallback) {
      setEditingModeId(fallback.id);
      setModeLabelDraft(fallback.label);
      setModePromptDraft(fallback.prompt);
    }
    if (selectedModeId === modeId) setSelectedModeId("general");
    setError(null);
    setErrorScope(null);
  };

  const loadMapping = (existing?: ContextMapping) => {
    setPreferenceScopeMigrationNotice(false);
    setEditingMappingId(existing?.id ?? null);
    setPreferenceRevisionBaseline({
      mappingId: existing?.id ?? null,
      revision: existing ? existing.preference_revision ?? 0 : null,
    });
    if (existing) {
      setPreferenceReviewConflicts((current) => current.filter((conflict) => conflict.mappingId !== existing.id));
      setPreferenceReviewScopeConflictId((current) => current === existing.id ? null : current);
    }
    setSelectedApplicationId(existing?.bundle_id ?? "");
    setSelectedExecutable(existing?.executable ?? "");
    setSelectedWebsite(existing?.browser_host ?? "");
    setBrowserPathPrefix(existing?.browser_path_prefix ?? "");
    setFocusedField(existing?.focused_field ?? "");
    setSelectedModeId(existing?.mode_id ?? familyModeId(existing?.family ?? "general"));
    setSelectedWritingTaskId(existing?.writing_task_id ?? "");
    setWritingPreferenceDraft(preferenceDraftFrom(existing, existing?.writing_task_id ?? ""));
    setPreferenceGuidanceProviderApproved(
      existing?.preference_guidance_provider_approved === true
      && hasCloudSupportedActivePreference(preferencesFromDraft(
        preferenceDraftFrom(existing, existing?.writing_task_id ?? ""),
        existing?.writing_task_id ?? "",
      )),
    );
    setRemoveStyleExamples(false);
    setStyleExampleInput(existing?.style_example_input ?? "");
    setStyleExampleOutput(existing?.style_example_output ?? "");
    setStyleExamplesApproved(existing?.style_examples_approved === true);
    setSourcePermissions({
      ax_text: sourcePermissionValue(existing?.source_permissions, "ax_text"),
      local_ocr: sourcePermissionValue(existing?.source_permissions, "local_ocr"),
      // Preserve an old, currently ineffective host-only grant so the user
      // can see and explicitly clear it instead of silently losing data.
      cloud_vision: sourcePermissionValue(existing?.source_permissions, "cloud_vision"),
      context_text_to_providers: sourcePermissionValue(existing?.source_permissions, "context_text_to_providers"),
    });
    setMappingIntensity(mappingIntensityFrom(existing));
    setMappingCleanupEnabled(existing?.cleanup_enabled !== false);
    setMappingLearnEnabled(existing?.dictionary_learn_enabled !== false);
    setError(null);
    setErrorScope(null);
  };

  const selectApplication = (bundleId: string) => {
    if (editingMappingId) {
      setSelectedApplicationId(bundleId);
      return;
    }
    const hasOtherSelector = Boolean(
      selectedExecutable.trim()
      || selectedWebsite.trim()
      || browserPathPrefix.trim()
      || focusedField
      || styleExampleInput.trim()
      || styleExampleOutput.trim()
      || styleExamplesApproved
      || selectedWritingTaskId
      || preferencesFromDraft(writingPreferenceDraft, selectedWritingTaskId).length > 0
      || preferenceGuidanceProviderApproved
      || Object.values(sourcePermissions).some(Boolean)
    );
    const existing = bundleId && !hasOtherSelector
      ? mappings.find((mapping) => selectorsEqual(mapping, {
        bundle_id: bundleId,
        executable: null,
        browser_host: null,
        browser_path_prefix: null,
        focused_field: null,
      }))
      : undefined;
    if (existing) {
      loadMapping(existing);
    } else {
      setEditingMappingId(null);
      setPreferenceRevisionBaseline({ mappingId: null, revision: null });
      setSelectedApplicationId(bundleId);
    }
  };

  const selectWritingTask = (value: WritingTaskId | "") => {
    if (value === selectedWritingTaskId) return;
    setSelectedWritingTaskId(value);
    const existing = mappings.find((mapping) => mapping.id === editingMappingId);
    setWritingPreferenceDraft(preferenceDraftFrom(existing, value));
    setPreferenceGuidanceProviderApproved(existing?.writing_task_id === value
      && existing.preference_guidance_provider_approved === true
      && hasCloudSupportedActivePreference(preferencesFromDraft(
        preferenceDraftFrom(existing, value),
        value,
      )));
  };

  const applyPrivacyPreset = (preset: Exclude<ContextPrivacyPreset, "custom">) => {
    setSourcePermissions(preset === "off"
      ? { ax_text: false, local_ocr: false, cloud_vision: false, context_text_to_providers: false }
      : preset === "local"
        ? { ax_text: true, local_ocr: true, cloud_vision: false, context_text_to_providers: false }
        : { ax_text: true, local_ocr: false, cloud_vision: false, context_text_to_providers: true });
    setStyleExamplesApproved(false);
    setPreferenceGuidanceProviderApproved(false);
  };

  const setPreferenceValue = (dimension: WritingPreferenceDimension, value: WritingPreferenceValue | "") => {
    setWritingPreferenceDraft((current) => ({
      ...current,
      [dimension]: value
        ? { value, status: "active" }
        : { value: "", status: "" },
    }));
    if (!value) setPreferenceGuidanceProviderApproved(false);
  };

  const setPreferenceStatus = (dimension: WritingPreferenceDimension, status: WritingPreferenceStatus | "") => {
    const current = writingPreferenceDraft[dimension];
    if (!current.value && status) {
      setErrorScope("mapping");
      setError(t("请先选择要保存的偏好值"));
      return;
    }
    if (status === "forgotten") {
      setPendingConfirm({
        title: t("忘记此写作偏好"),
        description: t("忘记后会保留撤销标记，并停止在此任务中应用该偏好。确认保存后生效。"),
        confirmLabel: t("忘记偏好"),
        action: { type: "forget_preference", dimension },
      });
      return;
    }
    const next = { ...writingPreferenceDraft, [dimension]: { ...current, status } };
    setWritingPreferenceDraft(next);
    if (!Object.values(next).some((item) => item.status === "active")) {
      setPreferenceGuidanceProviderApproved(false);
    }
  };

  const toggle = async (nextEnabled: boolean) => {
    setBusyAction("context");
    setErrorScope("context");
    setError(null);
    try {
      await invoke("set_context_enabled", { enabled: nextEnabled });
      setEnabled(nextEnabled);
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const requestBrowserAccess = async () => {
    setBusyAction("browser");
    setErrorScope("browser");
    setError(null);
    try {
      await invoke("request_browser_access");
      const next = await invoke<ContextSnapshot>("get_context_snapshot");
      setSnapshot(next);
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const setOverride = async (value: string) => {
    setErrorScope("override");
    const family = value === "auto" ? null : value as ContextFamily;
    setBusyAction("override");
    setError(null);
    try {
      const next = await invoke<ContextSnapshot>("set_context_override", { family });
      setSnapshot(next);
      setOverrideFamily(family);
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const loadWritingPreferenceDrafts = async () => {
    const lifecycle = lifecycleRef.current;
    const [candidates, nextMappings] = await Promise.all([
      invoke<WritingPreferenceCandidate[]>("list_writing_preference_drafts"),
      invoke<ContextMapping[]>("get_context_mappings"),
    ]);
    if (lifecycleRef.current !== lifecycle) return [];
    const nextApplications = mergeApplicationOptions(applicationsRef.current, applicationsFromMappings(nextMappings));
    setMappings(nextMappings);
    setApplications(nextApplications);
    return candidates.map((candidate) => ({
      ...candidate,
      display_scope: writingPreferenceMappingScope(
        nextMappings.find((mapping) => mapping.id === candidate.mapping_id),
        nextApplications,
        t,
      ),
    }));
  };

  const decideWritingPreferenceDrafts = (
    decisions: WritingPreferenceDecisionItem[],
    candidates: WritingPreferenceCandidate[],
  ) => {
    for (const candidate of candidates) {
      preferenceReviewStartRef.current.set(writingPreferenceCandidateKey(candidate), {
        mapping: mappingsRef.current.find((mapping) => mapping.id === candidate.mapping_id),
      });
    }
    return invoke<WritingPreferenceDecisionOutcome[]>("review_writing_preference_drafts", { decisions });
  };

  const refreshMappingsAfterPreferenceReview = async (
    approvedCandidates: WritingPreferenceCandidate[],
    outcomes: WritingPreferenceDecisionOutcome[],
  ) => {
    const lifecycle = lifecycleRef.current;
    const outcomeByCandidate = new Map(outcomes.map((outcome) => [outcome.candidate_id, outcome]));
    const mappingIds = [...new Set(approvedCandidates.map((candidate) => candidate.mapping_id))];
    setPreferenceReviewRefreshIds((current) => new Set([...current, ...mappingIds]));

    const nextMappings = await invoke<ContextMapping[]>("get_context_mappings");
    if (lifecycleRef.current !== lifecycle) return;
    const reconciliations: { candidate: WritingPreferenceCandidate; mapping: ContextMapping; preference: ContextWritingPreference }[] = [];
    for (const candidate of approvedCandidates) {
      const outcome = outcomeByCandidate.get(candidate.candidate_id);
      const mapping = nextMappings.find((item) => item.id === candidate.mapping_id);
      if (outcome?.status !== "approved"
        || outcome.mapping_id !== candidate.mapping_id
        || typeof outcome.new_mapping_revision !== "number") {
        throw new Error("preference_review_mapping_refresh_incomplete");
      }
      // get_context_mappings is the authoritative owner snapshot. A successful
      // approval may race with an explicit mapping deletion; do not keep the
      // whole batch stuck retrying an owner that no longer exists.
      if (!mapping) continue;
      const preference = mapping.writing_preferences?.find((item) =>
        (item.task_id ?? mapping.writing_task_id) === candidate.task_id
        && item.dimension === candidate.dimension,
      );
      if (typeof mapping.preference_revision !== "number"
        || mapping.preference_revision < outcome.new_mapping_revision
        || !preference
        || preference.value !== candidate.value
        || preference.status !== "active") {
        throw new Error("preference_review_mapping_refresh_incomplete");
      }
      reconciliations.push({ candidate, mapping, preference });
    }

    setMappings(nextMappings);
    setApplications((current) => mergeApplicationOptions(current, applicationsFromMappings(nextMappings)));
    const conflicts: PreferenceReviewConflict[] = [];
    const editor = preferenceReviewEditorRef.current;
    for (const { candidate, mapping, preference } of reconciliations) {
      if (editor.editingMappingId !== candidate.mapping_id) continue;
      setPreferenceReviewScopeConflictId(mapping.id);
      const candidateKey = writingPreferenceCandidateKey(candidate);
      const savedBeforeReview = preferenceReviewStartRef.current.get(candidateKey)?.mapping;
      const currentSelectors = mappingSelectorsFromEditor(editor);
      const scopeUnchanged = savedBeforeReview
        && savedBeforeReview.preference_revision === candidate.mapping_revision
        && editor.preferenceRevisionBaseline.mappingId === candidate.mapping_id
        && editor.preferenceRevisionBaseline.revision === candidate.mapping_revision
        && savedBeforeReview.family === editor.selectedModeFamily
        && editor.selectedWritingTaskId === candidate.task_id
        && currentSelectors !== null
        && selectorsEqual(savedBeforeReview, currentSelectors);
      if (!scopeUnchanged) {
        conflicts.push({ candidateId: candidateKey, mappingId: mapping.id, taskId: candidate.task_id, dimension: candidate.dimension, reason: "scope_changed" });
        continue;
      }

      const savedPreference = preferenceDraftFrom(savedBeforeReview, candidate.task_id)[candidate.dimension];
      let preferenceDraftChanged = false;
      setWritingPreferenceDraft((current) => {
        if (!samePreferenceDraftItem(current[candidate.dimension], savedPreference)) {
          preferenceDraftChanged = true;
          return current;
        }
        return { ...current, [candidate.dimension]: { value: preference.value, status: preference.status } };
      });
      if (preferenceDraftChanged) {
        conflicts.push({ candidateId: candidateKey, mappingId: mapping.id, taskId: candidate.task_id, dimension: candidate.dimension, reason: "preference_draft_changed" });
      }

      const savedCloudGrant = savedBeforeReview.preference_guidance_provider_approved === true
        && hasCloudSupportedActivePreference(preferencesFromDraft(
          preferenceDraftFrom(savedBeforeReview, candidate.task_id),
          candidate.task_id,
        ));
      if (editor.preferenceGuidanceProviderApproved === savedCloudGrant) {
        setPreferenceGuidanceProviderApproved(mapping.preference_guidance_provider_approved === true
          && hasCloudSupportedActivePreference(mapping.writing_preferences ?? []));
      } else {
        conflicts.push({ candidateId: candidateKey, mappingId: mapping.id, taskId: candidate.task_id, dimension: candidate.dimension, reason: "provider_grant_changed" });
      }
      preferenceReviewStartRef.current.delete(candidateKey);
    }

    if (conflicts.length > 0) {
      setPreferenceReviewConflicts((current) => [
        ...current.filter((existing) => !conflicts.some((conflict) => conflict.candidateId === existing.candidateId)),
        ...conflicts,
      ]);
    }
    const editedMappingIds = new Set(reconciliations
      .filter(({ candidate }) => editor.editingMappingId === candidate.mapping_id)
      .map(({ candidate }) => candidate.mapping_id));
    for (const mappingId of editedMappingIds) {
      if (conflicts.some((conflict) => conflict.mappingId === mappingId)) continue;
      const refreshedMapping = nextMappings.find((mapping) => mapping.id === mappingId);
      if (typeof refreshedMapping?.preference_revision !== "number") continue;
      setPreferenceRevisionBaseline((current) => current.mappingId === mappingId
        ? { mappingId, revision: refreshedMapping.preference_revision ?? 0 }
        : current);
    }
    setPreferenceReviewRefreshIds((current) => {
      const next = new Set(current);
      for (const id of mappingIds) next.delete(id);
      return next;
    });
    for (const candidate of approvedCandidates) {
      preferenceReviewStartRef.current.delete(writingPreferenceCandidateKey(candidate));
    }
  };

  const reloadMappingAfterPreferenceReview = (mappingId: string) => {
    const mapping = mappingsRef.current.find((item) => item.id === mappingId);
    if (!mapping) {
      setErrorScope("mapping");
      setError(t("此 App 规则已不可用，请刷新规则列表后重新选择。"));
      return;
    }
    loadMapping(mapping);
  };

  const saveMapping = async () => {
    setErrorScope("mapping");
    if (editingMappingId && preferenceReviewConflicts.some((conflict) => conflict.mappingId === editingMappingId)) {
      setError(t("批量复核期间此 App 规则的偏好或授权草稿发生冲突。草稿仍保留；请重新加载获批规则后再保存。"));
      return;
    }
    if (editingMappingId && preferenceReviewRefreshIds.has(editingMappingId)) {
      setError(t("偏好已批准，但此 App 规则的最新修订尚未同步。请先在候选复核区重试同步，再保存映射。"));
      return;
    }
    if (editingMappingId && !mappings.some((mapping) => mapping.id === editingMappingId)) {
      setError(t("此映射已删除或不再可用，请重新选择规则后再保存。"));
      return;
    }
    const activeMapping = mappings.find((mapping) => mapping.id === editingMappingId);
    if (activeMapping && (preferenceRevisionBaseline.mappingId !== activeMapping.id
      || preferenceRevisionBaseline.revision !== (activeMapping.preference_revision ?? 0))) {
      setError(t("此 App 规则在当前草稿建立后已更新。草稿仍保留；请重新加载最新规则后再保存，避免覆盖已批准偏好。"));
      return;
    }
    const partialPreference = Object.values(writingPreferenceDraft).some(({ value, status }) => Boolean(value) !== Boolean(status));
    if (partialPreference) {
      setError(t("每个偏好值都需要选择保存、暂停或忘记状态"));
      return;
    }
    const preferenceRecords = preferencesFromDraft(writingPreferenceDraft, selectedWritingTaskId);
    if (preferenceRecords.length > 0 && !selectedWritingTaskId) {
      setError(t("保存任务偏好前，请先明确选择写作任务"));
      return;
    }
    const application = applications.find((item) => item.bundle_id === selectedApplicationId);
    const mode = savedWritingModes.find((item) => item.id === selectedModeId);
    if (!mode) {
      setError(t("请先选择一个语气"));
      return;
    }
    const rawHost = selectedWebsite.trim();
    const browserHost = normalizeBrowserHost(rawHost);
    if (rawHost && !browserHost) {
      setError(t("请输入主机名，不要粘贴网址、路径或查询参数"));
      return;
    }
    const pathPrefix = browserPathPrefix.trim();
    if (pathPrefix && (
      !pathPrefix.startsWith("/")
      || pathPrefix.includes("//")
      || /[\\\s?#]/.test(pathPrefix)
    )) {
      setError(t("路径前缀必须以 / 开头，且不能含空格、双斜线、查询或片段"));
      return;
    }
    if (pathPrefix && !browserHost) {
      setError(t("路径前缀需要网站主机名"));
      return;
    }
    const executable = selectedExecutable.trim() || null;
    if (!application && !executable && !browserHost && !focusedField) {
      setError(t("至少选择一个匹配条件"));
      return;
    }
    const hasTaskAnchor = Boolean(application?.bundle_id || executable || browserHost);
    const invalidTaskField = ["unknown", "secure", "search"].includes(focusedField);
    const hasTaskDisambiguator = !invalidTaskField && Boolean(
      focusedField
      || (browserHost && pathPrefix)
      || (!browserHost && application?.bundle_id && knownNativeTaskBundleIds.has(application.bundle_id.toLowerCase()))
    );
    if (selectedWritingTaskId && (!hasTaskAnchor || !hasTaskDisambiguator)) {
      setError(t("指定写作任务需要认可的原生 App、可信输入框，或精确网站路径"));
      return;
    }
    const selectors = {
      bundle_id: application?.bundle_id ?? null,
      executable,
      browser_host: browserHost,
      browser_path_prefix: pathPrefix || null,
      focused_field: focusedField || null,
    };
    if (editingMappingId && preferenceReviewScopeConflictId === editingMappingId) {
      const persisted = mappings.find((mapping) => mapping.id === editingMappingId);
      if (persisted && (persisted.family !== mode.family || !selectorsEqual(persisted, selectors))) {
        setError(t("批量复核已更新此 App 规则的偏好。当前范围与已保存范围不同；为避免把批准的偏好移到其他范围，请恢复已保存范围后再保存。表单草稿仍保留。"));
        return;
      }
      setPreferenceReviewScopeConflictId(null);
    }
    const savedSelectorMatch = mappings.find((mapping) => selectorsEqual(mapping, selectors));
    if (savedSelectorMatch && savedSelectorMatch.id !== editingMappingId) {
      if (!editingMappingId) loadMapping(savedSelectorMatch);
      setErrorScope("mapping");
      setError(t("此目标已有保存规则；请先检查其来源权限，再明确更新。"));
      return;
    }
    const cloudVisionNeedsNativeApp = sourcePermissions.cloud_vision && !selectors.bundle_id && !selectors.executable;
    if (cloudVisionNeedsNativeApp) {
      setError(t("自动云端视觉需要具体 App 或可执行文件；请关闭此权限或添加 App 选择器"));
      return;
    }
    const existing = mappings.find((mapping) => mapping.id === editingMappingId);
    const declaredScopeChanged = Boolean(existing && (
      existing.family !== mode.family || !selectorsEqual(existing, selectors)
    ));
    const mappingId = existing?.id ?? newMappingId();
    const mappingLabel = existing?.label
      ?? (application?.label ?? (executable ? t("自定义 App 规则") : t("网站规则")));
    setBusyAction("mapping");
    setError(null);
    try {
      const mapping: ContextMapping = {
        id: mappingId,
        label: mappingLabel,
        family: mode.family,
        mode_id: mode.id,
        writing_task_id: selectedWritingTaskId || null,
        writing_preferences: preferenceRecords,
        preference_guidance_provider_approved: Boolean(
          selectedWritingTaskId
          && preferenceGuidanceProviderApproved
          && hasCloudSupportedActivePreference(preferenceRecords),
        ),
        preference_revision: existing
          ? preferenceRevisionBaseline.mappingId === existing.id ? preferenceRevisionBaseline.revision ?? 0 : 0
          : 0,
        ...selectors,
        source_permissions: { ...sourcePermissions },
        style_examples_approved: styleExamplesApproved,
        enabled: true,
        cleanup_effort: null,
        cleanup_intensity: mappingIntensity === "inherit" ? null : mappingIntensity,
        cleanup_enabled: mappingCleanupEnabled,
        dictionary_learn_enabled: mappingLearnEnabled,
      };
      const styleInput = styleExampleInput.trim();
      const styleOutput = styleExampleOutput.trim();
      mapping.style_example_input = styleInput || null;
      mapping.style_example_output = styleOutput || null;
      mapping.style_example_pairs = removeStyleExamples ? [] : existing?.style_example_pairs ?? [];
      const next = await invoke<ContextMapping[]>("save_context_mapping", { mapping, replaceStyleExamples: true });
      setMappings(next);
      if (declaredScopeChanged) {
        const savedMapping = next.find((saved) => saved.id === mappingId);
        if (!savedMapping) {
          throw new Error(t("保存后的映射未返回；请刷新映射列表后检查保存状态"));
        }
        loadMapping(savedMapping);
        setPreferenceScopeMigrationNotice(true);
        return;
      }
      setRemoveStyleExamples(false);
      setPreferenceReviewScopeConflictId(null);
      setEditingMappingId(null);
      setPreferenceRevisionBaseline({ mappingId: null, revision: null });
      setSelectedApplicationId("");
      setSelectedExecutable("");
      setSelectedWebsite("");
      setBrowserPathPrefix("");
      setFocusedField("");
      setSourcePermissions({
        ax_text: false,
        local_ocr: false,
        cloud_vision: false,
        context_text_to_providers: false,
      });
      setSelectedModeId("general");
      setSelectedWritingTaskId("");
      setWritingPreferenceDraft({
        list_structure: { ...emptyWritingPreferenceDraft.list_structure },
        greeting_tone: { ...emptyWritingPreferenceDraft.greeting_tone },
        terminal_punctuation: { ...emptyWritingPreferenceDraft.terminal_punctuation },
      });
      setPreferenceGuidanceProviderApproved(false);
      setPreferenceScopeMigrationNotice(false);
      setStyleExampleInput("");
      setStyleExampleOutput("");
      setStyleExamplesApproved(false);
      setMappingIntensity("inherit");
      setMappingCleanupEnabled(true);
      setMappingLearnEnabled(true);
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const deleteMapping = async (id: string) => {
    setBusyAction("delete_mapping");
    setErrorScope("mapping");
    setError(null);
    try {
      const nextMappings = await invoke<ContextMapping[]>("delete_context_mapping", { id });
      setMappings(nextMappings);
      if (editingMappingId === id) loadMapping();
      setErrorScope(null);
    } catch (reason) {
      setError(settingsErrorMessage(reason));
    } finally {
      setBusyAction(null);
    }
  };

  const selectedApplication = applications.find((application) => application.bundle_id === selectedApplicationId);
  const selectedBrowserHost = selectedWebsite.trim() ? normalizeBrowserHost(selectedWebsite) : null;
  const selectedSelectors = {
    bundle_id: selectedApplication?.bundle_id ?? null,
    executable: selectedExecutable.trim() || null,
    browser_host: selectedBrowserHost,
    browser_path_prefix: browserPathPrefix.trim() || null,
    focused_field: focusedField || null,
  };
  const selectedExistingMapping = mappings.find((mapping) => selectorsEqual(mapping, selectedSelectors));
  const savedEditingMapping = mappings.find((mapping) => mapping.id === editingMappingId);
  const editingMappingOwnerMissing = Boolean(editingMappingId && !savedEditingMapping);
  const preferenceRevisionBaselineMismatch = Boolean(editingMappingId && savedEditingMapping
    && (preferenceRevisionBaseline.mappingId !== savedEditingMapping.id
      || preferenceRevisionBaseline.revision !== (savedEditingMapping.preference_revision ?? 0)));
  const editedMapping = savedEditingMapping ?? selectedExistingMapping;
  useEffect(() => {
    if (!editingMappingId || !preferenceGuidanceProviderApproved) return;
    const currentOwner = mappings.find((mapping) => mapping.id === editingMappingId);
    const currentDraftSelectors = {
      bundle_id: selectedApplication?.bundle_id ?? null,
      executable: selectedExecutable.trim() || null,
      browser_host: selectedBrowserHost,
      browser_path_prefix: browserPathPrefix.trim() || null,
      focused_field: focusedField || null,
    };
    if (currentOwner && !selectorsEqual(currentOwner, currentDraftSelectors)) {
      setPreferenceGuidanceProviderApproved(false);
    }
  }, [
    editingMappingId,
    mappings,
    preferenceGuidanceProviderApproved,
    selectedApplication?.bundle_id,
    selectedExecutable,
    selectedBrowserHost,
    browserPathPrefix,
    focusedField,
  ]);
  useEffect(() => {
    if (!editingMappingId || !styleExamplesApproved) return;
    const currentOwner = mappings.find((mapping) => mapping.id === editingMappingId);
    const currentDraftSelectors = {
      bundle_id: selectedApplication?.bundle_id ?? null,
      executable: selectedExecutable.trim() || null,
      browser_host: selectedBrowserHost,
      browser_path_prefix: browserPathPrefix.trim() || null,
      focused_field: focusedField || null,
    };
    if (currentOwner && !selectorsEqual(currentOwner, currentDraftSelectors)) {
      setStyleExamplesApproved(false);
    }
  }, [
    editingMappingId,
    mappings,
    styleExamplesApproved,
    selectedApplication?.bundle_id,
    selectedExecutable,
    selectedBrowserHost,
    browserPathPrefix,
    focusedField,
  ]);
  const retainedStylePairs = removeStyleExamples ? [] : editedMapping?.style_example_pairs ?? [];
  const hasStyleExamples = Boolean(
    styleExampleInput.trim()
    || styleExampleOutput.trim()
    || retainedStylePairs.length > 0
  );
  const cloudVisionAvailable = Boolean(selectedApplication?.bundle_id || selectedExecutable.trim());
  const invalidCloudVisionGrant = sourcePermissions.cloud_vision && !cloudVisionAvailable;
  const privacyPreset = contextPrivacyPreset(
    sourcePermissions,
    styleExamplesApproved,
    preferenceGuidanceProviderApproved,
  );
  const promptError = (errorScope === "prompt" ? error : null)
    ?? (saveFailure?.fields.includes("writing_modes") ? saveFailure.message : null);
  const advancedError = (errorScope === "override" ? error : null)
    ?? (saveFailure?.fields.some((field) => field.startsWith("accurate_asr_") || field.startsWith("vision_")) ? saveFailure.message : null);
  return (
    <SettingsShell>
      <SettingsPageHeader
        title={t(combined ? "写作与整理" : automationOnly ? "智能整理" : "语气")}
        description={t(combined ? "设置全局整理方式、场景语气和 App / 网站规则。" : automationOnly ? "根据当前 App、输入框和你说的内容，自动选择合适的 Prompt 和输出格式。" : "编辑语气 Prompt，并为 App 或网站指定固定语气。")}
        saveHint={t(combined ? "强度和输出选项自动保存；语气与 App 规则按各自按钮保存。" : automationOnly ? "更改自动保存" : "更改需保存；草稿在本窗口切页保留。")}
      />
      {initializationError && <SettingsAlert>{initializationError}</SettingsAlert>}
      {errorScope === null && error && <SettingsAlert>{error}</SettingsAlert>}
      {combined && (
        <section aria-label={t("生效优先级")} className="mt-4 rounded-2xl border border-border bg-elevated/50 px-5 py-4">
          <p className="text-xs font-medium uppercase tracking-wide text-tertiary">{t("生效优先级")}</p>
          <p className="mt-2 text-sm leading-6 text-primary">{t("单次快捷键指令 → App 专属规则 → 场景语气卡 → 全局默认档位")}</p>
        </section>
      )}
      {showAutomationSettings && (
        <SmartCleanupSettings
          combined={combined}
          busy={busy}
          busyAction={busyAction}
          enabled={enabled}
          onToggleEnabled={(nextEnabled) => void toggle(nextEnabled)}
          snapshot={snapshot}
          mappings={mappings}
          applications={applications}
          cleanupIntensity={cleanupIntensity}
          onCleanupIntensityChange={onCleanupIntensityChange}
          windowOcrEnabled={windowOcrEnabled}
          onWindowOcrEnabledChange={onWindowOcrEnabledChange}
          onRequestBrowserAccess={() => void requestBrowserAccess()}
          outputMode={outputMode}
          onOutputModeChange={onOutputModeChange}
          translationTargetLanguage={translationTargetLanguage}
          onTranslationTargetLanguageChange={onTranslationTargetLanguageChange}
          onOpenPermissions={onOpenPermissions ? () => openGuideDestination("permissions") : undefined}
          onOpenMappings={onOpenMappings ? () => openGuideDestination("mappings") : undefined}
          advancedError={advancedError}
          overrideFamily={overrideFamily}
          onSetOverride={(value) => void setOverride(value)}
          accurateAsrProvider={accurateAsrProvider}
          onAccurateAsrProviderChange={onAccurateAsrProviderChange}
          accurateAsrModel={accurateAsrModel}
          onAccurateAsrModelChange={onAccurateAsrModelChange}
          accurateAsrBaseUrl={accurateAsrBaseUrl}
          onAccurateAsrBaseUrlChange={onAccurateAsrBaseUrlChange}
          visionProvider={visionProvider}
          onVisionProviderChange={onVisionProviderChange}
          visionModel={visionModel}
          onVisionModelChange={onVisionModelChange}
          contextError={errorScope === "context" ? error : null}
          browserError={errorScope === "browser" ? error : null}
        />
      )}

      {showWritingSettings && (
        <>
          <WritingToneSettings
            combined={combined}
            busy={busy}
            writingModes={writingModes}
            editingMode={editingMode}
            editingModeSelectRef={editingModeSelectRef}
            modePromptRef={modePromptRef}
            allScenesOpen={allScenesOpen}
            onAllScenesOpenChange={setAllScenesOpen}
            onSelectEditingMode={selectEditingMode}
            onAddCustomMode={addCustomMode}
            editingModeLabel={editingModeLabel}
            modeLabelDraft={modeLabelDraft}
            onModeLabelDraftChange={setModeLabelDraft}
            modePromptDraft={modePromptDraft}
            onModePromptDraftChange={setModePromptDraft}
            modeIsDirty={modeIsDirty}
            draftModeId={draftModeId}
            onSaveEditingMode={saveEditingMode}
            onCancelModeEditing={cancelModeEditing}
            onDeleteEditingMode={deleteEditingMode}
            promptError={promptError}
            promptErrorPlacement={promptErrorPlacement}
          />
          <AppMappingEditor
            combined={combined}
            busy={busy}
            busyAction={busyAction}
            draftKey={draftKey}
            onOpenDictionary={onOpenDictionary ? () => openGuideDestination("dictionary") : undefined}
            onOpenPermissions={onOpenPermissions ? () => openGuideDestination("permissions") : undefined}
            preferenceScopeMigrationNotice={preferenceScopeMigrationNotice}
            selectedApplicationId={selectedApplicationId}
            onSelectApplication={selectApplication}
            applicationsLoading={applicationsLoading}
            applications={applications}
            onAddApplicationFromDisk={() => void addApplicationFromDisk()}
            onRefreshApplications={() => void refreshApplications()}
            applicationsError={errorScope === "applications" ? error : null}
            savedWritingModes={savedWritingModes}
            selectedMappingMode={selectedMappingMode}
            onSelectedModeIdChange={setSelectedModeId}
            selectedWritingTaskId={selectedWritingTaskId}
            onSelectWritingTask={selectWritingTask}
            writingPreferenceDraft={writingPreferenceDraft}
            onPreferenceValueChange={setPreferenceValue}
            onPreferenceStatusChange={setPreferenceStatus}
            mappingDetailsRef={mappingDetailsRef}
            selectedExecutable={selectedExecutable}
            onSelectedExecutableChange={setSelectedExecutable}
            selectedWebsite={selectedWebsite}
            onSelectedWebsiteChange={setSelectedWebsite}
            browserPathPrefix={browserPathPrefix}
            onBrowserPathPrefixChange={setBrowserPathPrefix}
            focusedField={focusedField}
            onFocusedFieldChange={setFocusedField}
            hasStyleExamples={hasStyleExamples}
            styleExampleInput={styleExampleInput}
            onStyleExampleInputChange={setStyleExampleInput}
            styleExampleOutput={styleExampleOutput}
            onStyleExampleOutputChange={setStyleExampleOutput}
            mappingIntensity={mappingIntensity}
            onMappingIntensityChange={setMappingIntensity}
            mappingCleanupEnabled={mappingCleanupEnabled}
            onMappingCleanupEnabledChange={setMappingCleanupEnabled}
            mappingLearnEnabled={mappingLearnEnabled}
            onMappingLearnEnabledChange={setMappingLearnEnabled}
            privacyPreset={privacyPreset}
            onApplyPrivacyPreset={applyPrivacyPreset}
            sourcePermissions={sourcePermissions}
            onSourcePermissionsChange={setSourcePermissions}
            styleExamplesApproved={styleExamplesApproved}
            onStyleExamplesApprovedChange={setStyleExamplesApproved}
            preferenceGuidanceProviderApproved={preferenceGuidanceProviderApproved}
            onPreferenceGuidanceProviderApprovedChange={setPreferenceGuidanceProviderApproved}
            retainedStylePairs={retainedStylePairs}
            removeStyleExamples={removeStyleExamples}
            onRemoveAllStyleExamples={() => {
              setRemoveStyleExamples(true);
              setStyleExampleInput("");
              setStyleExampleOutput("");
              setStyleExamplesApproved(false);
            }}
            invalidCloudVisionGrant={invalidCloudVisionGrant}
            mappingError={errorScope === "mapping" ? error : null}
            selectedExistingMapping={selectedExistingMapping}
            editingMappingId={editingMappingId}
            editingMappingOwnerMissing={editingMappingOwnerMissing}
            onResetToNewMapping={() => loadMapping()}
            preferenceRevisionBaselineMismatch={preferenceRevisionBaselineMismatch}
            preferenceReviewConflicts={preferenceReviewConflicts}
            onReloadMappingAfterPreferenceReview={reloadMappingAfterPreferenceReview}
            onSaveMapping={() => void saveMapping()}
            mappings={mappings}
            writingModes={writingModes}
            onLoadMapping={loadMapping}
            onRequestDeleteMapping={(mapping) => setPendingConfirm({
              title: t("删除映射"),
              description: t("确定删除“{name}”吗？").replace("{name}", mapping.label),
              confirmLabel: t("删除映射"),
              action: { type: "delete_mapping", mappingId: mapping.id },
            })}
            onLoadWritingPreferenceDrafts={loadWritingPreferenceDrafts}
            onDecideWritingPreferenceDrafts={decideWritingPreferenceDrafts}
            onRefreshMappingsAfterPreferenceReview={refreshMappingsAfterPreferenceReview}
          />
        </>
      )}
      <ConfirmDialog
        open={pendingConfirm != null}
        title={pendingConfirm?.title ?? ""}
        description={pendingConfirm?.description ?? ""}
        confirmLabel={pendingConfirm?.confirmLabel ?? t("确定")}
        cancelLabel={t("取消")}
        returnFocusRef={confirmationFocusRef}
        onCancel={() => setPendingConfirm(null)}
        onConfirm={() => {
          const action = pendingConfirm?.action;
          setPendingConfirm(null);
          if (action?.type === "select_mode") applySelectEditingMode(action.modeId);
          else if (action?.type === "add_mode") applyAddCustomMode();
          else if (action?.type === "delete_mode") applyDeleteMode(action.modeId);
          else if (action?.type === "delete_mapping") void deleteMapping(action.mappingId);
          else if (action?.type === "forget_preference") {
            const next = {
              ...writingPreferenceDraft,
              [action.dimension]: {
                ...writingPreferenceDraft[action.dimension],
                status: "forgotten" as const,
              },
            };
            setWritingPreferenceDraft(next);
            if (!Object.values(next).some((item) => item.status === "active")) {
              setPreferenceGuidanceProviderApproved(false);
            }
          }
        }}
      />
    </SettingsShell>
  );
}
