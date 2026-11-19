import type { RefObject } from "react";
import { Plus } from "lucide-react";
import { Select } from "../Select";
import { SettingsDisclosure, SettingsGroup } from "../SettingsLayout";
import { WritingPreview } from "../WritingPreview";
import { useI18n } from "../../lib/i18n";
import { buttonClass, colors, dangerButtonClass, focusRingClass, secondaryButtonClass } from "../../lib/theme";
import {
  ADD_CUSTOM_MODE_OPTION,
  type ContextFamily,
  type WritingMode,
  coreWritingSceneCards,
  familyLabels,
  familyModeId,
  fieldClass,
} from "./contextShared";

export type WritingToneSettingsProps = {
  combined?: boolean;
  busy: boolean;
  writingModes: WritingMode[];
  editingMode: WritingMode | undefined;
  editingModeSelectRef: RefObject<HTMLButtonElement | null>;
  modePromptRef: RefObject<HTMLTextAreaElement | null>;
  allScenesOpen: boolean;
  onAllScenesOpenChange: (open: boolean) => void;
  onSelectEditingMode: (modeId: string) => void;
  onAddCustomMode: () => void;
  editingModeLabel: string;
  modeLabelDraft: string;
  onModeLabelDraftChange: (value: string) => void;
  modePromptDraft: string;
  onModePromptDraftChange: (value: string) => void;
  modeIsDirty: boolean;
  draftModeId: string | null;
  onSaveEditingMode: (placement: "preview" | "prompt") => void;
  onCancelModeEditing: (placement: "preview" | "prompt") => void;
  onDeleteEditingMode: () => void;
  promptError: string | null;
  promptErrorPlacement: "preview" | "prompt";
};

