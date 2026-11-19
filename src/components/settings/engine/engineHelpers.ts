import { capabilityLabel, providerById, type ProviderId } from "../../../lib/providers";
import { type EngineDraft } from "../../../lib/engineWizard";
import { colors, focusRingClass, radius } from "../../../lib/theme";
import type { LocalCleanupModelStatus, MlxCleanupModelStatus } from "../../../types/settings";

export const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;
export const LEGACY_MLX_CLEANUP_MODEL_ID = "qwen3_8_27b_mlx_6bit";

export type ProbeStage = {
  ok: boolean;
  skipped: boolean;
  error_kind?: string | null;
  message?: string;
};

export type ProbeResult = {
  asr: ProbeStage;
  cleanup: ProbeStage;
};

export type EnginePage = "service" | "api_key" | "local_models" | "advanced";

export const ENGINE_PAGES: ReadonlyArray<{ id: EnginePage; label: string; description: string }> = [
  { id: "service", label: "服务配置", description: "配置转写和整理路线；更改在保存并使用后生效。" },
  { id: "api_key", label: "API Key", description: "按服务商管理凭据；密钥保存在这台 Mac 的钥匙串中。" },
  { id: "local_models", label: "本地模型", description: "下载并校验后，可直接设为当前使用模型；模型在这台 Mac 上运行。" },
  { id: "advanced", label: "高级设置", description: "管理严格离线策略、连接地址和高级请求参数。" },
];

