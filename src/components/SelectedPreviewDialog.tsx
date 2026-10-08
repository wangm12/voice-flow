import type { RefObject } from "react";
import { useRef } from "react";
import { createPortal } from "react-dom";
import { Check, Copy, X } from "lucide-react";
import { IconButton } from "./IconButton";
import { useDialogBehavior } from "../lib/focusTrap";
import { useI18n } from "../lib/i18n";
import { buttonClass, colors, focusRingClass, secondaryButtonClass } from "../lib/theme";

export type TextActionOperation =
  | "rewrite"
  | "shorten"
  | "translate"
  | "organize"
  | "draft_reply"
  | "modify_exact"
  | "screen_assist";

export type TextActionOutcome = "replaced" | "unverified" | "copied" | "copied_target_changed";

export type TextActionErrorCode =
  | "no_source"
  | "unsupported_instruction"
  | "ambiguous_instruction"
  | "source_too_long"
  | "reply_context_unavailable"
  | "translation_unverifiable"
  | "permission_required"
  | "vision_unavailable"
  | "provider_failed";

export type SelectedActionLifecycle = {
  action_sequence: number;
  transaction_id: string;
  state: "started" | "cancelled" | "completed" | "failed";
};

export type SelectedActionError = {
  action_sequence: number;
  transaction_id: string;
  code: TextActionErrorCode;
};

export type TextActionTargetKind = "selection" | "field_text" | "empty_composer" | "screen";
export type TextActionTargetLabel = "selected_text" | "current_field" | "empty_composer" | "captured_screen";

export type SelectedActionPreview = {
  transaction_id: string;
  action_sequence: number;
  kind: "selected" | "screen";
  operation: TextActionOperation;
  target_kind: TextActionTargetKind;
  target_label: TextActionTargetLabel;
  source_text: string;
  instruction: string;
  delivery_mode: "replace_or_copy" | "clipboard_only";
  delivery_notice: "" | "copy_if_target_changed" | "clipboard_only";
  selected_text: string;
  transcript: string;
  final_text: string;
  thumbnail?: string | null;
  replace_allowed: boolean;
};

const textActionOperations: readonly TextActionOperation[] = [
  "rewrite",
  "shorten",
  "translate",
  "organize",
  "draft_reply",
  "modify_exact",
  "screen_assist",
];

const textActionTargetKinds: readonly TextActionTargetKind[] = [
  "selection",
  "field_text",
  "empty_composer",
  "screen",
];
const textActionTargetLabels: readonly TextActionTargetLabel[] = [
  "selected_text",
  "current_field",
  "empty_composer",
  "captured_screen",
];

const textActionErrorCodes: readonly TextActionErrorCode[] = [
  "no_source",
  "unsupported_instruction",
  "ambiguous_instruction",
  "source_too_long",
  "reply_context_unavailable",
  "translation_unverifiable",
  "permission_required",
  "vision_unavailable",
  "provider_failed",
];

function hasActionIdentity(value: unknown): value is { action_sequence: number; transaction_id: string } {
  if (!value || typeof value !== "object") return false;
  const event = value as { action_sequence?: unknown; transaction_id?: unknown };
  return Number.isSafeInteger(event.action_sequence)
    && (event.action_sequence as number) > 0
    && typeof event.transaction_id === "string"
    && event.transaction_id.length > 0;
}

export function isSelectedActionLifecycle(value: unknown): value is SelectedActionLifecycle {
  if (!hasActionIdentity(value)) return false;
  const state = (value as { state?: unknown }).state;
  return state === "started" || state === "cancelled" || state === "completed" || state === "failed";
}

export function isSelectedActionError(value: unknown): value is SelectedActionError {
  if (!hasActionIdentity(value)) return false;
  return textActionErrorCodes.includes((value as { code?: TextActionErrorCode }).code as TextActionErrorCode);
}

