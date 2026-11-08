export type CleanupGuardReason =
  | "explicit_layout_changed"
  | "source_script_changed"
  | "protected_token_changed"
  | "empty_completion"
  | "missing_finish"
  | "missing_stop"
  | "truncated"
  | "tool_call"
  | "multiple_choices"
  | "continued_after_finish"
  | "continued_after_done"
  | "invalid_response"
  | "unsupported_content"
  | "unavailable_text"
  | "response_budget";

export type CleanupRejectionStage = "provider_output" | "final_output";

export const cleanupGuardReasonLabels: Record<CleanupGuardReason, string> = {
  explicit_layout_changed: "列表或结构未通过保真检查",
  source_script_changed: "语言或文字体系未通过保真检查",
  protected_token_changed: "名称、数字或事实未通过保真检查",
  empty_completion: "模型返回空内容",
  missing_finish: "模型回复未完整结束",
  missing_stop: "模型回复未完整结束",
  truncated: "模型回复被截断",
  tool_call: "模型返回了工具调用",
  multiple_choices: "模型返回了多个结果",
  continued_after_finish: "模型回复结束后仍有额外内容",
  continued_after_done: "模型结束标记后仍有额外内容",
  invalid_response: "模型回复无法按支持的格式确认",
  unsupported_content: "模型返回了不支持的内容",
  unavailable_text: "模型回复中没有可用文字",
  response_budget: "模型回复超过允许长度",
};

export const cleanupCandidateLabels = {
  provider_output: "服务商候选（保真检查未采纳）",
  final_output: "最终整理候选（保真检查未采纳）",
  unknown: "整理候选（阶段未确认，未采用）",
} as const;

const knownReasons = new Set<string>(Object.keys(cleanupGuardReasonLabels));

function storageReasonCode(value: string | null | undefined): string | null {
  if (!value) return null;
  return value.replace(/^(provider|final)_/, "");
}

export function cleanupGuardReasonLabel(value: string | null | undefined): string {
  const code = storageReasonCode(value);
  return code && knownReasons.has(code)
    ? cleanupGuardReasonLabels[code as CleanupGuardReason]
    : "具体原因未确认";
}

export function cleanupRejectionStage(
  value: CleanupRejectionStage | string | null | undefined,
): CleanupRejectionStage | null {
  if (value === "provider_output" || value?.startsWith("provider_")) return "provider_output";
  if (value === "final_output" || value?.startsWith("final_")) return "final_output";
  return null;
}

export function cleanupCandidateLabel(stage: CleanupRejectionStage | string | null | undefined): string {
  switch (cleanupRejectionStage(stage)) {
    case "provider_output": return cleanupCandidateLabels.provider_output;
    case "final_output": return cleanupCandidateLabels.final_output;
    default: return cleanupCandidateLabels.unknown;
  }
}
