// Delivery status of one owner message in a conversation thread, derived
// from the messages alone (no schema change): "sent" while nothing came
// back, "answered" once an agent message exists after it.
// Pure function, no DOM.

export type MessageStatus = "sent" | "answered";

export interface StatusMessage {
  role: string;
}

// Roles a message can carry on the way to "answered": the agent's reply, a
// question it asks the owner, and the notices it posts while working.
const AGENT_ROLES = new Set(["assistant", "question", "notice"]);

export function messageStatus(m: StatusMessage, later: StatusMessage[]): MessageStatus {
  return later.some((x) => AGENT_ROLES.has(x.role)) ? "answered" : "sent";
}