export function formatModelBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(exponent === 0 ? 0 : 1)} ${units[exponent]}`;
}

function modelParameterTier(label: string): string | null {
  return label.match(/\b(?:E\d+(?:\.\d+)?|\d+(?:\.\d+)?)B\b/i)?.[0] ?? null;
}

export function onDeviceSpeedCharacteristic(modelId: string, label: string, t: (key: string) => string): string {
  const tier = modelParameterTier(label);
  switch (modelId) {
    case "qwen3-asr-0.6b": return `${t("自回归语音识别")} · ${tier ?? "0.6B"} · ${t("较小本机模型档")}`;
    case "qwen3-asr-1.7b": return `${t("自回归语音识别")} · ${tier ?? "1.7B"} · ${t("较大本机模型档")}`;
    case "parakeet-tdt-0.6b-v3-8bit": return `${t("FastConformer + TDT 解码")} · ${tier ?? "0.6B"}`;
    case "firered-asr2-aed-mlx": return t("Attention-based Encoder-Decoder (AED)");
    case "fun-asr-nano-2512-mlx": return `${t("ASR 架构特点未核实")} · ${tier ?? "800M"} · ${t("运行时 prompt: auto / zh / en")}`;
    case "cohere-transcribe-2b": return `${t("旧版路线；模型结构与速度特点未核实")} · ${tier ?? "2B"}`;
    case "sensevoice-small": return t("ASR 架构特点未核实");
    default: return t("ASR 架构特点未核实");
  }
}

export function mlxCleanupSpeedCharacteristic(modelId: string, label: string, t: (key: string) => string): string {
  const tier = modelParameterTier(label);
  const route = `${t("文本生成整理路线")} · ${tier ?? t("参数档未标注")} · MLX`;
  return modelId === "qwen3_8_27b_mlx_6bit"
    ? `${t("旧版路线；模型结构与速度特点未核实")} · ${tier ?? t("参数档未标注")} · MLX`
    : route;
}

export function llamaCleanupSpeedCharacteristic(label: string, t: (key: string) => string): string {
  const tier = modelParameterTier(label);
  return `${t("文本生成整理路线")} · ${tier ?? t("参数档未标注")} · llama.cpp`;
}

export function onDeviceStateCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("模型文件已下载并校验");
    case "downloading": return t("模型文件正在下载");
    case "corrupt": return t("模型文件校验失败，可重新下载");
    case "missing": return t("模型文件未下载");
    default: return `${t("模型文件状态")}：${state}`;
  }
}

export function onDeviceFileSummaryCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("已下载");
    case "downloading": return t("下载中");
    case "corrupt": return t("文件校验失败");
    case "missing": return t("未下载");
    default: return `${t("文件状态")}: ${state}`;
  }
}

export function onDeviceRuntimeCopy(status: string, t: (key: string) => string): string {
  switch (status) {
    case "not_checked": return t("本机运行时尚未检查");
    case "unsupported_platform": return t("需要 Apple Silicon 与 macOS 14 或更新版本");
    case "sidecar_missing": return t("本机推理组件未安装");
    case "runtime_ready": return t("本机运行时握手通过，模型尚未加载");
    case "loading": return t("本机模型正在加载");
    case "loaded": return t("模型已加载");
    case "runtime_failed": return t("本机运行时启动失败");
    case "legacy_no_runtime": return t("旧版模型文件没有 MLX 推理支持");
    default: return `${t("本机运行时状态")}：${status}`;
  }
}

export function cleanupModelStoreStateCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("模型文件已校验");
    case "installed": return t("模型已安装；首次加载会校验全部文件");
    case "missing": return t("尚未安装模型文件");
    case "partial": return t("下载尚未完成；可以继续下载或删除部分文件。");
    case "downloading": return t("正在下载本机模型");
    case "importing": return t("正在导入并校验本机模型");
    case "deleting": return t("正在删除本机模型");
    case "cancelled": return t("已取消");
    case "invalid": return t("模型清单或文件校验失败");
    case "error": return t("本机模型操作失败");
    default: return t("正在读取模型状态");
  }
}

export function cleanupModelStoreSummaryCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("已安装");
    case "installed": return t("已安装");
    case "missing": return t("未下载");
    case "partial": return t("下载未完成");
    case "downloading": return t("下载中");
    case "importing": return t("导入中");
    case "deleting": return t("删除中");
    case "cancelled": return t("已取消");
    case "invalid": return t("文件校验失败");
    case "error": return t("操作失败");
    default: return t("状态未知");
  }
}

export function localCleanupModelStateCopy(state: string, verified: boolean, t: (key: string) => string, partialManaged = false): string {
  switch (state) {
    case "missing": return partialManaged ? t("下载未完成") : t("尚未下载");
    case "downloading": return t("正在下载");
    case "installed": return verified ? t("已下载并校验") : t("文件尚未校验");
    case "corrupt": return t("文件校验失败");
    case "deleting": return t("正在删除");
    default: return `${t("文件状态")}: ${state}`;
  }
}

export function mlxCleanupModelStateCopy(model: MlxCleanupModelStatus, t: (key: string) => string): string {
  if (model.operation === "verify") return t("正在校验模型文件");
  switch (model.state) {
    case "missing": return model.partial_managed ? t("下载未完成") : t("尚未下载");
    case "downloading": return t("正在下载");
    case "canceling": return t("正在取消下载");
    case "installed": return model.verified ? t("已下载并校验") : t("文件尚未校验");
    case "ready": return model.verified ? t("已下载并校验") : t("已安装；首次使用时会重新校验");
    case "corrupt": return t("文件校验失败");
    case "deleting": return t("正在删除");
    case "error": return t("操作失败");
    default: return `${t("文件状态")}: ${model.state}`;
  }
}

export function mlxCleanupCompatibilityCopy(code: string | null | undefined, t: (key: string) => string): string {
  switch (code) {
    case "runtime_missing": return t("本机推理组件未安装");
    case "unsupported_platform": return t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。");
    case "model_not_verified": return t("模型文件尚未通过校验。");
    case "uncalibrated_memory": return t("此模型的最低内存档尚未完成验证。");
    case "insufficient_memory": return t("当前可用内存不足以运行此模型。");
    default: return code ? `${t("兼容性检查未通过")} (${code})` : t("当前设备不满足此模型的兼容条件。");
  }
}

export function mlxCleanupLanguageCopy(language: string): string | null {
  return language === "Multilingual; product language coverage pending verification" ? null : language;
}

export function isMlxCleanupModelOperationActive(model: MlxCleanupModelStatus): boolean {
  return Boolean(model.operation) || ["downloading", "canceling", "deleting"].includes(model.state);
}

export function isCancellationError(message: string | null | undefined): boolean {
  return /cancel/i.test(message ?? "");
}

export function localCleanupCompatibilityCopy(code: string | null | undefined, t: (key: string) => string): string {
  switch (code) {
    case "runtime_missing": return t("本机 llama.cpp 运行时不可用；请重新安装 VoiceFlow。");
    case "unsupported_platform": return t("当前设备或 macOS 版本不支持此模型。");
    case "model_not_verified": return t("模型文件尚未通过校验。");
    case "uncalibrated_memory": return t("此模型的最低内存档尚未完成验证。");
    case "insufficient_memory": return t("当前可用内存不足以运行此模型。");
    default: return code ? `${t("兼容性检查未通过")} (${code})` : t("当前设备不满足此模型的兼容条件。");
  }
}

export function isReadyLocalCleanupModel(model: LocalCleanupModelStatus | undefined): boolean {
  return Boolean(model
    && model.state === "installed"
    && model.managed
    && model.verified
    && model.compatible
    && !model.operation);
}

export function isSelectableLocalCleanupModel(model: LocalCleanupModelStatus | undefined): boolean {
  return model?.selectable === true && isReadyLocalCleanupModel(model);
}

export function probeCopy(kind: string | null | undefined, message: string | undefined, t: (key: string) => string): string {
  if (kind === "address") return t("地址连不上");
  if (kind === "key") return t("密钥无效");
  if (kind === "model") return t("模型名不被这个接口接受");
  if (kind === "path") return t("请检查接口地址和模型 ID");
  if (kind === "missing_key") return t("缺少密钥");
  if (kind === "provider") return t("服务返回错误");
  if (kind === "on_device_model_missing") return t("本地模型未就绪");
  if (kind === "on_device_inference_unavailable") return t("本机推理不可用；请检查运行时或模型状态后重试");
  return message?.trim() || t("服务返回错误");
}

export function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object).sort().map((key) => `${JSON.stringify(key)}:${stableJson(object[key])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}

