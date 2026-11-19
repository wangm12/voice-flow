import { SettingsGroup, SettingsRow, SettingsStatus } from "../../SettingsLayout";
import { useI18n } from "../../../lib/i18n";
import { type EngineDraft } from "../../../lib/engineWizard";
import { isRetiredOnDeviceModel, onDeviceMemoryCompatibilityCopy, type ProviderId } from "../../../lib/providers";
import { compactButtonClass, compactDangerButtonClass, focusRingClass, ghostButtonClass } from "../../../lib/theme";
import type {
  CleanupModelStatus,
  DeviceMemoryBudget,
  LocalCleanupModelStatus,
  MlxCleanupModelStatus,
  OnDeviceModelStatus,
  Settings,
} from "../../../types/settings";
import {
  cleanupModelStoreStateCopy,
  cleanupModelStoreSummaryCopy,
  formatModelBytes,
  isCancellationError,
  isMlxCleanupModelOperationActive,
  isSelectableLocalCleanupModel,
  llamaCleanupSpeedCharacteristic,
  localCleanupCompatibilityCopy,
  localCleanupModelStateCopy,
  mlxCleanupCompatibilityCopy,
  mlxCleanupLanguageCopy,
  mlxCleanupModelStateCopy,
  mlxCleanupSpeedCharacteristic,
  onDeviceFileSummaryCopy,
  onDeviceRuntimeCopy,
  onDeviceSpeedCharacteristic,
  type EnginePage,
} from "./engineHelpers";

export type LocalModelsTabProps = {
  page: EnginePage;
  probing: boolean;
  rowProbing: ProviderId | null;
  removing: boolean;
  strictOfflineSaving: boolean;
  settings: Settings;
  draft: EngineDraft;
  draftCleanupEnabled: boolean;
  currentAsrProvider: ProviderId;
  currentCleanupProvider: ProviderId;
  savedAsrModel: string;
  savedCleanupModel: string;
  deviceMemoryBudget: DeviceMemoryBudget | null;
  onDeviceModels: OnDeviceModelStatus[];
  onDeviceModelsLoading: boolean;
  onDeviceModelReadError: string | null;
  onDeviceActionId: string | null;
  onDeviceActionError: { id: string; message: string } | null;
  legacyModelsOpen: boolean;
  onLegacyModelsOpenChange: (open: boolean) => void;
  legacyModel: OnDeviceModelStatus | undefined;
  legacyModelSavedSelected: boolean;
  legacyModelDraftSelected: boolean;
  legacyModelShouldOpen: boolean;
  onRefreshOnDeviceModels: () => void;
  onRunOnDeviceAction: (id: string, action: "download" | "cancel" | "cancel_load" | "delete") => void;
  onCommitOnDeviceModel: (model: OnDeviceModelStatus) => void;
  onRequestDeleteModel: (id: string) => void;
  onDeviceModelActivationBlock: (model: OnDeviceModelStatus) => string | null;
  visibleMlxCleanupModels: MlxCleanupModelStatus[];
  mlxCleanupModelsLoading: boolean;
  mlxCleanupModelsReadError: string | null;
  mlxCleanupModelBusy: Record<string, string>;
  mlxCleanupModelActionErrors: Record<string, string>;
  onRefreshMlxCleanupModels: (showLoading?: boolean) => void;
  onRunMlxCleanupModelAction: (id: string, action: "download" | "verify" | "cancel" | "delete") => void;
  onCommitMlxCleanupModel: (model: MlxCleanupModelStatus) => void;
  onRequestDeleteMlxCleanupModel: (id: string) => void;
  cleanupModelStoreStatus: CleanupModelStatus | null;
  cleanupModelStoreBusy: boolean;
  cleanupModelStoreReadError: string | null;
  cleanupModelStoreActionError: string | null;
  cleanupMlxPlatformSupported: boolean;
  cleanupModelStoreIdle: boolean;
  cleanupModelStoreNeedsRefresh: boolean;
  legacyMlxActivationBlock: (status: CleanupModelStatus) => string | null;
  onRunCleanupModelStoreAction: (action: "import" | "download" | "cancel" | "delete") => void;
  onRefreshCleanupModelStore: () => void;
  onCommitLegacyMlxModel: (status: CleanupModelStatus) => void;
  onRequestDeleteCleanupModel: () => void;
  localCleanupManagementRequired: boolean;
  visibleLocalCleanupModels: LocalCleanupModelStatus[];
  localCleanupModelsLoading: boolean;
  localCleanupModelsReadError: string | null;
  localCleanupModelBusy: Record<string, string>;
  localCleanupModelActionErrors: Record<string, string>;
  onRefreshLocalCleanupModels: (showLoading?: boolean) => void;
  onRunLocalCleanupModelAction: (id: string, action: "download" | "cancel" | "delete") => void;
  onCommitLocalCleanupModel: (model: LocalCleanupModelStatus) => void;
  onRequestDeleteLocalCleanupModel: (id: string) => void;
  cleanupActivationBlock: () => string | null;
};

