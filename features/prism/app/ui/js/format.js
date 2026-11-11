/** Text helpers shared by the views. */

const AGENT_LABELS = { claude: 'Claude', codex: 'Codex' };
/** Enough of a conversation ID to tell two conversations of one project apart. */
const SHORT_CONVERSATION_CHARS = 8;

export function agentLabel(agent) {
  return AGENT_LABELS[agent] ?? agent;
}

/** Last path component of a project directory. */
export function projectName(project) {
  return project.split(/[\\/]/).filter(Boolean).at(-1) ?? project;
}

export function shortConversation(conversation) {
  return conversation.slice(0, SHORT_CONVERSATION_CHARS);
}

/** A moment in milliseconds since the Unix epoch, as date and time in the user's locale. */
export function formatTime(ms) {
  return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** The text's lines joined into one paragraph, for an excerpt clamped to a few lines. */
export function flowText(text) {
  return text.split('\n').map((line) => line.trim()).filter((line) => line !== '').join(' ');
}