export function WritingToneSettings({
  combined,
  busy,
  writingModes,
  editingMode,
  editingModeSelectRef,
  modePromptRef,
  allScenesOpen,
  onAllScenesOpenChange,
  onSelectEditingMode,
  onAddCustomMode,
  editingModeLabel,
  modeLabelDraft,
  onModeLabelDraftChange,
  modePromptDraft,
  onModePromptDraftChange,
  modeIsDirty,
  draftModeId,
  onSaveEditingMode,
  onCancelModeEditing,
  onDeleteEditingMode,
  promptError,
  promptErrorPlacement,
}: WritingToneSettingsProps) {
  const { t } = useI18n();
  const inlineError = (message: string | null) => message
    ? <p role="alert" className="mt-2 break-words text-[13px] leading-5 text-error-ink">{message}</p>
    : null;

  return (
    <>
      {combined && (
        <SettingsGroup title={t("核心场景语气卡")} description={t("先选最接近的场景；展开后可编辑全部 15 个细分场景。")}>
          <div className="grid gap-3 px-5 py-4 sm:grid-cols-2">
            {coreWritingSceneCards.map((card, index) => {
              const Icon = card.icon;
              const selected = card.families.some((family) => family === editingMode?.family);
              const targetMode = writingModes.find((mode) => mode.id === card.modeId);
              const activeRepresentative = editingMode?.id === card.modeId;
              const hasRepresentative = coreWritingSceneCards.some((candidate) => candidate.modeId === editingMode?.id);
              return (
                <button
                  key={card.modeId}
                  ref={activeRepresentative || (!hasRepresentative && index === 0) ? editingModeSelectRef : undefined}
                  type="button"
                  aria-label={t(card.label)}
                  aria-describedby={`core-writing-scene-${card.modeId}-description`}
                  aria-pressed={selected}
                  disabled={busy || !targetMode}
                  onClick={() => onSelectEditingMode(card.modeId)}
                  className={`flex min-h-28 flex-col items-start rounded-xl border p-4 text-left transition-colors ${focusRingClass} ${selected ? "border-accent/50 bg-accent-soft" : "border-border bg-card hover:bg-elevated/60"}`}
                >
                  <span className="flex w-full items-center gap-2 text-sm font-semibold text-primary">
                    <Icon size={17} aria-hidden="true" className="shrink-0 text-accent" />
                    <span className="flex-1">{t(card.label)}</span>
                    {selected && <span className="rounded-full bg-accent/10 px-2 py-0.5 text-[10px] font-medium text-accent">{t("正在编辑")}</span>}
                  </span>
                  <span id={`core-writing-scene-${card.modeId}-description`} className="mt-2 text-xs leading-5 text-secondary">{t(card.description)}</span>
                </button>
              );
            })}
          </div>
          <SettingsDisclosure
            variant="plain"
            title={t("全部 15 个细分场景")}
            description={t("每个场景都有独立 Prompt；自定义语气会保留在此列表。")}
            summary={editingMode ? `${t("当前编辑")}: ${t(editingMode.label)}` : t("展开选择")}
            open={allScenesOpen}
            defaultOpen={allScenesOpen}
            onExpandedChange={onAllScenesOpenChange}
          >
            <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {(Object.keys(familyLabels) as ContextFamily[]).map((family) => {
                const mode = writingModes.find((item) => item.id === familyModeId(family));
                const selected = mode?.id === editingMode?.id;
                return (
                  <button
                    key={family}
                    type="button"
                    aria-pressed={selected}
                    disabled={busy || !mode}
                    onClick={() => mode && onSelectEditingMode(mode.id)}
                    className={`min-h-11 rounded-lg border px-3 py-2 text-left text-sm ${focusRingClass} ${selected ? "border-accent/50 bg-accent-soft font-medium text-primary" : "border-border bg-card text-secondary hover:bg-elevated/60 hover:text-primary"}`}
                  >
                    {t(familyLabels[family])}
                  </button>
                );
              })}
              {writingModes.filter((mode) => !mode.builtin).map((mode) => (
                <button
                  key={mode.id}
                  type="button"
                  aria-pressed={mode.id === editingMode?.id}
                  disabled={busy}
                  onClick={() => onSelectEditingMode(mode.id)}
                  className={`min-h-11 rounded-lg border px-3 py-2 text-left text-sm ${focusRingClass} ${mode.id === editingMode?.id ? "border-accent/50 bg-accent-soft font-medium text-primary" : "border-border bg-card text-secondary hover:bg-elevated/60 hover:text-primary"}`}
                >
                  {t(mode.label)} · {t("自定义")}
                </button>
              ))}
              <button type="button" onClick={onAddCustomMode} disabled={busy} className={`${secondaryButtonClass} min-h-11 justify-center`}>
                <Plus size={15} aria-hidden="true" />{t("添加自定义语气")}
              </button>
            </div>
          </SettingsDisclosure>
        </SettingsGroup>
      )}
      <SettingsGroup title={t("语气与预览")} description={t("编辑与试跑不会切换当前听写语气。更改需保存。")}>
        <div className="px-5 py-4">
          {!combined ? <label className="block text-xs text-secondary">
            {t("编辑哪种语气")}
            <Select ref={editingModeSelectRef} aria-label={t("编辑哪种语气")} value={editingMode?.id ?? ""} onValueChange={(value) => value === ADD_CUSTOM_MODE_OPTION ? onAddCustomMode() : onSelectEditingMode(value)} disabled={busy || !editingMode} className={`mt-2 w-full ${fieldClass}`}>
              {writingModes.map((mode) => <option key={mode.id} value={mode.id}>{t(mode.label)}{mode.builtin ? "" : ` · ${t("自定义")}`}</option>)}
              <option value={ADD_CUSTOM_MODE_OPTION}>＋ {t("添加自定义语气")}</option>
            </Select>
          </label> : (
            <div className="mb-4 flex flex-wrap items-center gap-2 text-sm text-secondary">
              <span>{t("当前编辑场景")}</span>
              <span className="rounded-full bg-elevated px-2.5 py-1 font-medium text-primary">{editingMode ? t(editingMode.label) : t("未选择")}</span>
            </div>
          )}
          {!editingMode?.builtin && <label className="mt-4 block text-xs text-secondary">{t("语气名称")}<input aria-label={t("自定义语气名称")} value={modeLabelDraft} onChange={(event) => onModeLabelDraftChange(event.target.value)} disabled={busy} maxLength={64} className={`mt-2 w-full ${fieldClass}`} /></label>}
          {editingMode && <WritingPreview mode={{ ...editingMode, label: editingModeLabel, prompt: modePromptDraft }} compareSaved={modeIsDirty && editingMode.id !== draftModeId} isDraft={modeIsDirty || Boolean(draftModeId)} />}
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <button type="button" onClick={() => onSaveEditingMode("preview")} disabled={busy || !editingMode} className={`${buttonClass} min-w-28`}>{t("保存语气")}</button>
            {(modeIsDirty || draftModeId) && <button type="button" disabled={busy} onClick={() => onCancelModeEditing("preview")} className={secondaryButtonClass}>{t("取消修改")}</button>}
            <span role="status" className="text-xs text-secondary">{t(modeIsDirty || draftModeId ? "有未保存的更改" : "更改需保存")}</span>
            {!editingMode?.builtin && <button type="button" onClick={onDeleteEditingMode} disabled={busy} className={dangerButtonClass}>{t("删除自定义语气")}</button>}
          </div>
          {promptErrorPlacement === "preview" && inlineError(promptError)}
        </div>
      </SettingsGroup>
      <SettingsDisclosure
        title={t("编辑 Prompt")}
        description={t("编辑 Prompt，或创建只属于你的语气。")}
        summary={modeIsDirty || draftModeId ? t("有未保存的更改") : t("可选")}
        error={promptError}
      >
        <div className="pt-4">
          <p className="mb-4 max-w-prose text-xs leading-5 text-secondary">{t("Prompt 只作为写作指导，不会覆盖 VoiceFlow 的事实保护规则。")}</p>
          <label className="block text-xs text-secondary">
            {t("Prompt")}
            <textarea ref={modePromptRef} aria-label={t("语气 Prompt")} value={modePromptDraft} onChange={(event) => onModePromptDraftChange(event.target.value)} disabled={busy} maxLength={8_000} rows={6} className={`mt-2 w-full resize-y rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2.5 text-sm leading-6 outline-none transition-colors duration-150 ${focusRingClass}`} />
            <span className="mt-1 block text-xs text-tertiary">{t("可以用中文或英文描述希望保留什么、如何组织，以及明确禁止添加什么。")} </span>
          </label>
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <button type="button" onClick={() => onSaveEditingMode("prompt")} disabled={busy || !editingMode} className={`${buttonClass} min-w-28`}>{t("保存语气")}</button>
            {(modeIsDirty || draftModeId) && <button type="button" disabled={busy} onClick={() => onCancelModeEditing("prompt")} className={secondaryButtonClass}>{t("取消修改")}</button>}
            <span role="status" className="text-xs text-secondary">{t(modeIsDirty || draftModeId ? "有未保存的更改" : "更改需保存")}</span>
          </div>
          {promptErrorPlacement === "prompt" && inlineError(promptError)}
        </div>
      </SettingsDisclosure>
    </>
  );
}
