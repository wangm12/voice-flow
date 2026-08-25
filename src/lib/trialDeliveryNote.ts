import { deliveryReasonMessage } from "./deliveryCopy";

export function shouldClearTrialDeliveryNote(state: string): boolean {
  return state === "recording" || state === "starting";
}

export function trialDeliveryNote(
  state: string,
  fallbackReason: string | null | undefined,
  translate: (source: string) => string,
): string | null {
  if (state !== "copied" && state !== "unverified" && state !== "degraded" && state !== "error") {
    return null;
  }
  if (fallbackReason) {
    return deliveryReasonMessage(fallbackReason, translate);
  }
  if (state === "copied") return translate("已复制到剪贴板，请手动粘贴");
  if (state === "error") return translate("语音输入失败，请重试");
  return null;
}
