import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  CleanupModelStatus,
  LocalCleanupModelStatus,
  MlxCleanupModelStatus,
  OnDeviceModelStatus,
} from "../types/settings";

export const LOCAL_MODELS_STATUS_EVENT = "local-models://status";

export type LocalModelStatusEvent =
  | { domain: "on_device_asr"; status: OnDeviceModelStatus }
  | { domain: "on_device_runtime" }
  | { domain: "cleanup_store"; status: CleanupModelStatus }
  | { domain: "mlx_cleanup"; status: MlxCleanupModelStatus }
  | { domain: "gguf_cleanup"; status: LocalCleanupModelStatus };

export type UseLocalModelsStatusOptions = {
  enabled?: boolean;
  onOnDeviceAsrStatus?: (status: OnDeviceModelStatus) => void;
  onOnDeviceRuntimeChanged?: () => void;
  onCleanupStoreStatus?: (status: CleanupModelStatus) => void;
  onMlxCleanupStatus?: (status: MlxCleanupModelStatus) => void;
  onGgufCleanupStatus?: (status: LocalCleanupModelStatus) => void;
};

export function useLocalModelsStatus({
  enabled = true,
  onOnDeviceAsrStatus,
  onOnDeviceRuntimeChanged,
  onCleanupStoreStatus,
  onMlxCleanupStatus,
  onGgufCleanupStatus,
}: UseLocalModelsStatusOptions) {
  const callbacksRef = useRef({
    onOnDeviceAsrStatus,
    onOnDeviceRuntimeChanged,
    onCleanupStoreStatus,
    onMlxCleanupStatus,
    onGgufCleanupStatus,
  });
  callbacksRef.current = {
    onOnDeviceAsrStatus,
    onOnDeviceRuntimeChanged,
    onCleanupStoreStatus,
    onMlxCleanupStatus,
    onGgufCleanupStatus,
  };

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const unlisteners: Promise<() => void>[] = [];
    const lastSignatures = new Map<string, string>();
    const shouldDispatch = (key: string, value: unknown) => {
      try {
        const signature = JSON.stringify(value);
        if (lastSignatures.get(key) === signature) return false;
        lastSignatures.set(key, signature);
      } catch {
        // If serialization fails, deliver the event rather than dropping state.
      }
      return true;
    };

    unlisteners.push(
      listen<LocalModelStatusEvent>(LOCAL_MODELS_STATUS_EVENT, (event) => {
        if (cancelled || !event.payload) return;
        const payload = event.payload;
        switch (payload.domain) {
          case "on_device_asr":
            if (payload.status?.id && shouldDispatch(`on_device_asr:${payload.status.id}`, payload.status)) {
              callbacksRef.current.onOnDeviceAsrStatus?.(payload.status);
            }
            break;
          case "on_device_runtime":
            callbacksRef.current.onOnDeviceRuntimeChanged?.();
            break;
          case "cleanup_store":
            if (payload.status && shouldDispatch("cleanup_store", payload.status)) {
              callbacksRef.current.onCleanupStoreStatus?.(payload.status);
            }
            break;
          case "mlx_cleanup":
            if (payload.status?.id && shouldDispatch(`mlx_cleanup:${payload.status.id}`, payload.status)) {
              callbacksRef.current.onMlxCleanupStatus?.(payload.status);
            }
            break;
          case "gguf_cleanup":
            if (payload.status?.id && shouldDispatch(`gguf_cleanup:${payload.status.id}`, payload.status)) {
              callbacksRef.current.onGgufCleanupStatus?.(payload.status);
            }
            break;
        }
      }),
    );

    if (onOnDeviceAsrStatus) {
      unlisteners.push(
        listen<OnDeviceModelStatus>("ondevice://download", (event) => {
          if (cancelled || !event.payload?.id) return;
          if (shouldDispatch(`on_device_asr:${event.payload.id}`, event.payload)) {
            callbacksRef.current.onOnDeviceAsrStatus?.(event.payload);
          }
        }),
      );
    }

    if (onMlxCleanupStatus) {
      unlisteners.push(
        listen<MlxCleanupModelStatus>("mlx-cleanup-model://status", (event) => {
          if (cancelled || !event.payload?.id) return;
          if (shouldDispatch(`mlx_cleanup:${event.payload.id}`, event.payload)) {
            callbacksRef.current.onMlxCleanupStatus?.(event.payload);
          }
        }),
      );
    }

    if (onGgufCleanupStatus) {
      unlisteners.push(
        listen<LocalCleanupModelStatus>("local-cleanup-model://status", (event) => {
          if (cancelled || !event.payload?.id) return;
          if (shouldDispatch(`gguf_cleanup:${event.payload.id}`, event.payload)) {
            callbacksRef.current.onGgufCleanupStatus?.(event.payload);
          }
        }),
      );
    }

    return () => {
      cancelled = true;
      for (const promise of unlisteners) {
        void promise.then((unlisten) => unlisten()).catch(() => undefined);
      }
    };
  }, [enabled, Boolean(onOnDeviceAsrStatus), Boolean(onMlxCleanupStatus), Boolean(onGgufCleanupStatus)]);
}
