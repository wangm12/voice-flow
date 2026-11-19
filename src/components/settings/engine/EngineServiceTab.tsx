import type { JSX } from "react";
import { Select } from "../../Select";
import { SettingsDisclosure, SettingsGroup, SettingsRow, SettingsStatus } from "../../SettingsLayout";
import { Toggle } from "../../Toggle";
import { useI18n } from "../../../lib/i18n";
import { asrLanguageDescription, type EngineDraft } from "../../../lib/engineWizard";
import { defaultModel, providerById, type ModelOption, type PROVIDERS, type ProviderId } from "../../../lib/providers";
import { compactButtonClass } from "../../../lib/theme";
import type {
  CleanupModelStatus,
  LocalCleanupModelStatus,
  LocalCleanupStatus,
  MlxCleanupModelStatus,
  OnDeviceModelStatus,
  Settings,
} from "../../../types/settings";
import { ModelControl } from "./EngineControls";
import {
  LEGACY_MLX_CLEANUP_MODEL_ID,
  cleanupModelStoreStateCopy,
  controlClass,
  formatModelBytes,
  localCleanupCompatibilityCopy,
  mlxCleanupCompatibilityCopy,
  mlxCleanupModelStateCopy,
  onDeviceRuntimeCopy,
  onDeviceStateCopy,
  providerHasCredentialInput,
  type EnginePage,
} from "./engineHelpers";

export type EngineServiceTabProps = {
  page: EnginePage;
  pendingChanges: boolean;
  probing: boolean;
  rowProbing: ProviderId | null;
  rowProbingStage: "asr" | "cleanup" | null;
  removing: boolean;
  strictOfflineSaving: boolean;
  settings: Settings;
  draft: EngineDraft;
  draftCleanupEnabled: boolean;
  onDraftCleanupEnabledChange: (enabled: boolean) => void;
  currentAsrProvider: ProviderId;
  currentCleanupProvider: ProviderId;
  savedAsrBlocked: boolean;
  savedCleanupBlocked: boolean;
  asrStatus: { label: string; tone: "accent" | "success" | "warning" | "error" | "neutral" };
  cleanupStatus: { label: string; tone: "accent" | "success" | "warning" | "error" | "neutral" | "unused" };
  rowProbeFeedback: { provider: ProviderId; stage?: "asr" | "cleanup"; tone: "success" | "error"; message: string } | null;
  probeSuccess: "tested" | "saved" | "";
  onProbeSuccessClear: () => void;
  onDeviceModels: OnDeviceModelStatus[];
  selectedOnDeviceModel: OnDeviceModelStatus | undefined;
  onDeviceModelsLoading: boolean;
  onDeviceModelReadError: string | null;
  asrProfile: ReturnType<typeof import("../../../lib/providers").asrModelProfile>;
  cloudAsrOptions: readonly (typeof PROVIDERS)[number][];
  localAsrOptions: readonly (typeof PROVIDERS)[number][];
  customAsrOptions: readonly (typeof PROVIDERS)[number][];
  cloudCleanupOptions: readonly (typeof PROVIDERS)[number][];
  localCleanupOptions: readonly (typeof PROVIDERS)[number][];
  customCleanupOptions: readonly (typeof PROVIDERS)[number][];
  renderAsrOption: (provider: (typeof PROVIDERS)[number]) => JSX.Element;
  renderCleanupOption: (provider: (typeof PROVIDERS)[number]) => JSX.Element;
  onChangeAsrProvider: (value: string) => void;
  onChangeAsrModel: (asrModel: string) => void;
  onChangeCleanupProvider: (value: string) => void;
  onChangeCleanupModel: (cleanupModel: string) => void;
  onDeviceAsrModelOptions: readonly ModelOption[];
  localCleanupModelOptions: readonly ModelOption[];
  mlxCleanupModelOptions: readonly ModelOption[];
  onNavigateToPage: (nextPage: EnginePage, targetId?: string) => void;
  onTestConnection: (id: ProviderId, stageScope: "asr" | "cleanup") => void;
  strictOfflineBlocksProbe: (provider: ProviderId, stage: "asr" | "cleanup") => boolean;
  selectedLocalCleanupModel: LocalCleanupModelStatus | undefined;
  localCleanupModelsReadError: string | null;
  localCleanupStatus: LocalCleanupStatus | null;
  localCleanupError: string | null;
  cleanupModelStoreStatus: CleanupModelStatus | null;
  selectedMlxCleanupModel: MlxCleanupModelStatus | undefined;
  selectedMlxCleanupLanguage: string | null;
  mlxCleanupModelsReadError: string | null;
  mlxCleanupManageTarget: string;
  customParametersOk: boolean;
  draftLocalCleanupModelReady: boolean;
};

