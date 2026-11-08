/* Adapted from Handy onboarding/ModelCard.tsx lines 72–161, 289–349 and
 * onboarding/Onboarding.tsx lines 142–159 (MIT), commit
 * 14f6f0d31cb22a4268acfbddba3052820fd0a06e.
 * Copyright (c) 2025 CJ Pais. Full notice: ./HANDY-LICENSE.txt. */
import type { ReactNode } from "react";
import type { OnDeviceModelStatus } from "../../types/settings";
import { useI18n } from "../../lib/i18n";

export function ModelCard({ model, children }: { model: OnDeviceModelStatus | undefined; children: ReactNode }) {
  const { t } = useI18n();
  const status = model?.state === "downloading" ? "downloading" : model?.state === "ready" ? "available" : "downloadable";
  const downloadProgress = model && Number.isFinite(model.downloaded_bytes) && model.bytes > 0 ? Math.min(100, Math.max(0, model.downloaded_bytes / model.bytes * 100)) : undefined;
  // Available files only describe the download owner; loading is a separate receipt.
  return <div className="mt-3 flex flex-col gap-2 rounded-xl border border-border px-4 py-3 text-left">
    <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="text-sm font-semibold">{model?.label ?? t("本机模型")}</h3>{model && Number.isFinite(model.bytes) && <span className="text-xs text-tertiary">{(model.bytes / 1024 ** 3).toFixed(1)} GB</span>}</div>
    {status === "downloading" && downloadProgress !== undefined && <div className="mt-1 w-full">
      <progress aria-label={t("模型下载进度")} className="h-2 w-full accent-accent" max={100} value={downloadProgress} />
      <p className="mt-1 text-xs text-secondary">{Math.round(downloadProgress)}%</p>
    </div>}
    {children}
  </div>;
}