export function isSelectedActionPreview(value: unknown): value is SelectedActionPreview {
  if (!value || typeof value !== "object") return false;
  const preview = value as Partial<SelectedActionPreview>;
  return typeof preview.transaction_id === "string"
    && preview.transaction_id.length > 0
    && Number.isSafeInteger(preview.action_sequence)
    && (preview.action_sequence as number) > 0
    && (preview.kind === "selected" || preview.kind === "screen")
    && textActionOperations.includes(preview.operation as TextActionOperation)
    && textActionTargetKinds.includes(preview.target_kind as TextActionTargetKind)
    && textActionTargetLabels.includes(preview.target_label as TextActionTargetLabel)
    && typeof preview.source_text === "string"
    && typeof preview.instruction === "string"
    && typeof preview.final_text === "string"
    && (preview.delivery_mode === "replace_or_copy" || preview.delivery_mode === "clipboard_only")
    && (preview.delivery_notice === "" || preview.delivery_notice === "copy_if_target_changed" || preview.delivery_notice === "clipboard_only")
    && typeof preview.selected_text === "string"
    && typeof preview.transcript === "string"
    && (preview.thumbnail === undefined || preview.thumbnail === null || typeof preview.thumbnail === "string")
    && typeof preview.replace_allowed === "boolean";
}