export function LocalModelsTab({
  page,
  probing,
  rowProbing,
  removing,
  strictOfflineSaving,
  settings,
  draft,
  draftCleanupEnabled,
  currentAsrProvider,
  currentCleanupProvider,
  savedAsrModel,
  savedCleanupModel,
  deviceMemoryBudget,
  onDeviceModels,
  onDeviceModelsLoading,
  onDeviceModelReadError,
  onDeviceActionId,
  onDeviceActionError,
  legacyModelsOpen,
  onLegacyModelsOpenChange,
  legacyModel,
  legacyModelSavedSelected,
  legacyModelDraftSelected,
  legacyModelShouldOpen,
  onRefreshOnDeviceModels,
  onRunOnDeviceAction,
  onCommitOnDeviceModel,
  onRequestDeleteModel,
  onDeviceModelActivationBlock,
  visibleMlxCleanupModels,
  mlxCleanupModelsLoading,
  mlxCleanupModelsReadError,
  mlxCleanupModelBusy,
  mlxCleanupModelActionErrors,
  onRefreshMlxCleanupModels,
  onRunMlxCleanupModelAction,
  onCommitMlxCleanupModel,
  onRequestDeleteMlxCleanupModel,
  cleanupModelStoreStatus,
  cleanupModelStoreBusy,
  cleanupModelStoreReadError,
  cleanupModelStoreActionError,
  cleanupMlxPlatformSupported,
  cleanupModelStoreIdle,
  cleanupModelStoreNeedsRefresh,
  legacyMlxActivationBlock,
  onRunCleanupModelStoreAction,
  onRefreshCleanupModelStore,
  onCommitLegacyMlxModel,
  onRequestDeleteCleanupModel,
  localCleanupManagementRequired,
  visibleLocalCleanupModels,
  localCleanupModelsLoading,
  localCleanupModelsReadError,
  localCleanupModelBusy,
  localCleanupModelActionErrors,
  onRefreshLocalCleanupModels,
  onRunLocalCleanupModelAction,
  onCommitLocalCleanupModel,
  onRequestDeleteLocalCleanupModel,
  cleanupActivationBlock,
}: LocalModelsTabProps) {
  const { t } = useI18n();

  const renderOnDeviceModelRow = (model: OnDeviceModelStatus, legacy = false) => {
    const downloading = model.state === "downloading";
    const unsupported = !legacy && (!model.platform_supported || model.runtime_status === "unsupported_platform");
    const actionBusy = onDeviceActionId === model.id;
    const savedSelected = currentAsrProvider === "on_device" && savedAsrModel === model.id;
    const draftSelected = draft.asrProvider === "on_device" && draft.asrModel === model.id && !savedSelected;
    const downloadCancelled = model.error?.trim().toLowerCase() === "download cancelled";
    const downloadAvailable = model.download_available === true && !isRetiredOnDeviceModel(model.id);
    const retired = !downloadAvailable || isRetiredOnDeviceModel(model.id);
    const memoryCompatibilityIssue = onDeviceMemoryCompatibilityCopy(model.memory_compatibility_error, t);
    const activationBlock = legacy || retired
      ? t("旧版或停用模型只能管理现有文件，不能设为当前使用模型。")
      : onDeviceModelActivationBlock(model);
    const canDownloadOnThisDevice = downloadAvailable && !unsupported && !memoryCompatibilityIssue;
    const runtimeLabel = model.runtime_status === "loading"
      ? t("加载中")
      : model.loaded || model.runtime_status === "loaded"
        ? t("已加载")
        : null;
    const runtimeIssue = legacy
      ? t("SenseVoice 是旧版文件保留项；没有 MLX 推理支持，不能用于本机听写。")
      : !model.platform_supported || model.runtime_status === "unsupported_platform"
        ? t("需要 Apple Silicon 与 macOS 14 或更新版本")
        : model.runtime_status === "sidecar_missing"
          ? t("本机推理组件未安装")
          : model.runtime_status === "runtime_failed"
            ? t("本机运行时启动失败")
            : null;
    const showRuntimeIssue = legacy || unsupported || Boolean(runtimeIssue && (savedSelected || draftSelected));
    const detailId = `local-asr-details-${model.id}`;
    const summary = [
      model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      onDeviceFileSummaryCopy(model.state, t),
      runtimeLabel,
      downloadCancelled ? t("已取消") : null,
    ].filter(Boolean).join(" · ");

    return (
      <div id={`local-asr-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{summary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {model.id === "qwen3-asr-0.6b" && <SettingsStatus label={t("默认选项")} tone="neutral" />}
                {canDownloadOnThisDevice && <SettingsStatus label={t("可下载")} tone="neutral" />}
                {!downloadAvailable && (savedSelected || draftSelected) && <SettingsStatus label={t("旧配置保留")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("本机运行时状态")}: {onDeviceRuntimeCopy(model.runtime_status, t)}</p>
                    <p>{t("运行要求")}: {t("Apple Silicon 与 macOS 14 或更新版本")}</p>
                    <p>{t("路线")}: {t("在本机使用 MLX 推理")}</p>
                    {model.revision && <p className="break-all">{t("模型版本")}: {model.revision}</p>}
                    {model.file_count > 0 && <p>{t("模型文件数")}: {model.file_count}</p>}
                    {model.sha256 && <p className="break-all">SHA-256: {model.sha256}</p>}
                  </div>
                </details>
              </div>
              {downloading && model.bytes > 0 && (
                <progress
                  aria-label={t("模型下载进度")}
                  className="h-1.5 w-full accent-accent"
                  max={model.bytes}
                  value={Math.min(model.bytes, model.downloaded_bytes)}
                />
              )}
              {showRuntimeIssue && runtimeIssue && <p role={legacy || retired || unsupported ? "note" : undefined} className="text-warning-ink">{runtimeIssue}</p>}
              {memoryCompatibilityIssue && <p role="note" className="text-warning-ink">{memoryCompatibilityIssue}</p>}
              {!legacy && (model.license_unreviewed || model.id === "qwen3-asr-1.7b") && (
                <p role="note" className="text-xs leading-5 text-warning-ink">{t("转换权重仓库许可尚未完成审查，详见第三方许可说明。")}</p>
              )}
              <p className="text-xs leading-5 text-secondary">
                {t("适用语言")}: {model.language_note ? t(model.language_note) : t("语言覆盖尚未核实。")}
              </p>
              <p className="text-xs leading-5 text-secondary">
                {t("内存占用估算")}: {model.memory_estimate?.source === "admission_reserve"
                  ? `${formatModelBytes(model.memory_estimate.bytes)} · ${t("模型加载准入预留；估算值，不是实测峰值")}`
                  : t("此模型尚无可用的内存预留估算")}
              </p>
              <p className="text-xs leading-5 text-secondary">{t("速度特点")}: {onDeviceSpeedCharacteristic(model.id, model.label, t)}</p>
              <p className="text-xs leading-5 text-tertiary">{t("此 Mac 上的模型速度尚未实测。")}</p>
              {model.state === "ready" && !savedSelected && activationBlock && (
                <p role="note" className="text-xs leading-5 text-warning-ink">{activationBlock}</p>
              )}
              {model.id === "cohere-transcribe-2b" && <p role="note" className="text-warning-ink">{t("需固定选择中文或 English；不支持自动语言识别。")}</p>}
              {model.error_category === "disk_full" && <p className="text-error-ink">{t("检查可用空间后重试下载。")}</p>}
              {model.error && !downloadCancelled && <p role="alert" className="break-words text-error-ink">{model.error}</p>}
              {onDeviceActionError?.id === model.id && <p role="alert" className="text-error-ink">{onDeviceActionError.message}</p>}
            </div>
          )}
        >
          {model.state === "ready" && !savedSelected && !legacy && (
            <button
              type="button"
              disabled={Boolean(activationBlock) || probing || rowProbing !== null || removing || strictOfflineSaving || actionBusy}
              onClick={() => onCommitOnDeviceModel(model)}
              className={compactButtonClass}
            >{t("设为当前使用模型")}</button>
          )}
          {downloading ? <button type="button" disabled={actionBusy} onClick={() => onRunOnDeviceAction(model.id, "cancel")} className={compactButtonClass}>{t("取消下载")}</button>
            : model.runtime_status === "loading" ? <button type="button" disabled={actionBusy} onClick={() => onRunOnDeviceAction(model.id, "cancel_load")} className={compactButtonClass}>{actionBusy ? t("正在取消…") : t("取消模型加载")}</button>
              : model.state === "ready" || (retired && model.state !== "missing") ? <button type="button" aria-label={`${t("删除模型")}: ${model.label}`} disabled={actionBusy} onClick={() => onRequestDeleteModel(model.id)} className={compactDangerButtonClass}>{t("删除模型")}</button>
                : retired ? null
                  : <button type="button" disabled={actionBusy || unsupported || Boolean(memoryCompatibilityIssue)} onClick={() => onRunOnDeviceAction(model.id, "download")} className={compactButtonClass}>{unsupported ? t("当前设备不支持") : actionBusy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
        </SettingsRow>
      </div>
    );
  };

  const renderLocalCleanupModelRow = (model: LocalCleanupModelStatus) => {
    const savedSelected = settings.cleanup_enabled
      && currentCleanupProvider === "on_device_llama_cpp"
      && savedCleanupModel === model.id;
    const draftSelected = draftCleanupEnabled
      && draft.cleanupProvider === "on_device_llama_cpp"
      && draft.cleanupModel === model.id
      && !savedSelected;
    const selected = savedSelected || draftSelected;
    const busy = Boolean(localCleanupModelBusy[model.id]);
    const downloading = model.operation === "download" || model.state === "downloading";
    const deleting = model.operation === "delete" || model.state === "deleting";
    const detailId = `local-cleanup-details-${model.id}`;
    const transferBytes = model.bytes_total > 0 ? model.bytes_total : model.bytes;
    const partialOnly = !model.managed && Boolean(model.partial_managed);
    const statusSummary = [
      partialOnly && !downloading
        ? `${formatModelBytes(model.bytes_done)} / ${formatModelBytes(transferBytes)}`
        : model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      deleting ? t("正在删除") : localCleanupModelStateCopy(model.state, model.verified, t, model.partial_managed),
      !model.operation && /cancel/i.test(`${model.error ?? ""} ${model.last_operation_error ?? ""}`) ? t("已取消") : null,
    ].filter(Boolean).join(" · ");
    const operationErrors = [...new Set([model.error, model.last_operation_error].filter((error): error is string => Boolean(error?.trim())))];
    const canOfferDownload = model.download_available === true
      && (model.state === "missing" || model.state === "corrupt");
    const canActivate = isSelectableLocalCleanupModel(model) && !savedSelected;

    return (
      <div id={`local-cleanup-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row={model.id} className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{statusSummary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {isSelectableLocalCleanupModel(model) && <SettingsStatus label={t("可用于服务配置")} tone="success" />}
                {!model.compatible && <SettingsStatus label={t("当前设备不兼容")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationErrors.length > 0 || !model.compatible ? t("查看技术信息与错误") : t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("用途")}: {t("通用文字整理")}</p>
                    <p>{t("运行时")}: {t("由 VoiceFlow 随包提供的 llama.cpp")}</p>
                    <p>{t("最低系统与内存要求")}: {t("该模型最低支持档尚待实机验收；兼容状态按本机检查结果显示。")}</p>
                    <p>{t("目标验收档")}: {t("Apple M1 · 16 GB；待实机验证")}</p>
                    <p>{t("文件大小")}: {formatModelBytes(model.bytes)}</p>
                    <p>{t("文件状态")}: {localCleanupModelStateCopy(model.state, model.verified, t, model.partial_managed)}</p>
                    <p>{t("文件校验")}: {model.verified ? t("已通过") : t("尚未通过")}</p>
                    <p>{t("设备兼容性")}: {model.compatible ? t("兼容检查通过") : localCleanupCompatibilityCopy(model.compatibility_error, t)}</p>
                    <p>{t("来源仓库")}: {model.repository}</p>
                    <p className="break-all">{t("固定版本")}: {model.revision}</p>
                    {model.sha256 && <p className="break-all">SHA-256: {model.sha256}</p>}
                    {model.compatibility_error && <p>{t("兼容性错误代码")}: {model.compatibility_error}</p>}
                    {operationErrors.map((error) => <p key={error} className="break-words text-error-ink">{error}</p>)}
                  </div>
                </details>
              </div>
              <p className="text-xs leading-5 text-secondary">{t("适用语言")}: {model.language ? t(model.language) : t("语言覆盖尚未核实。")}</p>
              <p className="text-xs leading-5 text-secondary">
                {t("内存占用估算")}: {model.memory_estimate?.source === "admission_reserve"
                  ? `${formatModelBytes(model.memory_estimate.bytes)} · ${t("模型加载准入预留；估算值，不是实测峰值")}`
                  : t("此模型尚无可用的内存预留估算")}
              </p>
              <p className="text-xs leading-5 text-secondary">{t("速度特点")}: {llamaCleanupSpeedCharacteristic(model.label, t)}</p>
              <p className="text-xs leading-5 text-tertiary">{t("此 Mac 上的模型速度尚未实测。")}</p>
              {downloading && transferBytes > 0 && (
                <div className="space-y-1">
                  <p role="status" className="text-xs text-tertiary">{formatModelBytes(model.bytes_done)} / {formatModelBytes(transferBytes)}</p>
                  <progress
                    aria-label={`${t("模型下载进度")}: ${model.label}`}
                    className="h-1.5 w-full accent-accent"
                    max={transferBytes}
                    value={Math.min(transferBytes, model.bytes_done)}
                  />
                </div>
              )}
              {partialOnly && !downloading && <p role="note" className="text-secondary">{t(model.download_available ? "下载已中断；可以重新下载或清理部分文件。" : "此版本不再提供重新下载；可以清理未完成文件。")}</p>}
              {!model.compatible && <p role="note" className="text-warning-ink">{localCleanupCompatibilityCopy(model.compatibility_error, t)}</p>}
              {canActivate && cleanupActivationBlock() && <p role="note" className="text-warning-ink">{cleanupActivationBlock()}</p>}
              {operationErrors.map((error) => <p key={error} role="alert" className="break-words text-error-ink">{error}</p>)}
              {localCleanupModelActionErrors[model.id] && <p role="alert" className="break-words text-error-ink">{localCleanupModelActionErrors[model.id]}</p>}
              {selected && <p role="note" className="text-warning-ink">{t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")}</p>}
            </div>
          )}
        >
          {canActivate && <button type="button" disabled={Boolean(cleanupActivationBlock()) || probing || rowProbing !== null || removing || strictOfflineSaving || busy} onClick={() => onCommitLocalCleanupModel(model)} className={compactButtonClass}>{t("设为当前使用模型")}</button>}
          {downloading && <button type="button" disabled={busy} onClick={() => onRunLocalCleanupModelAction(model.id, "cancel")} className={compactButtonClass}>{busy ? t("处理中…") : t("取消下载")}</button>}
          {!downloading && !deleting && canOfferDownload && <button type="button" disabled={busy || model.downloadable !== true} onClick={() => onRunLocalCleanupModelAction(model.id, "download")} className={compactButtonClass}>{busy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
          {!downloading && !deleting && (model.managed || model.partial_managed) && <button type="button" aria-label={`${t(partialOnly ? "清理未完成文件" : "删除模型")}: ${model.label}`} disabled={busy || selected} onClick={() => onRequestDeleteLocalCleanupModel(model.id)} className={compactDangerButtonClass}>{t(partialOnly ? "清理未完成文件" : "删除模型")}</button>}
        </SettingsRow>
      </div>
    );
  };

  const renderMlxCleanupModelRow = (model: MlxCleanupModelStatus) => {
    const savedSelected = settings.cleanup_enabled
      && currentCleanupProvider === "on_device_mlx"
      && savedCleanupModel === model.id;
    const draftSelected = draftCleanupEnabled
      && draft.cleanupProvider === "on_device_mlx"
      && draft.cleanupModel === model.id
      && !savedSelected;
    const busy = Boolean(mlxCleanupModelBusy[model.id]);
    const downloading = model.operation === "download" || ["downloading", "canceling"].includes(model.state);
    const canceling = model.state === "canceling";
    const verifying = model.operation === "verify";
    const operationActive = isMlxCleanupModelOperationActive(model);
    const partialOnly = !model.managed && model.partial_managed;
    const transferBytes = model.bytes_total > 0 ? model.bytes_total : model.bytes;
    const canOfferDownload = model.download_available === true
      && ["missing", "corrupt", "error"].includes(model.state);
    const canVerify = model.state === "installed" && model.managed && !model.verified
      && (!model.operation || model.operation === "verify");
    const canActivate = model.selectable && model.state === "ready" && model.verified && model.compatible
      && !operationActive && !savedSelected;
    const showProgress = downloading || verifying;
    const operationErrors = [...new Set([model.error, model.last_operation_error]
      .filter((error): error is string => Boolean(error?.trim()) && !isCancellationError(error)))];
    const operationWasCancelled = !isMlxCleanupModelOperationActive(model)
      && isCancellationError(model.error ?? model.last_operation_error);
    const statusSummary = [
      model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知"),
      mlxCleanupModelStateCopy(model, t),
      operationWasCancelled ? t("已取消") : null,
    ].filter(Boolean).join(" · ");
    const language = mlxCleanupLanguageCopy(model.language);
    const detailId = `mlx-cleanup-details-${model.id}`;

    return (
      <div id={`mlx-cleanup-model-${model.id}`} key={model.id} tabIndex={-1} data-model-row={model.id} className="outline-none">
        <SettingsRow
          className="items-center py-3"
          title={model.label}
          description={(
            <div className="space-y-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                <span>{statusSummary}</span>
                {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                {model.selectable && <SettingsStatus label={t("可用于服务配置")} tone="success" />}
                {!model.compatible && <SettingsStatus label={t("当前设备不兼容")} tone="warning" />}
                <details className="inline-block max-w-full text-xs leading-5">
                  <summary aria-controls={detailId} className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationErrors.length > 0 || !model.compatible ? t("查看技术信息与错误") : t("技术信息")}</summary>
                  <div id={detailId} className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                    <p>{t("用途")}: {t("通用文字整理")}</p>
                    {language && <p>{t("语言与模式")}: {t(language)}</p>}
                    <p>{t("运行时")}: VoiceFlow MLX</p>
                    <p>{t("文件大小")}: {model.bytes > 0 ? formatModelBytes(model.bytes) : t("大小未知")}</p>
                    <p>{t("文件状态")}: {mlxCleanupModelStateCopy(model, t)}</p>
                    <p>{t("文件校验")}: {model.verified ? t("已通过") : t("尚未通过")}</p>
                    <p>{t("设备兼容性")}: {model.compatible ? t("兼容检查通过") : mlxCleanupCompatibilityCopy(model.compatibility_error, t)}</p>
                    <p>{t("来源仓库")}: {model.repository}</p>
                    <p className="break-all">{t("固定版本")}: {model.revision}</p>
                    {model.compatibility_error && <p>{t("兼容性错误代码")}: {model.compatibility_error}</p>}
                    {operationErrors.map((error) => <p key={error} className="break-words text-error-ink">{error}</p>)}
                  </div>
                </details>
              </div>
              <p className="text-xs leading-5 text-secondary">{t("适用语言")}: {language ? t(language) : t("语言覆盖尚未核实。")}</p>
              <p className="text-xs leading-5 text-secondary">
                {t("内存占用估算")}: {model.memory_estimate?.source === "admission_reserve"
                  ? `${formatModelBytes(model.memory_estimate.bytes)} · ${t("模型加载准入预留；估算值，不是实测峰值")}`
                  : t("此模型尚无可用的内存预留估算")}
              </p>
              <p className="text-xs leading-5 text-secondary">{t("速度特点")}: {mlxCleanupSpeedCharacteristic(model.id, model.label, t)}</p>
              <p className="text-xs leading-5 text-tertiary">{t("此 Mac 上的模型速度尚未实测。")}</p>
              {showProgress && (verifying || transferBytes > 0) && (
                <div className="space-y-1">
                  {verifying ? (
                    <>
                      <p role="status" className="text-xs text-tertiary">{t("正在校验模型文件")}</p>
                      <progress aria-label={`${t("模型校验进度")}: ${model.label}`} className="h-1.5 w-full accent-accent" />
                    </>
                  ) : (
                    <>
                      <p role="status" className="text-xs text-tertiary">{formatModelBytes(model.bytes_done)} / {formatModelBytes(transferBytes)}</p>
                      <progress
                        aria-label={`${t("模型下载进度")}: ${model.label}`}
                        className="h-1.5 w-full accent-accent"
                        max={transferBytes}
                        value={Math.min(transferBytes, model.bytes_done)}
                      />
                    </>
                  )}
                </div>
              )}
              {partialOnly && !downloading && <p role="note" className="text-secondary">{t(model.download_available ? "下载已中断；可以重新下载或清理部分文件。" : "此版本不再提供重新下载；可以清理未完成文件。")}</p>}
              {!model.compatible && <p role="note" className="text-warning-ink">{mlxCleanupCompatibilityCopy(model.compatibility_error, t)}</p>}
              {canActivate && cleanupActivationBlock() && <p role="note" className="text-warning-ink">{cleanupActivationBlock()}</p>}
              {model.download_available && !model.downloadable && canOfferDownload && model.compatible && <p role="note" className="text-warning-ink">{t("下载暂不可用")}</p>}
              {!model.download_available && (model.managed || model.partial_managed || savedSelected) && <p role="note" className="text-tertiary">{t("仅保留已安装文件管理和当前已保存路线；不提供新的下载或选择。")}</p>}
              {operationErrors.map((error) => <p key={error} role="alert" className="break-words text-error-ink">{error}</p>)}
              {mlxCleanupModelActionErrors[model.id] && <p role="alert" className="break-words text-error-ink">{mlxCleanupModelActionErrors[model.id]}</p>}
              {savedSelected && <p role="note" className="text-warning-ink">{t("先在服务配置中切换此模型或关闭 AI 整理，再删除模型。")}</p>}
            </div>
          )}
        >
          {canActivate && <button type="button" disabled={Boolean(cleanupActivationBlock()) || probing || rowProbing !== null || removing || strictOfflineSaving || busy} onClick={() => onCommitMlxCleanupModel(model)} className={compactButtonClass}>{t("设为当前使用模型")}</button>}
          {downloading && <button type="button" disabled={busy || canceling} onClick={() => onRunMlxCleanupModelAction(model.id, "cancel")} className={compactButtonClass}>{busy || canceling ? t("正在取消下载") : t("取消下载")}</button>}
          {canVerify && <button type="button" disabled={busy || verifying} onClick={() => onRunMlxCleanupModelAction(model.id, "verify")} className={compactButtonClass}>{busy || verifying ? t("正在校验…") : t("校验模型")}</button>}
          {!operationActive && canOfferDownload && <button type="button" disabled={busy || !model.downloadable} onClick={() => onRunMlxCleanupModelAction(model.id, "download")} className={compactButtonClass}>{busy ? t("准备中…") : model.state === "corrupt" ? t("重新下载") : t("下载模型")}</button>}
          {!operationActive && (model.managed || model.partial_managed) && <button type="button" aria-label={`${t(partialOnly ? "清理未完成文件" : "删除模型")}: ${model.label}`} disabled={busy || savedSelected} onClick={() => onRequestDeleteMlxCleanupModel(model.id)} className={compactDangerButtonClass}>{t(partialOnly ? "清理未完成文件" : "删除模型")}</button>}
        </SettingsRow>
      </div>
    );
  };

  return (
    <div
      id="engine-panel-local_models"
      role="region"
      aria-label={t("本地模型")}
      hidden={page !== "local_models"}
      inert={page !== "local_models"}
      className="vf-engine-page-content"
    >
      <SettingsGroup title={t("本机转写")} description={t("把录音变成文字。")}>
        <div className="border-b border-border px-5 py-3" aria-label={t("设备内存预算")}>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-xs font-medium text-secondary">{t("80% hw.memsize 内存准入预算")}</p>
            {deviceMemoryBudget?.physical_memory_bytes != null && deviceMemoryBudget.budget_bytes != null && (
              <p className="text-xs text-tertiary">
                {formatModelBytes(deviceMemoryBudget.budget_bytes)} / {formatModelBytes(deviceMemoryBudget.physical_memory_bytes)}
              </p>
            )}
          </div>
          {deviceMemoryBudget?.physical_memory_bytes != null && deviceMemoryBudget.budget_bytes != null ? (
            <progress
              aria-label={t("80% hw.memsize 内存准入预算")}
              className="mt-2 h-2 w-full accent-accent"
              max={deviceMemoryBudget.physical_memory_bytes}
              value={deviceMemoryBudget.budget_bytes}
            />
          ) : (
            <p className="mt-1 text-xs text-tertiary">{t("无法读取 hw.memsize；本机内存预算未知。")}</p>
          )}
          <p className="mt-1 text-xs leading-5 text-tertiary">{t("这是模型加载准入上限，不代表当前内存使用量。")}</p>
        </div>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="w-full min-w-0 border-0 p-0">
          <div id="local-asr-models" tabIndex={-1} className="divide-y divide-border outline-none">
            {onDeviceModelReadError && onDeviceModels.length > 0 && (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                <p role="alert" className="text-xs leading-5 text-error-ink">{onDeviceModelReadError}</p>
                <button type="button" onClick={onRefreshOnDeviceModels} className={compactButtonClass}>{t("重新读取模型状态")}</button>
              </div>
            )}
            {onDeviceModelsLoading && onDeviceModels.length === 0 ? (
              <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机模型状态…")}</p>
            ) : onDeviceModelReadError && onDeviceModels.length === 0 ? (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                <p role="alert" className="text-xs leading-5 text-error-ink">{onDeviceModelReadError}</p>
                <button type="button" onClick={onRefreshOnDeviceModels} className={compactButtonClass}>{t("重新读取模型状态")}</button>
              </div>
            ) : onDeviceModels.length === 0 ? (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                <p className="text-xs leading-5 text-tertiary">{t("本机模型状态尚未读取")}</p>
                <button type="button" onClick={onRefreshOnDeviceModels} className={compactButtonClass}>{t("重新读取模型状态")}</button>
              </div>
            ) : (
              <>
                {onDeviceModels.filter((model) => model.id !== "sensevoice-small" && (model.download_available === true
                  || model.state !== "missing"
                  || (currentAsrProvider === "on_device" && savedAsrModel === model.id)
                  || (draft.asrProvider === "on_device" && draft.asrModel === model.id))).map((model) => renderOnDeviceModelRow(model))}
                {onDeviceModels.some((model) => model.id !== "sensevoice-small" && !model.platform_supported) && (
                  <p role="note" className="px-5 py-2 text-xs leading-5 text-warning-ink">{t("本机转写模型需要 Apple Silicon 与 macOS 14 或更新版本。")}</p>
                )}
                {legacyModel && (legacyModel.state !== "missing" || legacyModelSavedSelected || legacyModelDraftSelected) && (
                  <>
                    {(legacyModelSavedSelected || legacyModelDraftSelected) && (
                      <p role="note" className="px-5 py-2 text-xs leading-5 text-warning-ink">
                        {legacyModelSavedSelected ? t("服务配置仍选用了旧版 SenseVoice；此模型不能用于本机听写。") : t("草稿选用了旧版 SenseVoice；此模型不能用于本机听写。")}
                      </p>
                    )}
                    <details
                      open={legacyModelsOpen || legacyModelShouldOpen}
                      onToggle={(event) => onLegacyModelsOpenChange(event.currentTarget.open)}
                      className="px-5 py-1"
                    >
                      <summary className={`cursor-pointer py-2 text-xs font-medium text-secondary ${focusRingClass}`}>{t("旧版文件管理")}</summary>
                      <div className="-mx-5 border-t border-border">{renderOnDeviceModelRow(legacyModel, true)}</div>
                    </details>
                  </>
                )}
              </>
            )}
            {onDeviceActionError && !onDeviceModels.some((model) => model.id === onDeviceActionError.id) && <p role="alert" className="px-5 py-3 text-xs text-error-ink">{onDeviceActionError.message}</p>}
          </div>
        </fieldset>
      </SettingsGroup>
      <SettingsGroup title={t("本机文字整理")} description={t("在这台 Mac 上调整转写文字和排版。")}>
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="w-full min-w-0 border-0 p-0">
          <div id="local-mlx-cleanup-models" tabIndex={-1} className="divide-y divide-border outline-none">
            {mlxCleanupModelsReadError && visibleMlxCleanupModels.length > 0 && (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                <p role="alert" className="text-xs leading-5 text-error-ink">{mlxCleanupModelsReadError}</p>
                <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => onRefreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("重新读取 MLX 整理模型状态")}</button>
              </div>
            )}
            {mlxCleanupModelsLoading && visibleMlxCleanupModels.length === 0 ? (
              <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机 MLX 整理模型状态…")}</p>
            ) : mlxCleanupModelsReadError && visibleMlxCleanupModels.length === 0 ? (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                <p role="alert" className="text-xs leading-5 text-error-ink">{mlxCleanupModelsReadError}</p>
                <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => onRefreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("刷新 MLX 整理模型状态")}</button>
              </div>
            ) : visibleMlxCleanupModels.length === 0 ? (
              <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                <p role="status" className="text-xs leading-5 text-tertiary">{t("没有其他可下载或需要管理的本机 MLX 整理模型。")}</p>
                <button type="button" disabled={mlxCleanupModelsLoading} onClick={() => onRefreshMlxCleanupModels(true)} className={compactButtonClass}>{mlxCleanupModelsLoading ? t("正在读取…") : t("刷新 MLX 整理模型状态")}</button>
              </div>
            ) : (
              visibleMlxCleanupModels.map(renderMlxCleanupModelRow)
            )}
            <div id="local-cleanup-model" tabIndex={-1} className="outline-none">
              {cleanupModelStoreStatus ? (() => {
                const status = cleanupModelStoreStatus;
                const savedSelected = settings.cleanup_enabled && currentCleanupProvider === "on_device_mlx" && savedCleanupModel === status.id;
                const draftSelected = draftCleanupEnabled && draft.cleanupProvider === "on_device_mlx" && draft.cleanupModel === status.id && !savedSelected;
                const transferInProgress = Boolean(status.operation && status.operation !== "delete");
                const isCancellation = (message: string | null | undefined) => /^(download|import) cancelled$/i.test(message ?? "");
                const statusError = isCancellation(status.error) ? null : status.error;
                const lastOperationError = isCancellation(status.last_operation_error) ? null : status.last_operation_error;
                const operationError = statusError || lastOperationError;
                const operationWasCancelled = !status.operation && !operationError && (isCancellation(status.error) || isCancellation(status.last_operation_error));
                const statusSummary = [
                  status.bytes_total > 0 ? formatModelBytes(status.bytes_total) : t("大小未知"),
                  cleanupModelStoreSummaryCopy(status.state, t),
                  operationWasCancelled && status.state !== "cancelled" ? t("已取消") : null,
                ].filter(Boolean).join(" · ");
                const modelName = status.label.replace(/\s+·\s+MLX(?:\s+.*)?$/i, "");
                const canActivate = status.managed && status.state === "ready" && !savedSelected;
                const activationBlock = canActivate ? legacyMlxActivationBlock(status) : null;

                return (
                  <SettingsRow
                    className="items-center py-3"
                    title={modelName}
                    description={(
                      <div className="space-y-1">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5">
                          <span>{statusSummary}</span>
                          {savedSelected && <SettingsStatus label={t("服务配置已选")} tone="accent" />}
                          {draftSelected && <SettingsStatus label={t("待应用")} tone="warning" />}
                          <details className="inline-block max-w-full text-xs leading-5">
                            <summary className={`cursor-pointer text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{operationError ? t("查看技术信息与错误") : t("技术信息")}</summary>
                            <div className="mt-1 space-y-1 text-xs leading-5 text-secondary">
                              <p>{t("模型构建")}: {status.label}</p>
                              <p>{t("来源仓库")}: {status.repository}</p>
                              <p className="break-all">{t("固定版本")}: {status.revision}</p>
                              <p>{t("文件状态")}: {cleanupModelStoreStateCopy(status.state, t)}</p>
                              <p>{t("文件系统说明")}: {t("导入模型会使用 APFS 共享块；原始文件夹不会更改。")}</p>
                              {statusError && <p className="break-words text-error-ink">{statusError}</p>}
                              {lastOperationError && lastOperationError !== statusError && <p className="break-words text-error-ink">{lastOperationError}</p>}
                            </div>
                          </details>
                        </div>
                        {status.language && <p className="text-xs leading-5 text-secondary">{t("适用语言")}: {t(status.language)}</p>}
                        <p className="text-xs leading-5 text-secondary">
                          {t("内存占用估算")}: {status.memory_estimate?.source === "admission_reserve"
                            ? `${formatModelBytes(status.memory_estimate.bytes)} · ${t("模型加载准入预留；估算值，不是实测峰值")}`
                            : t("此模型尚无可用的内存预留估算")}
                        </p>
                        <p className="text-xs leading-5 text-secondary">{t("速度特点")}: {mlxCleanupSpeedCharacteristic(status.id, status.label, t)}</p>
                        <p className="text-xs leading-5 text-tertiary">{t("此 Mac 上的模型速度尚未实测。")}</p>
                        {activationBlock && <p role="note" className="text-xs leading-5 text-warning-ink">{activationBlock}</p>}
                        {transferInProgress && status.bytes_total > 0 && (
                          <div className="space-y-1">
                            <p role="status" className="text-xs text-tertiary">{formatModelBytes(status.bytes_done)} / {formatModelBytes(status.bytes_total)}</p>
                            <progress aria-label={t("本机模型操作进度")} className="h-1.5 w-full accent-accent" max={100} value={Math.min(100, status.bytes_done / status.bytes_total * 100)} />
                          </div>
                        )}
                        {status.platform_supported === false && <p className="text-warning-ink">{t("VoiceFlow 本机 MLX 整理需要 Apple Silicon 和 macOS 14 或更新版本。")}</p>}
                        {(status.platform_supported === null || status.platform_supported === undefined) && <p className="text-warning-ink">{t("无法确认此 Mac 是否支持 VoiceFlow 本机 MLX 整理；请刷新模型状态后重试。")}</p>}
                        {statusError && <p role="alert" className="text-error-ink">{t("模型状态有错误；查看技术信息与错误。")}</p>}
                        {lastOperationError && !statusError && <p role="alert" className="text-error-ink">{t("上次模型操作失败；查看技术信息与错误。")}</p>}
                        {cleanupModelStoreReadError && <p role="alert" className="text-error-ink">{cleanupModelStoreReadError}</p>}
                        {cleanupModelStoreActionError && <p role="alert" className="text-error-ink">{cleanupModelStoreActionError}</p>}
                      </div>
                    )}
                  >
                    {canActivate && <button type="button" disabled={Boolean(activationBlock) || probing || rowProbing !== null || removing || strictOfflineSaving || cleanupModelStoreBusy} onClick={() => onCommitLegacyMlxModel(status)} className={compactButtonClass}>{t("设为当前使用模型")}</button>}
                    {transferInProgress ? (
                      <button type="button" disabled={cleanupModelStoreBusy} onClick={() => onRunCleanupModelStoreAction("cancel")} className={compactButtonClass}>{cleanupModelStoreBusy ? t("处理中…") : t("取消模型操作")}</button>
                    ) : null}
                    {cleanupModelStoreIdle && !status.managed && (
                      <>
                        <button type="button" disabled={cleanupModelStoreBusy || !cleanupMlxPlatformSupported} onClick={() => onRunCleanupModelStoreAction("download")} className={compactButtonClass}>{cleanupModelStoreBusy ? t("准备中…") : t("下载模型")}</button>
                        <details className="max-w-full text-left">
                          <summary className={`cursor-pointer px-2 py-1 text-xs text-tertiary underline-offset-2 hover:text-primary hover:underline ${focusRingClass}`}>{t("导入已有模型")}</summary>
                          <div className="mt-1 space-y-1 text-right">
                            <p className="text-xs leading-5 text-tertiary">{t("导入匹配的 MLX 模型文件夹；原始文件保留。")}</p>
                            <button type="button" disabled={cleanupModelStoreBusy || !cleanupMlxPlatformSupported} onClick={() => onRunCleanupModelStoreAction("import")} className={ghostButtonClass}>{cleanupModelStoreBusy ? t("正在打开文件夹…") : t("选择文件夹并导入")}</button>
                          </div>
                        </details>
                      </>
                    )}
                    {cleanupModelStoreIdle && (status.managed || status.partial_managed) && <button type="button" aria-label={`${t("删除模型")}: ${modelName}`} disabled={cleanupModelStoreBusy} onClick={onRequestDeleteCleanupModel} className={compactDangerButtonClass}>{t("删除模型")}</button>}
                    {cleanupModelStoreNeedsRefresh && <button type="button" disabled={cleanupModelStoreBusy} onClick={onRefreshCleanupModelStore} className={compactButtonClass}>{t("刷新模型状态")}</button>}
                  </SettingsRow>
                );
              })() : (
                <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                  <p role={cleanupModelStoreReadError ? "alert" : "status"} className={`text-xs leading-5 ${cleanupModelStoreReadError ? "text-error-ink" : "text-tertiary"}`}>{cleanupModelStoreReadError ?? t("正在读取模型状态")}</p>
                  {cleanupModelStoreNeedsRefresh && <button type="button" disabled={cleanupModelStoreBusy} onClick={onRefreshCleanupModelStore} className={compactButtonClass}>{t("刷新模型状态")}</button>}
                </div>
              )}
            </div>
          </div>
          <div id="local-cleanup-models" tabIndex={-1} className="outline-none">
            {localCleanupManagementRequired && (
              <div className="border-t border-border">
                <h3 className="px-5 pb-2 pt-4 text-xs font-medium text-tertiary">{t("旧版 GGUF 文件管理")}</h3>
                <div className="divide-y divide-border">
                  {localCleanupModelsLoading && visibleLocalCleanupModels.length === 0 ? (
                    <p role="status" className="px-5 py-3 text-xs leading-5 text-tertiary">{t("正在读取本机整理模型状态…")}</p>
                  ) : localCleanupModelsReadError && visibleLocalCleanupModels.length === 0 ? (
                    <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                      <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupModelsReadError}</p>
                      <button type="button" disabled={localCleanupModelsLoading} onClick={() => onRefreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("重新读取本机整理模型状态")}</button>
                    </div>
                  ) : (
                    <>
                      {localCleanupModelsReadError && (
                        <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-2">
                          <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupModelsReadError}</p>
                          <button type="button" disabled={localCleanupModelsLoading} onClick={() => onRefreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("重新读取本机整理模型状态")}</button>
                        </div>
                      )}
                      {visibleLocalCleanupModels.length === 0 ? (
                        <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3">
                          <p role="status" className="text-xs leading-5 text-tertiary">{t("没有可管理的 GGUF 模型。")}</p>
                          <button type="button" disabled={localCleanupModelsLoading} onClick={() => onRefreshLocalCleanupModels(true)} className={compactButtonClass}>{localCleanupModelsLoading ? t("正在读取…") : t("刷新本机整理模型状态")}</button>
                        </div>
                      ) : (
                        visibleLocalCleanupModels.map(renderLocalCleanupModelRow)
                      )}
                    </>
                  )}
                </div>
              </div>
            )}
          </div>
        </fieldset>
      </SettingsGroup>
    </div>
  );
}
