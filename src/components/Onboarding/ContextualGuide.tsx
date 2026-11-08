import { useId, useRef, useState, type ReactNode } from "react";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass, focusRingClass } from "../../lib/theme";
import { useGuideAcknowledgement, type OnDemandGuideId } from "../../lib/onboardingGuides";

/** Inline teaching owns only disclosure state; native settings owns acknowledgements. */
export function ContextualGuide({ guideId, title, children, firstUse = false }: {
  guideId: OnDemandGuideId;
  title: string;
  children: ReactNode;
  firstUse?: boolean;
}) {
  const { t } = useI18n();
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const container = useRef<HTMLElement>(null);
  const interaction = useRef(0);
  const [choice, setChoice] = useState<{ guideId: OnDemandGuideId; open: boolean } | null>(null);
  const acknowledgement = useGuideAcknowledgement(guideId);
  const expanded = choice?.guideId === guideId ? choice.open : firstUse && acknowledgement.needsAcknowledgement === true;
  const close = () => {
    interaction.current++;
    setChoice({ guideId, open: false });
    // A native target may have focus while this inline guide is visible.
    if (document.hasFocus() && container.current?.contains(document.activeElement) && trigger.current?.isConnected) trigger.current.focus({ preventScroll: true });
  };

  return <aside ref={container} aria-label={title} className="my-4 min-w-0 rounded-xl border border-border bg-elevated/20 text-[13px] leading-6 text-secondary">
    <button ref={trigger} id={`${id}-trigger`} type="button" aria-expanded={expanded} aria-controls={id}
      onClick={() => { interaction.current++; setChoice({ guideId, open: !expanded }); }}
      className={`flex min-h-11 w-full items-center justify-between gap-3 rounded-xl px-4 py-3 text-left text-sm font-medium text-primary ${focusRingClass}`}>
      <span className="min-w-0 break-words">{title}</span><span aria-hidden="true">{expanded ? "−" : "+"}</span>
    </button>
    <div id={id} role="region" aria-labelledby={`${id}-trigger`} hidden={!expanded} inert={!expanded} className="space-y-3 border-t border-border p-4 [overflow-wrap:anywhere]">
      {children}
      <p className="text-xs text-tertiary">{t("说明可随时重看；关闭不会修改设置，也不表示功能已经验证。")}</p>
      {acknowledgement.error && <p role="alert" className="text-xs text-error-ink">{t(acknowledgement.error)}</p>}
      <div className="flex flex-wrap gap-2">
        <button type="button" className={compactButtonClass} onClick={close}>{t("收起说明")}</button>
        <button type="button" className={compactButtonClass} disabled={acknowledgement.pending} aria-busy={acknowledgement.pending}
          onClick={() => {
            const currentInteraction = ++interaction.current;
            void acknowledgement.acknowledge().then((saved) => { if (saved && interaction.current === currentInteraction) close(); });
          }}>{t("知道了")}</button>
      </div>
    </div>
  </aside>;
}
