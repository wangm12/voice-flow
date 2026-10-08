import { Check, Loader2 } from "lucide-react";
import { tagClass, validationMessage } from "../../lib/theme";
import { iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function ValidationStatus({ status, validating }: { status: string | null; validating: boolean }) {
  const { t } = useI18n();
  if (validating) {
    return (
        <span key="loading" role="status" aria-live="polite" className={`${tagClass} vf-status-enter bg-elevated text-secondary`}>
          <Loader2 {...iconPropsSm} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
          {t("验证中…")}
        </span>
    );
  }
  if (status) {
    return (
        <span key={status} role={status === "valid" ? "status" : "alert"} aria-live="polite" className={`${tagClass} vf-status-enter ${
            status === "valid"
              ? "bg-success/10 text-success-ink"
              : status === "invalid"
                ? "bg-error/10 text-error-ink"
                : "bg-warning/10 text-warning-ink"
          }`}
        >
          {status === "valid" && <Check {...iconPropsSm} aria-hidden="true" />}
          {validationMessage(status, t)}
        </span>
    );
  }
  return null;
}
