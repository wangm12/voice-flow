import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { AlertCircle, ChevronDown } from "lucide-react";
import { focusRingClass, ghostButtonClass } from "../lib/theme";

export type SettingsSaveFailure = { fields: readonly string[]; message: string };

type SettingsPageHeaderProps = {
  title: string;
  description: string;
  actions?: ReactNode;
};

export function SettingsPageHeader({ title, description, actions }: SettingsPageHeaderProps) {
  return (
    <header className="vf-page-header flex flex-wrap items-start justify-between gap-4">
      <div className="min-w-0 flex-1 basis-64">
        <h1 className="text-[28px] font-semibold leading-[34px] tracking-[-0.025em] text-primary">{title}</h1>
        <p className="mt-2 max-w-prose text-[13px] leading-5 text-secondary">{description}</p>
      </div>
      {actions && <div className="shrink-0">{actions}</div>}
    </header>
  );
}

export function SettingsGroup({ title, description, children, variant = "surface" }: { title: string; description?: string; children: ReactNode; variant?: "plain" | "surface" }) {
  return (
    <section className={`vf-settings-group vf-settings-group--${variant} mt-6`}>
      <div className="vf-settings-group-heading mb-2 flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-base font-medium leading-6 text-primary">{title}</h2>
        {description && <p className="max-w-prose text-xs leading-[18px] text-secondary">{description}</p>}
      </div>
      <div className={`vf-settings-group-body divide-y divide-border ${variant === "surface" ? "rounded-2xl border border-border bg-card" : ""}`}>
        {children}
      </div>
    </section>
  );
}

/** Hiding a group preserves drafts and effects; input blur keeps its existing meaning. */
export function SettingsDisclosure({ title, description, summary, children, defaultOpen = false, locked = false, error, variant = "surface" }: {
  title: string;
  description?: string;
  summary?: ReactNode;
  children: ReactNode;
  defaultOpen?: boolean;
  locked?: boolean;
  error?: string | null;
  variant?: "plain" | "surface";
}) {
  const id = useId();
  const [open, setOpen] = useState(defaultOpen);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const expanded = open || locked || Boolean(error);

  useEffect(() => {
    if (locked || error) setOpen(true);
  }, [locked, error]);

  return (
    <section className={`vf-settings-disclosure vf-settings-disclosure--${variant} mt-6 ${variant === "surface" ? "rounded-xl border border-border bg-card" : ""}`}>
      <h2>
        <button
          ref={triggerRef}
          type="button"
          disabled={locked}
          aria-expanded={expanded}
          aria-controls={id}
          aria-disabled={locked || Boolean(error)}
          onClick={() => {
            if (locked || error) return;
            setOpen(!expanded);
            if (expanded) triggerRef.current?.focus({ preventScroll: true });
          }}
          className={`flex w-full items-center gap-3 rounded-xl px-5 py-4 text-left transition-colors duration-150 motion-reduce:transition-none enabled:hover:bg-elevated/40 enabled:active:bg-elevated ${focusRingClass}`}
        >
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-primary">{title}</span>
            {description && <span className="mt-1 block text-[13px] font-normal leading-5 text-secondary">{description}</span>}
          </span>
          {summary && <span className="max-w-[45%] text-right text-xs leading-5 text-secondary">{summary}</span>}
          <ChevronDown size={16} aria-hidden="true" className={`shrink-0 text-tertiary transition-transform duration-150 motion-reduce:transition-none ${expanded ? "rotate-180" : ""}`} />
        </button>
      </h2>
      <div id={id} hidden={!expanded} inert={!expanded} className="vf-settings-disclosure-content border-t border-border px-5 pb-5">
        {children}
      </div>
    </section>
  );
}

export function SettingsRow({
  title,
  description,
  icon,
  children,
  className = "",
}: {
  title: string;
  description?: ReactNode;
  icon?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div className={`vf-settings-row flex min-w-0 flex-wrap items-center gap-x-4 gap-y-3 px-5 py-4 ${className}`}>
      {icon && <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-elevated text-secondary">{icon}</span>}
      <div className="vf-settings-row-label min-w-0 basis-48 flex-1">
        <p className="text-sm font-medium leading-5 text-primary">{title}</p>
        {description && <div className="mt-1 max-w-prose text-[13px] leading-5 text-secondary">{description}</div>}
      </div>
      {children && <div className="vf-settings-row-control ml-auto flex max-w-full flex-wrap items-center justify-end gap-2">{children}</div>}
    </div>
  );
}

const statusToneClass = {
  success: { dot: "bg-success", text: "text-success-ink" },
  accent: { dot: "bg-accent", text: "text-accent" },
  warning: { dot: "bg-warning", text: "text-warning-ink" },
  error: { dot: "bg-error", text: "text-error-ink" },
  unused: { dot: "bg-tertiary", text: "text-tertiary" },
  neutral: { dot: "bg-tertiary", text: "text-secondary" },
} as const;

export function SettingsStatus({
  label,
  tone = "neutral",
}: {
  label: string;
  tone?: keyof typeof statusToneClass;
}) {
  const toneClass = statusToneClass[tone];
  return (
    <span className={`inline-flex items-center gap-1.5 text-xs font-medium ${toneClass.text}`}>
      <span aria-hidden="true" className={`h-1.5 w-1.5 rounded-full ${toneClass.dot}`} />
      {label}
    </span>
  );
}

export function SettingsAlert({ children, onRetry, retryLabel }: { children: ReactNode; onRetry?: () => void; retryLabel?: string }) {
  return (
    <div role="alert" className="mb-6 flex items-start gap-3 rounded-xl border border-error/20 bg-error/5 px-4 py-3 text-sm text-error-ink">
      <AlertCircle size={17} className="mt-0.5 shrink-0" aria-hidden="true" />
      <span className="min-w-0 flex-1">{children}</span>
      {onRetry && retryLabel && <button type="button" onClick={onRetry} className={ghostButtonClass}>{retryLabel}</button>}
    </div>
  );
}

export function SettingsShell({ children }: { children: ReactNode }) {
  return <div className="vf-settings-shell w-full pb-12">{children}</div>;
}