export function EngineServiceTab({
  page,
  pendingChanges,
  probing,
  rowProbing,
  rowProbingStage,
  removing,
  strictOfflineSaving,
  settings,
  draft,
  draftCleanupEnabled,
  onDraftCleanupEnabledChange,
  currentAsrProvider,
  currentCleanupProvider,
  savedAsrBlocked,
  savedCleanupBlocked,
  asrStatus,
  cleanupStatus,
  rowProbeFeedback,
  probeSuccess,
  onProbeSuccessClear,
  selectedOnDeviceModel,
  onDeviceModels,
  onDeviceModelsLoading,
  onDeviceModelReadError,
  asrProfile,
  cloudAsrOptions,
  localAsrOptions,
  customAsrOptions,
  cloudCleanupOptions,
  localCleanupOptions,
  customCleanupOptions,
  renderAsrOption,
  renderCleanupOption,
  onChangeAsrProvider,
  onChangeAsrModel,
  onChangeCleanupProvider,
  onChangeCleanupModel,
  onDeviceAsrModelOptions,
  localCleanupModelOptions,
  mlxCleanupModelOptions,
  onNavigateToPage,
  onTestConnection,
  strictOfflineBlocksProbe,
  selectedLocalCleanupModel,
  localCleanupModelsReadError,
  localCleanupStatus,
  localCleanupError,
  cleanupModelStoreStatus,
  selectedMlxCleanupModel,
  selectedMlxCleanupLanguage,
  mlxCleanupModelsReadError,
  mlxCleanupManageTarget,
  customParametersOk,
  draftLocalCleanupModelReady,
}: EngineServiceTabProps) {
  const { t } = useI18n();
  return (
    <div
      id="engine-panel-service"
      role="region"
      aria-label={t("服务配置")}
      hidden={page !== "service"}
      inert={page !== "service"}
      className="vf-engine-page-content"
    >
      <SettingsGroup title={t("当前路线与草稿")} description={t(pendingChanges ? "有待应用的更改；当前听写仍使用已保存路线。" : "调整服务或模型后，可分别测试，再保存并使用。")}>
        <fieldset aria-label={t("当前生效路线")} disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="w-full min-w-0 border-0 p-0">
          <div className="divide-y divide-border">
            <div className="vf-service-asr-section">
              <SettingsRow
                className="vf-service-route-row"
                title={t("转写")}
                description={(
                  <div className="space-y-1">
                    <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <SettingsStatus label={asrStatus.label} tone={asrStatus.tone} />
                      <span className="vf-service-route-current">{t(savedAsrBlocked ? "已保存，当前被阻止" : "当前使用")} · {providerById(currentAsrProvider)?.label} · {settings.asr_model || defaultModel(currentAsrProvider, "asr")}</span>
                    </p>
                    {savedAsrBlocked && <p className="text-warning-ink">{t("严格离线模式已阻止此路线")}</p>}
                    {(draft.asrProvider !== currentAsrProvider || draft.asrModel !== (settings.asr_model || defaultModel(currentAsrProvider, "asr"))) && (
                      <p className="text-warning-ink">{t("待应用")} · {providerById(draft.asrProvider)?.label} · {draft.asrModel}</p>
                    )}
                    {rowProbeFeedback?.provider === draft.asrProvider && rowProbeFeedback.stage === "asr" && (
                      <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}>
                        {rowProbeFeedback.message}
                      </p>
                    )}
                    {draft.asrProvider === "soniox" && <p className="text-tertiary">{t("实时连接在听写时建立；API Key 通过下方共用操作保存。")}</p>}
                    {draft.asrProvider === "local_whisper" && <p className="text-tertiary">{t("本机 Whisper 走本机 HTTP；先自行运行 whisper.cpp 或 speaches。")}</p>}
                    {draft.asrProvider === "on_device" && (
                      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <span role={onDeviceModelReadError ? "alert" : "status"} className={onDeviceModelReadError ? "text-warning-ink" : "text-secondary"}>
                          {selectedOnDeviceModel
                            ? `${onDeviceStateCopy(selectedOnDeviceModel.state, t)} · ${onDeviceRuntimeCopy(selectedOnDeviceModel.runtime_status, t)}`
                            : onDeviceModelsLoading
                              ? t("正在读取本机模型状态…")
                              : onDeviceModelReadError ?? t("本机模型状态尚未读取")}
                        </span>
                        {selectedOnDeviceModel?.error_category === "disk_full" && <span className="text-error-ink">{t("检查可用空间后重试下载。")}</span>}
                        {selectedOnDeviceModel?.error && <span className="text-tertiary">{selectedOnDeviceModel.error}</span>}
                      </div>
                    )}
                    {asrProfile?.availabilityNote && <p className="text-warning-ink">{t(asrProfile.availabilityNote)}</p>}
                  </div>
                )}
              >
                <div className="vf-service-route-controls">
                  <div className="vf-service-fields">
                    <Select id="engine-asr-provider" aria-label={t("转写服务")} value={draft.asrProvider} onValueChange={onChangeAsrProvider} className={`${controlClass} w-40 max-w-full`}>
                      <optgroup label={t("云端服务")}>{cloudAsrOptions.map(renderAsrOption)}</optgroup>
                      <optgroup label={t("本机服务")}>{localAsrOptions.map(renderAsrOption)}</optgroup>
                      <optgroup label={t("自定义服务")}>{customAsrOptions.map(renderAsrOption)}</optgroup>
                    </Select>
                    <ModelControl
                      id={draft.asrProvider}
                      side="asr"
                      value={draft.asrModel}
                      ariaLabel={t("ASR 模型")}
                      onChange={onChangeAsrModel}
                      optionsOverride={draft.asrProvider === "on_device" ? onDeviceAsrModelOptions : undefined}
                      deviceModels={draft.asrProvider === "on_device" ? onDeviceModels : undefined}
                    />
                  </div>
                  <div className="vf-service-route-actions">
                    {providerHasCredentialInput(draft.asrProvider) && (
                      <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("api_key", `provider-key-${draft.asrProvider}`)}>{t("管理凭据")}</button>
                    )}
                    {draft.asrProvider === "on_device" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("local_models", selectedOnDeviceModel ? `local-asr-model-${selectedOnDeviceModel.id}` : "local-asr-models")}>{t("管理模型")}</button>}
                    {draft.asrProvider === "custom" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>}
                    {draft.asrProvider === "local_whisper" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("advanced", "advanced-local-whisper-url")}>{t("管理连接地址")}</button>}
                    {savedAsrBlocked && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("service", "engine-asr-provider")}>{t("修改路线")}</button>}
                    {draft.asrProvider !== "soniox" && (
                      <button
                        type="button"
                        onClick={() => onTestConnection(draft.asrProvider, "asr")}
                        aria-busy={rowProbing === draft.asrProvider && rowProbingStage === "asr"}
                        disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(draft.asrProvider, "asr")}
                        className={compactButtonClass}
                      >
                        {rowProbing === draft.asrProvider && rowProbingStage === "asr" ? t("测试中…") : t("测试转写")}
                      </button>
                    )}
                  </div>
                </div>
              </SettingsRow>
              {draft.asrProvider !== "on_device" && (
                <div className="vf-service-asr-guide px-5">
                  <SettingsDisclosure variant="plain" title={t("模型使用说明")}>
                    <div role="note" className="mt-3 space-y-1 rounded-lg bg-elevated px-3 py-2.5 text-xs leading-5 text-tertiary">
                      <p>{asrLanguageDescription(asrProfile, settings.language, t)}</p>
                      {asrProfile?.requestNote && <p>{t(asrProfile.requestNote)}</p>}
                      {asrProfile?.responseNote && <p>{t(asrProfile.responseNote)}</p>}
                      {asrProfile?.limitNote && <p>{t(asrProfile.limitNote)}</p>}
                      {asrProfile?.contextNote && <p>{t(asrProfile.contextNote)}</p>}
                      {asrProfile?.capabilityNote && <p>{t(asrProfile.capabilityNote)}</p>}
                      <p>{t("转写请求会发送到当前配置的服务；不会静默切换到其他服务。")}</p>
                    </div>
                  </SettingsDisclosure>
                </div>
              )}
            </div>
            <SettingsRow
              className="vf-service-route-row"
              title={t("AI 文字整理")}
              description={(
                <div className="space-y-1">
                  <p className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <SettingsStatus label={cleanupStatus.label} tone={cleanupStatus.tone} />
                    <span className="vf-service-route-current">{t(savedCleanupBlocked ? "已保存，当前被阻止" : "当前使用")} · {settings.cleanup_enabled
                      ? `${providerById(currentCleanupProvider)?.label} · ${settings.cleanup_model || defaultModel(currentCleanupProvider, "llm")}`
                      : t("关闭 · 只用本地规则")}</span>
                  </p>
                  {savedCleanupBlocked && <p className="text-warning-ink">{t("严格离线模式已阻止此路线")}</p>}
                  {draftCleanupEnabled !== settings.cleanup_enabled || (draftCleanupEnabled && (draft.cleanupProvider !== currentCleanupProvider || draft.cleanupModel !== (settings.cleanup_model || defaultModel(currentCleanupProvider, "llm")))) ? (
                    <p className="text-warning-ink">{t("待应用")} · {draftCleanupEnabled ? `${providerById(draft.cleanupProvider)?.label} · ${draft.cleanupModel}` : t("关闭 · 只用本地规则")}</p>
                  ) : null}
                  {draftCleanupEnabled && rowProbeFeedback?.provider === draft.cleanupProvider && rowProbeFeedback.stage === "cleanup" && (
                    <p role={rowProbeFeedback.tone === "error" ? "alert" : "status"} className={rowProbeFeedback.tone === "error" ? "text-error-ink" : "text-success-ink"}>
                      {rowProbeFeedback.message}
                    </p>
                  )}
                  {draftCleanupEnabled && draft.cleanupProvider === "on_device_llama_cpp" && (
                    <div className="space-y-1 text-tertiary">
                      <p>{t("只列出已下载、校验且兼容本机的模型；下载不会自动切换当前使用模型。")}</p>
                      {selectedLocalCleanupModel?.compatible === false && <p role="note" className="text-warning-ink">{localCleanupCompatibilityCopy(selectedLocalCleanupModel.compatibility_error, t)}</p>}
                      {localCleanupModelsReadError && <p role="alert" className="text-error-ink">{localCleanupModelsReadError}</p>}
                    </div>
                  )}
                  {draft.asrProvider === "assemblyai" && (
                    <div role="note" className="space-y-1 rounded-lg bg-elevated px-3 py-2.5 text-tertiary">
                      <p>{t("启用 AI 整理且满足条件的 120 秒内 Dictation 会提供 AssemblyAI 整理候选；不需要共同整理密钥，候选通过本地保护检查后才会采用。")}</p>
                      <p>{t("候选缺失或失败时，只有已选共同整理服务的密钥已配置才会再请求一次整理；否则保留原始转写并使用本地保护回退。保护检查拒绝候选时也保留原稿，不再发起第二次整理。")}</p>
                      <p>{t("AI Off、本地-only 和超过 120 秒的录音使用 AssemblyAI Sync 原始转写。长录音开启云端整理时需要已选的共同整理服务；服务不可用时保留原文并回退到本地或原始结果。保存密钥不代表服务访问已验证。")}</p>
                    </div>
                  )}
                  {draft.cleanupProvider === "ollama" && (
                    <div className="space-y-1 text-tertiary">
                      <p className="font-medium text-primary">{t("本机整理依赖")}: Ollama · qwen3.5:4b</p>
                      {localCleanupStatus && <p role="status" className={localCleanupStatus.available ? "text-success-ink" : "text-warning-ink"}>{localCleanupStatus.available ? t("本机整理服务可用") : t("本机整理服务不可用")} · {localCleanupStatus.status}</p>}
                      {localCleanupError && <p role="alert" className="text-error-ink">{localCleanupError}</p>}
                    </div>
                  )}
                  {draft.cleanupProvider === "on_device_mlx" && (
                    <div className="space-y-1 text-tertiary">
                      <p>{t("只列出可用于服务配置的本机 MLX 模型；下载不会自动切换当前模型。")}</p>
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-secondary">{draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID
                          ? cleanupModelStoreStatus ? cleanupModelStoreStateCopy(cleanupModelStoreStatus.state, t) : t("本机模型状态尚未读取")
                          : selectedMlxCleanupModel ? mlxCleanupModelStateCopy(selectedMlxCleanupModel, t) : t("本机模型状态尚未读取")}</span>
                        {draft.cleanupModel === LEGACY_MLX_CLEANUP_MODEL_ID && cleanupModelStoreStatus?.operation && cleanupModelStoreStatus.operation !== "delete"
                          ? <span>{formatModelBytes(cleanupModelStoreStatus.bytes_done)} / {formatModelBytes(cleanupModelStoreStatus.bytes_total)}</span>
                          : null}
                        {selectedMlxCleanupLanguage && <span>{t("语言与模式")}: {t(selectedMlxCleanupLanguage)}</span>}
                      </div>
                      {selectedMlxCleanupModel?.compatible === false && <p role="note" className="text-warning-ink">{mlxCleanupCompatibilityCopy(selectedMlxCleanupModel.compatibility_error, t)}</p>}
                      {mlxCleanupModelsReadError && <p role="alert" className="text-error-ink">{mlxCleanupModelsReadError}</p>}
                    </div>
                  )}
                </div>
              )}
            >
              <div className="vf-service-route-controls">
                <div className="vf-service-cleanup-toggle">
                  <Toggle id="engine-cleanup-toggle" checked={draftCleanupEnabled} onChange={(checked) => { onDraftCleanupEnabledChange(checked); onProbeSuccessClear(); }} label={t("AI 文字整理")} />
                </div>
                {draftCleanupEnabled && (
                  <div className="vf-service-fields">
                    <Select id="engine-cleanup-provider" aria-label={t("整理服务")} value={draft.cleanupProvider} onValueChange={onChangeCleanupProvider} className={`${controlClass} w-40 max-w-full`}>
                      <optgroup label={t("云端服务")}>{cloudCleanupOptions.map(renderCleanupOption)}</optgroup>
                      <optgroup label={t("本机服务")}>{localCleanupOptions.map(renderCleanupOption)}</optgroup>
                      <optgroup label={t("自定义服务")}>{customCleanupOptions.map(renderCleanupOption)}</optgroup>
                    </Select>
                    <ModelControl
                      id={draft.cleanupProvider}
                      side="llm"
                      value={draft.cleanupModel}
                      ariaLabel={t("AI 文字整理模型")}
                      onChange={onChangeCleanupModel}
                      optionsOverride={draft.cleanupProvider === "on_device_llama_cpp"
                        ? localCleanupModelOptions
                        : draft.cleanupProvider === "on_device_mlx" ? mlxCleanupModelOptions : undefined}
                    />
                  </div>
                )}
                <div className="vf-service-route-actions">
                  {draftCleanupEnabled && providerHasCredentialInput(draft.cleanupProvider) && (
                    <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("api_key", `provider-key-${draft.cleanupProvider}`)}>{t("管理凭据")}</button>
                  )}
                  {draft.cleanupProvider === "on_device_llama_cpp" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("local_models", "local-cleanup-models")}>{t("管理模型")}</button>}
                  {draft.cleanupProvider === "on_device_mlx" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("local_models", mlxCleanupManageTarget)}>{t("管理模型")}</button>}
                  {draft.cleanupProvider === "ollama" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("advanced", "advanced-ollama-url")}>{t("管理本机连接")}</button>}
                  {draft.cleanupProvider === "custom" && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("advanced", "advanced-custom-base-url")}>{t("管理连接地址")}</button>}
                  {savedCleanupBlocked && <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("service", "engine-cleanup-provider")}>{t("修改路线")}</button>}
                  {draftCleanupEnabled && (
                    <button
                      type="button"
                      onClick={() => onTestConnection(draft.cleanupProvider, "cleanup")}
                      aria-busy={rowProbing === draft.cleanupProvider && rowProbingStage === "cleanup"}
                      disabled={rowProbing !== null || probing || removing || strictOfflineSaving || strictOfflineBlocksProbe(draft.cleanupProvider, "cleanup") || (draft.cleanupProvider === "custom" && !customParametersOk) || (draft.cleanupProvider === "on_device_llama_cpp" && !draftLocalCleanupModelReady)}
                      className={compactButtonClass}
                    >
                      {rowProbing === draft.cleanupProvider && rowProbingStage === "cleanup" ? t("测试中…") : t("测试整理")}
                    </button>
                  )}
                </div>
              </div>
            </SettingsRow>
          </div>
        </fieldset>
        {probeSuccess === "saved" && <p role="status" className="px-5 py-3 text-xs leading-5 text-success-ink">{t("设置已保存")}</p>}
      </SettingsGroup>
    </div>
  );
}
