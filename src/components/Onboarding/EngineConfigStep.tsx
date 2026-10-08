import { Autocomplete } from "../Autocomplete";
import { Select } from "../Select";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CloudCog, Lock } from "lucide-react";
import { PasswordInput } from "../PasswordInput";
import { buttonClass, compactButtonClass, focusRingClass } from "../../lib/theme";
import { ValidationStatus } from "./ValidationStatus";
import { iconProps, iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";
import { asrLanguageDescription } from "../../lib/engineWizard";
import { asrModelProfile, isProviderId, providerById, providersFor, type DashscopeRegion, type ProviderId } from "../../lib/providers";
import type { OnDeviceModelStatus } from "../../types/settings";

function deviceStateCopy(state: string, t: (key: string) => string): string {
  switch (state) {
    case "ready": return t("模型文件已下载并校验");
    case "downloading": return t("模型文件正在下载");
    case "corrupt": return t("模型文件校验失败，可重新下载");
    case "missing": return t("模型文件未下载");
    default: return `${t("模型文件状态")}：${state}`;
  }
}

function deviceRuntimeCopy(status: string, t: (key: string) => string): string {
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

function formatBytes(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export function EngineConfigStep({
  asrProvider,
  onAsrProviderChange,
  dashscopeRegion,
  onDashscopeRegionChange,
  onDeviceReady,
  onDeviceModels,
  onDeviceActionId,
  onDeviceActionError,
  onDeviceAction,
  onRefreshOnDeviceModels,
  strictOfflineBlocked,
  asrModel,
  onAsrModelChange,
  language,
  onLanguageChange,
  keyHint,
  configuredAsrKey,
  keyValue,
  onKeyChange,
  valid,
  validating,
  onValidate,
  error,
}: {
  asrProvider: ProviderId;
  onAsrProviderChange: (value: ProviderId) => void;
  dashscopeRegion: DashscopeRegion;
  onDashscopeRegionChange: (value: DashscopeRegion) => void;
  onDeviceReady: boolean;
  onDeviceModels: OnDeviceModelStatus[];
  onDeviceActionId: string | null;
  onDeviceActionError: string | null;
  onDeviceAction: (id: string, action: "download" | "cancel") => void;
  onRefreshOnDeviceModels: () => void;
  strictOfflineBlocked: boolean;
  asrModel: string;
  onAsrModelChange: (value: string) => void;
  language: string;
  onLanguageChange: (value: string) => void;
  keyHint?: string;
  configuredAsrKey: boolean;
  keyValue: string;
  onKeyChange: (value: string) => void;
  valid: string | null;
  validating: boolean;
  onValidate: () => void;
  error?: string | null;
}) {
  const { t } = useI18n();
  const [loadCancelingId, setLoadCancelingId] = useState<string | null>(null);
  const [loadCancelError, setLoadCancelError] = useState<string | null>(null);
  const isValid = valid === "valid";
  const onDevice = asrProvider === "on_device";
  const groq = asrProvider === "groq";
  const soniox = asrProvider === "soniox";
  const assemblyAi = asrProvider === "assemblyai";
  const dashscope = asrProvider === "dashscope";
  const configuredProvider = !groq && !onDevice;
  const provider = providerById(asrProvider);
  const modelOptions = provider?.asrModels ?? [];
  const selectedOnDeviceModel = onDeviceModels.find((model) => model.id === asrModel);
  const modelLoadInProgress = onDeviceModels.some((model) => model.runtime_status === "loading");
  const hasSupportedDeviceModel = onDeviceModels.some((model) => model.id !== "sensevoice-small" && model.platform_supported);
  const providerOptions = Array.from(new Set<ProviderId>([
    ...providersFor("asr").map(({ id }) => id).filter((id) => id !== "custom" && id !== "local_whisper" && id !== "on_device"),
    ...(asrProvider === "custom" || asrProvider === "local_whisper" ? [asrProvider] : []),
    "on_device",
  ]));
  const profile = asrModelProfile(asrProvider, asrModel);

  useEffect(() => {
    if (!modelLoadInProgress) return;
    const timer = window.setInterval(() => void onRefreshOnDeviceModels(), 1000);
    return () => window.clearInterval(timer);
  }, [modelLoadInProgress, onRefreshOnDeviceModels]);

  const cancelModelLoad = async (id: string) => {
    setLoadCancelingId(id);
    setLoadCancelError(null);
    try {
      await invoke<boolean>("cancel_on_device_model_load", { id });
      await onRefreshOnDeviceModels();
    } catch {
      setLoadCancelError(t("本机模型操作失败，请重试。"));
      await onRefreshOnDeviceModels();
    } finally {
      setLoadCancelingId(null);
    }
  };

  return (
    <div>
      <h1 className="text-[28px] font-semibold leading-tight tracking-tight text-primary">
        {t("连接语音服务")}
      </h1>
      <p className="mt-2 text-[13px] leading-6 text-secondary">
        {onDevice
          ? t("选择一个本机 MLX 语音模型。需要 Apple Silicon 与 macOS 14 或更新版本；模型文件需要你手动下载。")
          : soniox
            ? t("Soniox 会在录音期间通过 WebSocket 接收音频；设置时只检查本地凭据，不连接服务。真实连接只在你开始听写时建立。")
            : assemblyAi
              ? t("AssemblyAI 使用固定的 Dictation / Sync 接口。只有你点击测试时才会检查转写服务；保存凭据本身不代表服务可用。")
              : dashscope
                ? t("Qwen Audio 3.1 Message 使用已选地区的 WebSocket 原始转写接口；这不是普通 HTTP flash 接口。")
                : groq
                  ? t("VoiceFlow 使用 Groq 将语音转换成文字。输入访问密钥，验证通过后即可开始。")
                  : t("选择语音转写服务。新服务需要密钥；验证成功后会保存在本机钥匙串。")}
      </p>
      {strictOfflineBlocked && (
        <p role="status" className="mt-2 rounded-lg bg-elevated px-3 py-2 text-xs leading-5 text-warning-ink">
          {t("严格离线模式会阻止云端转写。请使用受支持的本机模型，或在设置中关闭严格离线模式。")}
        </p>
      )}

      <div role="group" aria-label={t("选择首次使用路线")} className="mt-6 flex flex-wrap gap-2">
        <button type="button" aria-pressed={groq} disabled={validating} onClick={() => { if (!groq) onAsrProviderChange("groq"); }} className={`flex-1 rounded-lg border px-4 py-3 text-left outline-none ${focusRingClass} ${groq ? "border-accent bg-elevated" : "border-border hover:bg-elevated"}`}>
          <span className="block text-sm font-medium">{t("推荐云端路线")}</span>
          <span className="mt-1 block text-xs text-secondary">{t("Groq · 填写密钥后验证并试用")}</span>
        </button>
        <button type="button" aria-pressed={onDevice} disabled={validating || (onDeviceModels.length > 0 && !hasSupportedDeviceModel && !onDevice)} onClick={() => { if (!onDevice) onAsrProviderChange("on_device"); }} className={`flex-1 rounded-lg border px-4 py-3 text-left outline-none ${focusRingClass} ${onDevice ? "border-accent bg-elevated" : "border-border hover:bg-elevated"}`}>
          <span className="block text-sm font-medium">{t("本机路线")}</span>
          <span className="mt-1 block text-xs text-secondary">{t("Apple Silicon · 手动下载模型")}</span>
        </button>
      </div>
      {onDevice && <p className="mt-3 text-xs leading-5 text-secondary">{t("首次使用本机路线只做转写，不调用云端整理。之后可在语音服务中配置 AI 整理。")}</p>}

      <fieldset disabled={validating} className="mt-4 border-y border-border">
        <legend className="sr-only">{t("连接语音服务")}</legend>
        <details open={!groq && !onDevice ? true : undefined} className="vf-inline-disclosure py-4">
          <summary className={`cursor-pointer break-words rounded-md text-sm font-medium text-secondary ${focusRingClass}`}>{t("高级配置")}{` · ${provider?.label ?? asrProvider} / ${asrModel}`}</summary>
        <div className="mt-3 flex items-start gap-3">
          <CloudCog {...iconProps} className="mt-0.5 shrink-0 text-secondary" aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <label className="block text-sm font-medium text-primary" htmlFor="onboarding-asr-provider">{t("转写服务")}</label>
            <Select
              id="onboarding-asr-provider"
              aria-label={t("转写服务")}
              value={asrProvider}
              disabled={validating}
              onValueChange={(value) => {
                if (isProviderId(value)) onAsrProviderChange(value);
              }}
              className="mt-2 h-9 w-full rounded-lg border border-border bg-elevated px-3 text-sm text-primary"
            >
              {providerOptions.map((id) => (
                <option
                  key={id}
                  value={id}
                  disabled={id === "on_device" && onDeviceModels.length > 0 && !hasSupportedDeviceModel && asrProvider !== "on_device"}
                >
                  {id === "on_device" ? t("本机模型") : id === "custom" ? t("兼容接口") : providerById(id)?.label ?? id}
                </option>
              ))}
            </Select>
            <p className="mt-1 text-xs leading-relaxed text-secondary">
              {onDevice
                ? t("音频由本机模型处理；文字整理可在语音服务设置中单独选择本机或云端服务。")
                : groq
                  ? <>{t("低延迟")} · {t("适合日常口述；密钥只保存在这台 Mac 上。")}</>
                  : soniox
                    ? t("Soniox 接收完整实时音频流；临时结果不会写入目标 App 或历史记录。")
                    : assemblyAi
                      ? t("AssemblyAI 固定端点；整理开启时符合条件的 120 秒内录音走 Dictation 候选；AI Off、本地-only 或超过 120 秒走 raw Sync。")
                      : dashscope
                        ? t("Qwen Audio 3.1 ASR Flash Message · 原始识别后由 VoiceFlow 独立整理")
                        : `${provider?.label ?? t("服务商")} · ${t("测试音频会发送到当前配置的转写服务")}`}
            </p>
            {onDevice && (!selectedOnDeviceModel
              || (selectedOnDeviceModel.state === "ready"
                && !selectedOnDeviceModel.inference_ready)) && (
              <button
                type="button"
                onClick={onRefreshOnDeviceModels}
                className={`mt-2 ${compactButtonClass}`}
              >{selectedOnDeviceModel ? t("重新检查本机运行时") : t("重新读取模型状态")}</button>
            )}
            <div className="mt-3">
              {dashscope && (
                <label className="mb-3 block text-xs font-medium text-secondary">
                  {t("服务区域")}
                  <Select
                    aria-label={t("服务区域")}
                    value={dashscopeRegion}
                    onValueChange={(value) => {
                      if (value === "beijing" || value === "singapore") {
                        onDashscopeRegionChange(value);
                      }
                    }}
                    className="mt-1 h-9 w-full rounded-lg border border-border bg-elevated px-3 text-sm text-primary"
                  >
                    <option value="beijing">{t("北京")}</option>
                    <option value="singapore">{t("新加坡")}</option>
                  </Select>
                </label>
              )}
                <label htmlFor="onboarding-asr-model" className="block text-xs font-medium text-secondary">{t("ASR 模型")}</label>
              {provider?.asrModelField === "select" && modelOptions.length > 0 ? (
                  <Select
                    id="onboarding-asr-model"
                    aria-label={t("ASR 模型")}
                    value={asrModel}
                    disabled={validating}
                    onValueChange={(value) => onAsrModelChange(value)}
                    className="mt-1 h-9 w-full rounded-lg border border-border bg-elevated px-3 text-sm text-primary"
                  >
                    {modelOptions.map((option) => (
                      <option
                        key={option.value}
                        value={option.value}
                        disabled={Boolean(option.retiredForNewSelection && option.value !== asrModel)
                          || (onDevice
                            && option.value !== "sensevoice-small"
                            && onDeviceModels.find((model) => model.id === option.value)?.platform_supported === false)}
                      >
                        {option.note ? `${option.label} · ${t(option.note)}` : option.label}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <>
                    <Autocomplete
                      id="onboarding-asr-model"
                      aria-label={t("ASR 模型")}
                      value={asrModel}
                      disabled={validating}
                      options={modelOptions}
                      onValueChange={onAsrModelChange}
                      placeholder={provider?.defaultAsrModel || "model"}
                      autoComplete="off"
                      spellCheck={false}
                      className="mt-1 h-9 w-full rounded-lg border border-border bg-elevated px-3 font-mono text-sm text-primary"
                    />

                  </>
                )}
            </div>
            {profile && (
              <div className="mt-2 space-y-1 text-xs leading-relaxed text-tertiary">
                <p>{asrLanguageDescription(profile, language, t)}</p>
                {profile?.requestNote && <p>{t(profile.requestNote)}</p>}
                {profile?.responseNote && <p>{t(profile.responseNote)}</p>}
                {profile?.limitNote && <p>{t(profile.limitNote)}</p>}
                {profile?.contextNote && <p>{t(profile.contextNote)}</p>}
                {profile?.availabilityNote && <p className="text-warning-ink">{t(profile.availabilityNote)}</p>}
                {profile?.capabilityNote && <p>{t(profile.capabilityNote)}</p>}
              </div>
            )}
          </div>
        </div>
        </details>

        {onDevice ? (
          <div className="border-t border-border py-4">
            {profile?.languageSupport === "explicit_language_required" && (
              <label className="mb-4 block text-xs font-medium text-secondary">
                {t("识别语言")}
                <Select
                  aria-label={t("识别语言")}
                  value={language}
                  onValueChange={(value) => onLanguageChange(value)}
                  className="mt-1 h-9 w-full rounded-lg border border-border bg-elevated px-3 text-sm text-primary"
                >
                  <option value="auto" disabled>{t("自动（此模型不支持）")}</option>
                  <option value="zh">{t("中文")}</option>
                  <option value="en">English</option>
                </Select>
              </label>
            )}
            <p className="text-sm font-medium text-primary">{t("本机模型")}</p>
            <p className="mt-1 text-xs leading-relaxed text-secondary">
              {selectedOnDeviceModel
                ? `${deviceStateCopy(selectedOnDeviceModel.state, t)} · ${deviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t)}`
                : t("正在读取本机模型状态…")}
            </p>
            {selectedOnDeviceModel?.state === "downloading" && (
              <p className="mt-1 text-xs text-tertiary">{formatBytes(selectedOnDeviceModel.downloaded_bytes)} / {formatBytes(selectedOnDeviceModel.bytes)}</p>
            )}
            {selectedOnDeviceModel?.error && <p className="mt-1 text-xs text-error-ink">{selectedOnDeviceModel.error}</p>}
            {selectedOnDeviceModel?.id === "sensevoice-small" && <p className="mt-1 text-xs text-warning-ink">{t("SenseVoice 是旧版文件保留项；没有 MLX 推理支持，不能用于本机听写。")}</p>}
            {selectedOnDeviceModel?.runtime_status === "loading"
              ? <p className="mt-1 text-xs text-warning-ink">{t("本机模型正在加载；加载完成后会单独显示运行时状态。")}</p>
              : selectedOnDeviceModel?.loaded
                ? <p className="mt-1 text-xs text-secondary">{t("能力握手通过，模型当前已加载。")}</p>
                : onDeviceReady && <p className="mt-1 text-xs text-secondary">{t("能力握手通过；模型是否已加载会单独显示。")}</p>}
            <p className="mt-1 text-xs leading-relaxed text-tertiary">{t("MLX 模型仅支持 Apple Silicon 和 macOS 14 或更新版本。下载模型需要网络；VoiceFlow 不会自动下载或安装模型。")}</p>
            {onDeviceActionError && <p role="alert" className="mt-2 text-xs text-error-ink">{onDeviceActionError}</p>}
            {loadCancelError && <p role="alert" className="mt-2 text-xs text-error-ink">{loadCancelError}</p>}
            <div className="mt-3 flex flex-wrap gap-2">
              {selectedOnDeviceModel?.runtime_status === "loading" && (
                <button
                  type="button"
                  disabled={loadCancelingId === asrModel}
                  aria-busy={loadCancelingId === asrModel}
                  onClick={() => void cancelModelLoad(asrModel)}
                  className={`${compactButtonClass} w-44 max-w-full`}
                >{loadCancelingId === asrModel ? t("正在取消…") : t("取消模型加载")}</button>
              )}
              {selectedOnDeviceModel?.state === "downloading" ? (
                <button type="button" aria-busy={onDeviceActionId === asrModel} disabled={onDeviceActionId === asrModel} onClick={() => onDeviceAction(asrModel, "cancel")} className={`${compactButtonClass} w-44 max-w-full`}>{t(onDeviceActionId === asrModel ? "正在取消…" : "取消下载")}</button>
              ) : selectedOnDeviceModel?.state !== "ready" ? (
                <button
                  type="button"
                  disabled={onDeviceActionId === asrModel || selectedOnDeviceModel?.platform_supported === false || !selectedOnDeviceModel}
                  aria-busy={onDeviceActionId === asrModel}
                  onClick={() => onDeviceAction(asrModel, "download")}
                  className={`${compactButtonClass} w-44 max-w-full`}
                >{selectedOnDeviceModel?.platform_supported === false ? t("当前设备不支持") : onDeviceActionId === asrModel ? t("准备中…") : selectedOnDeviceModel?.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>
              ) : null}
            </div>
            {onDeviceModels.filter((model) => model.state === "downloading" && model.id !== asrModel).map((model) => (
              <div key={model.id} className="mt-2 flex flex-wrap items-center justify-between gap-2 text-xs text-tertiary">
                <span>{model.label} · {formatBytes(model.downloaded_bytes)} / {formatBytes(model.bytes)}</span>
                <button type="button" disabled={onDeviceActionId === model.id} onClick={() => onDeviceAction(model.id, "cancel")} className={compactButtonClass}>{t("取消下载")}</button>
              </div>
            ))}
          </div>
        ) : groq ? (
          <>
            <div className="border-t border-border py-4">
              <label htmlFor="onboarding-groq-api-key" className="block text-sm font-medium text-primary">{t("Groq API Key（访问密钥）")}</label>
              <p className="mt-0.5 text-xs leading-5 text-secondary"><a className="text-accent underline underline-offset-2" href="https://console.groq.com" target="_blank" rel="noreferrer">{t("在 Groq Console 创建密钥")}</a><span className="mt-1 block text-tertiary">{t("密钥通常以 gsk_ 开头。")}</span></p>
              <PasswordInput id="onboarding-groq-api-key" ariaLabel={t("Groq API Key")} value={keyValue} onChange={onKeyChange} placeholder="gsk_…" valid={isValid} monospace className="mt-2" />
            </div>

            <div className="flex items-center gap-2 pb-4">
              <button type="button" onClick={() => void onValidate()} disabled={(!keyValue.trim() && groq && !configuredAsrKey) || validating} className={`${buttonClass} min-w-[80px]`}>
                {validating ? t("验证中…") : groq ? t("验证 API Key") : t("测试转写服务")}
              </button>
              {groq
                ? <ValidationStatus status={valid} validating={validating} />
                : valid === "valid"
                  ? <span role="status" className="text-xs text-success-ink">{t("服务检查通过")}</span>
                  : valid && <span role="alert" className="text-xs text-error-ink">{t("验证失败，请重试")}</span>}
            </div>
          </>
        ) : configuredProvider ? (
          <div className="border-t border-border py-4">
            <p className="text-sm font-medium text-primary">{provider?.label ?? t("服务商")}</p>
            <p className="mt-1 text-xs leading-relaxed text-secondary">
              {t("在所选服务的控制台创建 API Key，并粘贴在这里。")}
            </p>
            <PasswordInput
              id={`onboarding-${asrProvider}-api-key`}
              ariaLabel={`${provider?.label ?? t("服务商")} API Key`}
              value={keyValue}
              onChange={onKeyChange}
              placeholder={keyHint || `${provider?.label ?? t("服务商")} API Key`}
              monospace
              className="mt-2"
            />
            <div className="mt-3 flex items-center gap-2">
              <button
                type="button"
                onClick={() => void onValidate()}
                disabled={(!keyValue.trim() && !keyHint && !provider?.allowsEmptyKey) || validating}
                className={`${buttonClass} min-w-[80px]`}
              >
                {validating ? t("验证中…") : soniox ? t("确认凭据") : t("测试转写服务")}
              </button>
              {valid === "configured"
                ? <span role="status" className="text-xs text-secondary">{t("凭据已配置；实时连接尚未测试。")}</span>
                : valid === "valid"
                  ? <span role="status" className="text-xs text-success-ink">{t("服务检查通过")}</span>
                  : valid && <span role="alert" className="text-xs text-error-ink">{t("验证失败，请重试")}</span>}
            </div>
          </div>
        ) : null}

        {error && <p role="alert" className="border-t border-error/20 py-3 text-xs text-error-ink">{error}</p>}

        <p className="flex items-center gap-1.5 border-t border-border py-4 text-xs text-tertiary">
          <Lock {...iconPropsSm} className="shrink-0" />
          {onDevice
            ? t("本机 MLX 模型文件保存在这台 Mac；下载是需要网络的独立操作，运行时能力检查通过后才会标记为可用。")
            : groq
              ? t("密钥仅保存在这台 Mac 的钥匙串中；VoiceFlow 不会代存，验证时只发送到 Groq")
              : soniox
                ? t("Soniox 密钥只保存在这台 Mac 的钥匙串中；设置不会连接服务，只有开始听写时才会发送音频。")
                : t("服务商密钥保存在这台 Mac 的钥匙串中；测试音频只发送到当前配置的转写服务。")}
        </p>
      </fieldset>
    </div>
  );
}
