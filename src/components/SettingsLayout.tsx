import type { ReactNode } from "react";
import { AlertCircle } from "lucide-react";

type SettingsPageHeaderProps = {
  title: string;
  description: string;
  actions?: ReactNode;
};

export function SettingsPageHeader({ title, description, actions }: SettingsPageHeaderProps) {
  return (
    <header className="flex flex-wrap items-start justify-between gap-4">
      <div className="min-w-0">
        <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.025em] text-primary">{title}</h1>
        <p className="mt-2 max-w-2xl text-sm leading-6 text-secondary">{description}</p>
      </div>
      {actions && <div className="shrink-0">{actions}</div>}
    </header>
  );
}

export function SettingsGroup({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <section className="mt-8">
      <div className="mb-2 flex flex-wrap items-baseline justify-between gap-2 px-1">
        <h2 className="text-[11px] font-semibold uppercase tracking-[0.12em] text-tertiary">{title}</h2>
        {description && <p className="text-xs text-tertiary">{description}</p>}
      </div>
      <div className="overflow-hidden rounded-2xl border border-border bg-card/35 divide-y divide-border">
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
    <div className={`flex min-w-0 flex-wrap items-center gap-x-4 gap-y-3 px-4 py-4 sm:px-5 ${className}`}>
      {icon && <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-elevated text-secondary">{icon}</span>}
      <div className="min-w-[12rem] flex-1">
        <p className="text-sm font-medium text-primary">{title}</p>
        {description && <p className="mt-1 max-w-xl text-xs leading-5 text-tertiary">{description}</p>}
      </div>
      {children && <div className="ml-auto flex max-w-full shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

const statusToneClass = {
  success: { dot: "bg-success", text: "text-success" },
  accent: { dot: "bg-accent", text: "text-accent" },
  warning: { dot: "bg-warning", text: "text-warning" },
  error: { dot: "bg-error", text: "text-error" },
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
    <div role="alert" className="mb-6 flex items-start gap-3 rounded-xl border border-error/20 bg-error/5 px-4 py-3 text-sm text-error">
      <AlertCircle size={17} className="mt-0.5 shrink-0" aria-hidden="true" />
      <span className="min-w-0 flex-1">{children}</span>
      {onRetry && retryLabel && <button type="button" onClick={onRetry} className="shrink-0 font-medium underline underline-offset-2">{retryLabel}</button>}
    </div>
  );
}

export function SettingsShell({ children }: { children: ReactNode }) {
  return <div className="mx-auto w-full max-w-3xl pb-12">{children}</div>;
}
