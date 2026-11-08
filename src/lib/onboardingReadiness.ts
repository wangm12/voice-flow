/** Presentation receipts only. Each producing owner retains authority over its stage. */
export type ReadinessStage = "credential_configured" | "provider_probe" | "files_validated" | "runtime_capability" | "model_loaded" | "microphone_frames" | "hotkey_registered" | "native_hotkey" | "physical_hotkey" | "in_app_trial" | "external_delivery" | "clipboard_restoration";
export type ReadinessFact = {
  stage: ReadinessStage;
  state: "not_tested" | "passed" | "failed" | "unknown";
  source: string;
  config_generation: string;
  permission_invalidation?: { source: "check_permissions" | "onboarding_window"; reason: string };
  expires_or_superseded: { session_id: string; configuration_changed: true };
};
export type TrialLease = { trial_id: string; config_generation: string };
export type TrialReceipt = TrialLease & { recording_generation: string; trigger_source: "native_hotkey" | "app_command" | "unknown"; trial_current: true };

export function isNativeGeneration(value: unknown): value is string {
  return typeof value === "string" && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18_446_744_073_709_551_615n;
}

export function isTrialLease(value: unknown): value is TrialLease & { enabled: boolean } {
  if (!value || typeof value !== "object") return false;
  const lease = value as Record<string, unknown>;
  return typeof lease.trial_id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(lease.trial_id)
    && isNativeGeneration(lease.config_generation) && typeof lease.enabled === "boolean";
}

export function matchesTrialIdentity(value: unknown, lease: TrialLease | null): value is TrialLease & { recording_generation: string; trigger_source: TrialReceipt["trigger_source"]; trial_current: boolean } {
  if (!lease || !value || typeof value !== "object") return false;
  const receipt = value as Record<string, unknown>;
  return receipt.trial_id === lease.trial_id && isNativeGeneration(receipt.config_generation) && receipt.config_generation === lease.config_generation
    && isNativeGeneration(receipt.recording_generation) && typeof receipt.trial_current === "boolean"
    && typeof receipt.trigger_source === "string" && ["native_hotkey", "app_command", "unknown"].includes(receipt.trigger_source);
}

export function matchesTrialMetadata(value: unknown, lease: TrialLease | null): value is TrialReceipt {
  return matchesTrialIdentity(value, lease) && value.trial_current;
}

export function readinessFact(stage: ReadinessStage, state: ReadinessFact["state"], source: string, generation: string, sessionId: string): ReadinessFact {
  return { stage, state, source, config_generation: generation, expires_or_superseded: { session_id: sessionId, configuration_changed: true } };
}

export function currentReadiness(facts: ReadinessFact[], generation: string, sessionId: string): ReadinessFact[] {
  return facts.filter((fact) => fact.config_generation === generation && fact.expires_or_superseded.session_id === sessionId)
    .map((fact) => fact.permission_invalidation ? { ...fact, state: "unknown" as const } : fact);
}

export function matchesTrialReceipt(value: unknown, lease: TrialLease | null): value is TrialReceipt & { final_text: string; raw_text?: string } {
  if (!matchesTrialMetadata(value, lease)) return false;
  const receipt = value as Record<string, unknown>;
  return typeof receipt.final_text === "string";
}

export function readinessLabel(state: ReadinessFact["state"], t: (source: string) => string): string {
  return t(state === "passed" ? "本次检查通过" : state === "failed" ? "本次检查失败" : state === "unknown" ? "无法确认" : "尚未测试");
}

export const readinessStageLabels: Record<ReadinessStage, string> = {
  credential_configured: "凭据配置", provider_probe: "服务探测", files_validated: "模型文件校验", runtime_capability: "运行时能力握手", model_loaded: "模型加载", microphone_frames: "实际音频采集", hotkey_registered: "快捷键注册", native_hotkey: "原生快捷键回调", physical_hotkey: "实体快捷键触发", in_app_trial: "应用内听写结果", external_delivery: "外部插入确认", clipboard_restoration: "本次剪贴板恢复",
};

/** Permissions are observations, separate from the native settings generation.
 * Missing, unknown or conflicting readback cannot preserve current trial facts. */
export type OnboardingPermissions = { microphone: boolean; microphone_status: string; accessibility: boolean; screen_recording?: boolean };
export type PermissionReadback =
  | { state: "known"; permissions: OnboardingPermissions; source_value: unknown }
  | { state: "unknown"; permissions: null; source_value: unknown; reason: "unavailable" | "malformed" | "unknown_status" | "conflicting_status" };
export function parseOnboardingPermissions(value: unknown): PermissionReadback {
  if (value == null) return { state: "unknown", permissions: null, source_value: value, reason: "unavailable" };
  if (typeof value !== "object") return { state: "unknown", permissions: null, source_value: value, reason: "malformed" };
  const fact = value as Record<string, unknown>;
  if (typeof fact.microphone !== "boolean" || typeof fact.accessibility !== "boolean" || typeof fact.microphone_status !== "string"
    || (fact.screen_recording !== undefined && typeof fact.screen_recording !== "boolean")) {
    return { state: "unknown", permissions: null, source_value: value, reason: "malformed" };
  }
  if (!["authorized", "not_determined", "restricted", "denied"].includes(fact.microphone_status)) {
    return { state: "unknown", permissions: null, source_value: value, reason: "unknown_status" };
  }
  if (fact.microphone !== (fact.microphone_status === "authorized")) {
    return { state: "unknown", permissions: null, source_value: value, reason: "conflicting_status" };
  }
  return { state: "known", permissions: { microphone: fact.microphone, microphone_status: fact.microphone_status, accessibility: fact.accessibility,
    ...(typeof fact.screen_recording === "boolean" ? { screen_recording: fact.screen_recording } : {}) }, source_value: value };
}
export function sameOnboardingPermissions(a: OnboardingPermissions | null, b: OnboardingPermissions | null): boolean {
  return a === b || (a !== null && b !== null && a.microphone === b.microphone
    && a.microphone_status === b.microphone_status && a.accessibility === b.accessibility && a.screen_recording === b.screen_recording);
}
export function invalidatePermissionReadiness(facts: ReadinessFact[], invalidation: NonNullable<ReadinessFact["permission_invalidation"]>): ReadinessFact[] {
  const dependent: readonly ReadinessStage[] = ["microphone_frames", "native_hotkey", "physical_hotkey", "in_app_trial", "external_delivery", "clipboard_restoration"];
  // Preserve the original receipt/source/state. Current presentation is unknown;
  // regrant cannot reactivate it, and only a new actual owner fact replaces it.
  return facts.map((fact) => !dependent.includes(fact.stage) || fact.permission_invalidation
    ? fact : { ...fact, permission_invalidation: invalidation });
}
