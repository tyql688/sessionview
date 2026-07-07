export const SESSION_COMMAND_EVENTS = {
  sessionSearch: "cc-session:session-search",
  resume: "cc-session:resume",
  exportSession: "cc-session:export",
  favorite: "cc-session:favorite",
  delete: "cc-session:delete",
} as const;

export type SessionCommandEvent = (typeof SESSION_COMMAND_EVENTS)[keyof typeof SESSION_COMMAND_EVENTS];

export function dispatchSessionCommand(eventName: SessionCommandEvent): void {
  document.dispatchEvent(new CustomEvent(eventName));
}
