import { useEffect, useRef } from "react";
import {
  SESSION_COMMAND_EVENTS,
  type SessionCommandEvent,
} from "../../lib/session-command-events";

export interface UseSessionCommandEventsOptions {
  active: boolean;
  onResume: () => void;
  onExport: () => void;
  onFavorite: () => void;
  onWatch: () => void;
  onDelete: () => void;
  onSessionSearch: () => void;
}

export function useSessionCommandEvents(
  opts: UseSessionCommandEventsOptions,
): void {
  // Latest-value ref so the once-mounted document listeners read the current
  // `active` flag + callbacks (Solid read `opts.*` fresh at event time).
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    const runIfActive = (callback: () => void) => {
      if (optsRef.current.active) callback();
    };

    const handlers: Array<[SessionCommandEvent, EventListener]> = [
      [
        SESSION_COMMAND_EVENTS.resume,
        () => runIfActive(optsRef.current.onResume),
      ],
      [
        SESSION_COMMAND_EVENTS.exportSession,
        () => runIfActive(optsRef.current.onExport),
      ],
      [
        SESSION_COMMAND_EVENTS.favorite,
        () => runIfActive(optsRef.current.onFavorite),
      ],
      [
        SESSION_COMMAND_EVENTS.watch,
        () => runIfActive(optsRef.current.onWatch),
      ],
      [
        SESSION_COMMAND_EVENTS.delete,
        () => runIfActive(optsRef.current.onDelete),
      ],
      [
        SESSION_COMMAND_EVENTS.sessionSearch,
        () => runIfActive(optsRef.current.onSessionSearch),
      ],
    ];

    for (const [eventName, handler] of handlers) {
      document.addEventListener(eventName, handler);
    }

    return () => {
      for (const [eventName, handler] of handlers) {
        document.removeEventListener(eventName, handler);
      }
    };
  }, []);
}
