import type { RefObject } from "react";
import { useRef } from "react";
import { createPortal } from "react-dom";
import { Check, Copy, X } from "lucide-react";
import { useDialogBehavior } from "../lib/focusTrap";
import { useI18n } from "../lib/i18n";
import { buttonClass, colors, focusRingClass, secondaryButtonClass } from "../lib/theme";

export type SelectedActionPreview = {
  selected_text: string;
  transcript: string;
  final_text: string;
};

export function SelectedPreviewDialog({
  preview,
  draft,
  onDraftChange,
  onCancel,
  onCopy,
  onConfirm,
  restoreFocusRef,
}: {
  preview: SelectedActionPreview;
  draft: string;
  onDraftChange: (value: string) => void;
  onCancel: () => void;
  onCopy: () => void;
  onConfirm: () => void;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLElement>(null);

  useDialogBehavior({
    open: true,
    dialogRef,
    onCancel,
    restoreFocusRef,
    isolateBackground: true,
  });

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-5">
      <div
        role="presentation"
        className="absolute inset-0 bg-black/20 backdrop-blur-[2px]"
        onClick={onCancel}
      />
      <section
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="selected-preview-title"
        onClick={(event) => event.stopPropagation()}
        className={`relative w-full max-w-2xl rounded-2xl border ${colors.border} ${colors.bg.card} p-5 shadow-2xl outline-none ${focusRingClass}`}
      >
        <div className="flex items-start justify-between gap-4">
          <div>
            <h2 id="selected-preview-title" className="text-base font-semibold text-primary">{t("预览选中文本操作")}</h2>
            <p className="mt-1 text-xs leading-5 text-tertiary">{t("确认后才会替换原文；取消或复制不会修改原输入框。")}</p>
          </div>
          <button
            type="button"
            aria-label={t("取消")}
            onClick={onCancel}
            className={`rounded-lg p-1.5 text-tertiary transition-colors hover:bg-elevated hover:text-primary ${focusRingClass}`}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </div>
        <div className="mt-5 grid gap-4">
          <div>
            <p className="mb-1.5 text-xs font-medium text-secondary">{t("原选中文本")}</p>
            <p className={`max-h-28 overflow-y-auto rounded-xl border ${colors.border} ${colors.bg.elevated} px-3 py-2 text-sm leading-6 text-secondary whitespace-pre-wrap`}>
              {preview.selected_text}
            </p>
          </div>
          <div>
            <p className="mb-1.5 text-xs font-medium text-secondary">{t("VoiceFlow 生成结果")}</p>
            <textarea
              aria-label={t("VoiceFlow 生成结果")}
              value={draft}
              onChange={(event) => onDraftChange(event.target.value)}
              rows={7}
              className={`w-full resize-y rounded-xl border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm leading-6 outline-none focus:border-accent ${focusRingClass}`}
            />
          </div>
          <p className="text-xs leading-5 text-tertiary">{t("语音指令")}: {preview.transcript}</p>
        </div>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button type="button" onClick={onCancel} className={secondaryButtonClass}>{t("取消")}</button>
          <button type="button" onClick={onCopy} disabled={!draft.trim()} className={secondaryButtonClass}>
            <Copy size={14} aria-hidden="true" />
            {t("只复制")}
          </button>
          <button type="button" onClick={onConfirm} disabled={!draft.trim()} className={buttonClass}>
            <Check size={14} aria-hidden="true" />
            {t("替换原文")}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