export type ProviderKeyRevisions = Partial<Record<ProviderId, number>>;

export function asrDraftIdentity(draft: EngineDraft, keyRevisions: ProviderKeyRevisions): string {
  return JSON.stringify([
    draft.asrProvider,
    draft.asrModel.trim(),
    draft.asrProvider === "custom" ? draft.customBaseUrl.trim() : null,
    draft.asrProvider === "local_whisper" ? draft.localWhisperBaseUrl.trim() : null,
    draft.asrProvider === "dashscope" ? draft.dashscopeRegion : null,
    keyRevisions[draft.asrProvider] ?? 0,
  ]);
}

export function cleanupDraftIdentity(
  draft: EngineDraft,
  enabled: boolean,
  keyRevisions: ProviderKeyRevisions,
): string {
  return JSON.stringify([
    enabled,
    draft.cleanupProvider,
    draft.cleanupModel.trim(),
    draft.cleanupProvider === "custom" ? draft.customBaseUrl.trim() : null,
    draft.cleanupProvider === "custom" ? draft.customCleanupParametersText : null,
    draft.cleanupProvider === "ollama" ? draft.ollamaBaseUrl.trim() : null,
    keyRevisions[draft.cleanupProvider] ?? 0,
  ]);
}

export function capabilityText(id: ProviderId, t: (key: string) => string): string {
  const definition = providerById(id);
  const capabilities = id === "custom"
    ? ["asr" as const, "llm" as const]
    : definition?.capabilities ?? [];
  const kind = capabilityLabel(capabilities);
  if (kind === "both") return `${t("转写")} · ${t("润色")}`;
  if (kind === "asr") return t("转写");
  return t("润色");
}

export function providerHasCredentialInput(id: ProviderId): boolean {
  return id !== "on_device" && id !== "on_device_mlx" && id !== "on_device_llama_cpp";
}

export function keepEngineControlInsideRails(target: HTMLElement, alignFirst = false) {
  if (!target.closest(".vf-engine-settings") || target.closest(".vf-engine-footer")
    || target.closest("[data-radix-popper-content-wrapper]")) return;
  if (alignFirst) target.scrollIntoView?.({ block: "nearest" });
  const scrollPane = target.closest<HTMLElement>(".vf-settings-scroll-pane");
  if (!scrollPane) return;
  const paneBounds = scrollPane.getBoundingClientRect();
  const footerTop = document.querySelector<HTMLElement>(".vf-engine-footer")?.getBoundingClientRect().top;
  const bounds = target.getBoundingClientRect();
  const visibleTop = paneBounds.top + 8;
  const visibleBottom = (footerTop ?? paneBounds.bottom) - 8;
  if (bounds.bottom > visibleBottom) scrollPane.scrollTop += bounds.bottom - visibleBottom;
  else if (bounds.top < visibleTop) scrollPane.scrollTop -= visibleTop - bounds.top;
}

