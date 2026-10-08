import { useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { IconButton } from "./IconButton";
import { useDialogBehavior } from "../lib/focusTrap";
import { useI18n } from "../lib/i18n";
import { buttonClass, colors, focusRingClass } from "../lib/theme";

export type WhatsNewPayload = {
  version: string;
  notes: string | null;
};

export function WhatsNewDialog({
  payload,
  onDismiss,
}: {
  payload: WhatsNewPayload;
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLElement>(null);

  useDialogBehavior({
    open: true,
    dialogRef,
    onCancel: onDismiss,
    isolateBackground: true,
  });

  return createPortal(
    <div className="vf-settings fixed inset-0 z-50 flex items-center justify-center p-6">
      <div role="presentation" className="absolute inset-0 bg-black/20 backdrop-blur-[2px]" onClick={onDismiss} />
      <section
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="whats-new-title"
        onClick={(event) => event.stopPropagation()}
        className={`relative flex max-h-[min(calc(100dvh-3rem),640px)] w-full max-w-xl flex-col rounded-2xl border ${colors.border} ${colors.bg.card} shadow-elevated outline-none ${focusRingClass}`}
      >
        <div className="flex items-start justify-between gap-4 border-b border-border px-6 py-6">
          <div>
            <h2 id="whats-new-title" className="text-lg font-semibold leading-6 text-primary">{t("更新说明")}</h2>
            <p className="mt-1 text-xs text-tertiary">{payload.version}</p>
          </div>
          <IconButton label={t("关闭")} icon={<X size={16} aria-hidden="true" />} onClick={onDismiss} />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
          {payload.notes
            ? <div className="space-y-4 break-words text-sm leading-6 text-secondary">
                {payload.notes.trim().split(/\r?\n\s*\r?\n/).map((block, index) => {
                  const heading = block.match(/^#{1,6}\s+(.+)$/);
                  if (heading) return heading[1] === `VoiceFlow ${payload.version}` ? null
                    : <h3 key={index} className="text-sm font-semibold text-primary">{t(heading[1])}</h3>;
                  const lines = block.split(/\r?\n/);
                  if (lines.every((line) => /^[-*]\s+/.test(line))) return <ul key={index} className="list-disc space-y-2 pl-5">{lines.map((line, itemIndex) => <li key={itemIndex}>{t(line.replace(/^[-*]\s+/, ""))}</li>)}</ul>;
                  return <p key={index} className="whitespace-pre-line">{t(block)}</p>;
                })}
              </div>
            : <p className="text-sm leading-6 text-secondary">{t("此版本没有额外说明。")}</p>}
        </div>
        <div className="flex justify-end border-t border-border px-6 py-4">
          <button type="button" className={buttonClass} onClick={onDismiss}>{t("知道了")}</button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
