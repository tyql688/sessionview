import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { MaintenanceEvent } from "./types";

export interface BackendEventPayloads {
  "sessions-changed": string[];
  "maintenance-status": MaintenanceEvent;
}

export function listenBackendEvent<Name extends keyof BackendEventPayloads>(
  name: Name,
  handler: (payload: BackendEventPayloads[Name]) => void,
): Promise<UnlistenFn> {
  return listen<BackendEventPayloads[Name]>(name, (event) => {
    handler(event.payload);
  });
}

export type { UnlistenFn };
