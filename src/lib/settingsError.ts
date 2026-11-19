export const SETTINGS_ERROR_CODES = [
  "invalid_patch",
  "shutdown_in_progress",
  "language_unsupported",
  "preferred_languages_invalid",
  "retired_on_device_asr_model",
  "input_monitoring_required",
  "modifier_event_tap_unavailable",
  "hotkey_conflict",
  "translation_hotkey_conflict",
  "hotkey_invalid",
  "translation_hotkey_invalid",
  "credential_storage_failed",
  "api_key_invalid",
  "rate_limited",
  "network_unavailable",
  "invalid_endpoint_url",
  "dictionary_partial_sync",
  "dictionary_capacity_or_invalid_word",
  "dictionary_file_invalid",
  "local_model_active",
  "local_model_unavailable",
  "mapping_validation_failed",
  "operation_busy",
  "internal",
] as const;

export type SettingsErrorCode = (typeof SETTINGS_ERROR_CODES)[number];

const SETTINGS_ERROR_CODE_SET = new Set<string>(SETTINGS_ERROR_CODES);

export function isSettingsErrorCode(value: unknown): value is SettingsErrorCode {
  return typeof value === "string" && SETTINGS_ERROR_CODE_SET.has(value);
}

export function settingsErrorCode(reason: unknown): SettingsErrorCode | null {
  if (!reason || typeof reason !== "object") return null;
  const code = (reason as { code?: unknown }).code;
  return isSettingsErrorCode(code) ? code : null;
}

export function settingsErrorMessage(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  if (reason && typeof reason === "object" && "message" in reason) {
    const message = (reason as { message: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return String(reason);
}

export function isDictionaryPartialSyncError(reason: unknown): boolean {
  if (settingsErrorCode(reason) === "dictionary_partial_sync") return true;
  return settingsErrorMessage(reason).toLowerCase().startsWith("learning_dictionary_partial:");
}

export function friendlyDictionaryLearningError(
  reason: unknown,
  translate: (source: string) => string,
): string | null {
  const code = settingsErrorCode(reason);
  if (code === "dictionary_partial_sync") {
    return translate("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
  }
  if (code === "dictionary_capacity_or_invalid_word") {
    return translate("词条无效或已达到上限。");
  }
  const message = settingsErrorMessage(reason);
  if (message.toLowerCase().startsWith("learning_dictionary_partial:")) {
    return translate("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
  }
  if (message === "dictionary_capacity_or_invalid_word") {
    return translate("词条无效或已达到上限。");
  }
  return null;
}

export function friendlySettingsError(
  reason: unknown,
  translate: (source: string) => string,
): string {
  const dictionaryMessage = friendlyDictionaryLearningError(reason, translate);
  if (dictionaryMessage) return dictionaryMessage;

  switch (settingsErrorCode(reason)) {
    case "retired_on_device_asr_model":
      return translate("Cohere Transcribe 2B 已停止提供新选择；已有保存配置可以继续保留。");
    case "input_monitoring_required":
      return translate("Fn 快捷键需要输入监控权限。请在系统设置的隐私与安全性中为 VoiceFlow 开启输入监控。");
    case "modifier_event_tap_unavailable":
      return translate("Fn 按键监听暂不可用；请检查输入监控权限后重试。");
    case "hotkey_conflict":
      return translate("快捷键与其他快捷键冲突，请选择不同的组合键。");
    case "translation_hotkey_conflict":
      return translate("翻译快捷键与其他快捷键冲突，请选择不同的组合键。");
    case "hotkey_invalid":
      return translate("快捷键组合无效，请重新设置。");
    case "translation_hotkey_invalid":
      return translate("翻译快捷键需要有效的组合键，请重新设置。");
    case "credential_storage_failed":
      return translate("无法保存到这台 Mac 的钥匙串。请重启 VoiceFlow 后再试。");
    case "api_key_invalid":
      return translate("这个 API Key 无效，请检查后重试。");
    case "rate_limited":
      return translate("验证请求过频，请稍后再试。");
    case "network_unavailable":
      return translate("暂时无法连接服务，请检查网络后重试。");
    case "language_unsupported":
      return translate("当前所选识别语言不受当前转写模型支持，请改选支持列表中的语言。");
    case "preferred_languages_invalid":
      return translate("常用口述语言设置无效（最多选 4 项，不能包含自动识别或重复语言）。");
    case "invalid_endpoint_url":
      return translate("服务地址格式无效，请检查后重试。");
    default:
      break;
  }

  const message = settingsErrorMessage(reason);
  const normalized = message.toLowerCase();
  if (normalized === "retired_on_device_asr_model") {
    return translate("Cohere Transcribe 2B 已停止提供新选择；已有保存配置可以继续保留。");
  }
  if (normalized === "input_monitoring_required") {
    return translate("Fn 快捷键需要输入监控权限。请在系统设置的隐私与安全性中为 VoiceFlow 开启输入监控。");
  }
  if (normalized === "modifier_event_tap_unavailable") {
    return translate("Fn 按键监听暂不可用；请检查输入监控权限后重试。");
  }
  if (
    normalized.includes("hotkey")
    || normalized.includes("accelerator")
    || normalized.includes("shortcut")
  ) {
    if (normalized.includes("conflicts")) {
      return translate(
        normalized.includes("translation")
          ? "翻译快捷键与其他快捷键冲突，请选择不同的组合键。"
          : "快捷键与其他快捷键冲突，请选择不同的组合键。",
      );
    }
    return translate(
      normalized.includes("translation")
        ? "翻译快捷键需要有效的组合键，请重新设置。"
        : "快捷键组合无效，请重新设置。",
    );
  }
  if (
    normalized.includes("credential_storage")
    || normalized.includes("failed to store api key securely")
    || normalized.includes("credential write")
    || normalized.includes("keychain")
  ) {
    return translate("无法保存到这台 Mac 的钥匙串。请重启 VoiceFlow 后再试。");
  }
  if (
    normalized.includes("api key validation failed")
    || normalized.includes("invalid api key")
    || normalized.includes("invalid_api_key")
    || normalized.includes("incorrect api key")
    || normalized.includes("unauthorized")
    || normalized.includes("http 401")
  ) {
    return translate("这个 API Key 无效，请检查后重试。");
  }
  if (normalized.includes("rate_limited") || normalized.includes("请求过频")) {
    return translate("验证请求过频，请稍后再试。");
  }
  if (normalized.includes("network") || normalized.includes("timeout")) {
    return translate("暂时无法连接服务，请检查网络后重试。");
  }
  if (
    normalized.includes("recognition language")
    && normalized.includes("is not supported by the selected asr model")
  ) {
    return translate("当前所选识别语言不受当前转写模型支持，请改选支持列表中的语言。");
  }
  if (normalized.includes("preferred_languages") || normalized.includes("preferred language")) {
    return translate("常用口述语言设置无效（最多选 4 项，不能包含自动识别或重复语言）。");
  }
  if (
    normalized.includes("invalid url")
    || normalized.includes("invalid base_url")
    || normalized.includes("invalid endpoint")
    || normalized.includes("custom_base_url")
    || normalized.includes("asr_base_url")
    || normalized.includes("cleanup_base_url")
    || normalized.includes("accurate_asr_base_url")
    || normalized.includes("ollama_base_url")
    || normalized.includes("missing host")
    || normalized.includes("relative url")
    || normalized.includes("unsupported url scheme")
  ) {
    return translate("服务地址格式无效，请检查后重试。");
  }
  return translate(message);
}