export function SelectedPreviewDialog({
  preview,
  draft,
  onDraftChange,
  onCancel,
  onCopy,
  onConfirm,
  busy = false,
  error = null,
  restoreFocusRef,
}: {
  preview: SelectedActionPreview;
  draft: string;
  onDraftChange: (value: string) => void;
  onCancel: () => void;
  onCopy: () => void;
  onConfirm: () => void;
  busy?: boolean;
  error?: string | null;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLElement>(null);

  const operationLabels: Record<TextActionOperation, string> = {
    rewrite: "改写",
    shorten: "精简",
    translate: "翻译",
    organize: "结构整理",
    draft_reply: "起草回复",
    modify_exact: "按指令修改值或术语",
    screen_assist: "看屏幕",
  };
  const targetLabels: Record<TextActionTargetKind, string> = {
    selection: "选中文本",
    field_text: "当前文本框",
    empty_composer: "空白回复输入框",
    screen: "已捕获的屏幕",
  };
  const isScreenAssist = preview.operation === "screen_assist";
  const copyOnly = preview.delivery_mode === "clipboard_only" || preview.delivery_notice === "clipboard_only";

  useDialogBehavior({
    open: true,
    dialogRef,
    onCancel,
    restoreFocusRef,
    isolateBackground: true,
  });

  return createPortal(
    <div className="vf-settings fixed inset-0 z-50 flex items-center justify-center p-6">
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
        style={{ maxHeight: "calc(100dvh - 3rem)" }}
        className={`relative flex min-h-0 w-full max-w-2xl flex-col overflow-hidden rounded-2xl border ${colors.border} ${colors.bg.card} p-6 shadow-elevated outline-none ${focusRingClass}`}
      >
        <div className="flex flex-none items-start justify-between gap-4">
          <div>
            <h2 id="selected-preview-title" className="text-lg font-semibold leading-6 text-primary">
              {isScreenAssist ? t("看屏幕预览") : t("文字操作预览")}
            </h2>
            <p className="mt-1 text-xs leading-5 text-tertiary">
              {isScreenAssist
                ? t("截图和结果只在本次预览中暂存；取消会丢弃，不会保存到历史。")
                : t("你可以编辑结果。确认前不会写入目标，取消不会产生副作用。")}
            </p>
          </div>
          <IconButton label={t("关闭")} icon={<X size={16} aria-hidden="true" />} onClick={onCancel} />
        </div>
        <div className="mt-6 min-h-0 flex-1 space-y-4 overflow-y-auto overscroll-contain">
          <div className="flex flex-wrap gap-x-4 gap-y-1 rounded-lg border border-border bg-elevated/40 px-3 py-2 text-xs">
            <span><span className="text-tertiary">{t("操作")}: </span><span className="font-medium text-primary">{t(operationLabels[preview.operation])}</span></span>
            <span><span className="text-tertiary">{t("目标")}: </span><span className="font-medium text-primary">{t(targetLabels[preview.target_kind])}</span></span>
          </div>
          <div>
            <p className="mb-2 text-xs font-medium text-secondary">{t("VoiceFlow 生成结果")}</p>
            <textarea
              aria-label={t("VoiceFlow 生成结果")}
              value={draft}
              onChange={(event) => onDraftChange(event.target.value)}
              disabled={busy}
              rows={5}
              className={`vf-preview-result w-full resize-none rounded-lg border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm leading-6 outline-none focus:border-accent ${focusRingClass}`}
            />
          </div>
          {preview.delivery_notice === "copy_if_target_changed" && (
            <p className="rounded-lg border border-warning/30 bg-warning/5 px-3 py-2 text-xs leading-5 text-secondary">
              {t("如果确认前目标或来源发生变化，VoiceFlow 会只复制结果，不会覆盖新内容。")}
            </p>
          )}
          {copyOnly && (
            <p className="rounded-lg border border-warning/30 bg-warning/5 px-3 py-2 text-xs leading-5 text-secondary">
              {t("此目标目前只支持复制；确认后不会插入或发送。")}
            </p>
          )}
          {preview.operation === "draft_reply" && (
            <p className="rounded-lg border border-border bg-elevated/40 px-3 py-2 text-xs leading-5 text-secondary">
              {t("回复只会作为草稿写入当前输入框或复制，不会发送。")}
            </p>
          )}
          <details className="vf-inline-disclosure border-t border-border pt-3 text-xs text-secondary">
            <summary className={`cursor-pointer rounded-md font-medium text-primary ${focusRingClass}`}>{t("查看来源和指令")}</summary>
            <div className="mt-4 space-y-4">
          {preview.thumbnail && (
            <div>
              <p className="mb-2 text-xs font-medium text-secondary">{t("窗口截图")}</p>
              <img
                src={preview.thumbnail}
                alt={t("窗口截图")}
                className="max-h-40 w-full rounded-xl border object-contain"
              />
            </div>
          )}
          {preview.source_text && (
            <div>
              <p className="mb-2 text-xs font-medium text-secondary">{t("来源文本")}</p>
              <p className={`rounded-lg border ${colors.border} ${colors.bg.elevated} px-3 py-2 text-sm leading-6 text-secondary whitespace-pre-wrap break-words`}>
                {preview.source_text}
              </p>
            </div>
          )}
          <div>
            <p className="mb-2 text-xs font-medium text-secondary">{t("操作指令")}</p>
            <p className={`rounded-lg border ${colors.border} ${colors.bg.elevated} px-3 py-2 text-sm leading-6 text-secondary whitespace-pre-wrap break-words`}>
              {preview.instruction}
            </p>
          </div>
            </div>
          </details>
        </div>
        {error && <p role="alert" className="mt-4 shrink-0 rounded-lg border border-error/30 bg-error/5 px-3 py-2 text-xs leading-5 text-error-ink">{error}</p>}
        <div className="mt-6 flex flex-none flex-wrap justify-end gap-2">
          <p role="status" aria-live="polite" className="mr-auto flex min-h-9 items-center text-[13px] leading-5 text-secondary">{busy ? t("处理中…") : ""}</p>
          <button type="button" onClick={onCancel} className={secondaryButtonClass}>{t("取消")}</button>
          <button type="button" onClick={onCopy} aria-busy={busy} disabled={busy || !draft.trim()} className={`${secondaryButtonClass} min-w-32`}>
            <Copy size={14} aria-hidden="true" />
            {t("只复制")}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            aria-busy={busy}
            disabled={busy || !draft.trim()}
            className={`${buttonClass} min-w-32`}
          >
            <Check size={14} aria-hidden="true" />
            {copyOnly ? t("确认并复制") : t("确认")}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
