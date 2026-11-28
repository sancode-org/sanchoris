import type { Logger } from "./log.ts"
import type { Agent, TurnError, Usage, WorkerCredential } from "./protocol.ts"

export interface WorkerTurnResult {
  result: "completed" | "failed" | "interrupted"
  summary: string
  error: TurnError | null
  usage: Usage
  costUsd: number | null
  durationMs: number
}

export interface WorkerRateLimit {
  status: string
  resetsAt: number | null
  windows: Record<
    string,
    { utilization: number | null; resetsAt: number | null; status: string | null }
  >
}

export type EventKind = "text" | "thinking" | "tool_use" | "tool_result" | "log" | "system"

export interface WorkerHandlers {
  /** The Worker began the turn asked for with `turn` or `compact`. */
  onTurnStarted: () => void
  onEvent: (kind: EventKind, data: Record<string, unknown>) => void
  onTurnEnd: (result: WorkerTurnResult) => void
  onRateLimit: (rateLimit: WorkerRateLimit) => void
  onFatal: (error: Error) => void
}

export interface WorkerContext {
  model: string
  workspaceDir: string
  /** The provider's state directory: its configuration, conversations and credential files. */
  home: string
  env: NodeJS.ProcessEnv
  handlers: WorkerHandlers
  logger: Logger
}

/** One provider conversation, driven by turns (adapted from T3 Code's provider adapters). */
export interface Worker {
  /** Whether a running turn takes more input (`steer`). */
  readonly steerable: boolean
  start(resumeSessionId: string | null): Promise<{ sessionId: string }>
  /** Starts a turn with `text`. The agent calls it only between turns. */
  turn(text: string): void
  /** Puts `text` into the running turn; false when the turn ended before it could. */
  steer(text: string): Promise<boolean>
  /** Starts a turn that compacts the conversation. */
  compact(): void
  interrupt(): Promise<void>
  close(): Promise<void>
}

/** What the agent knows of a provider: where it keeps its state, credentials and skills. */
export interface WorkerProvider {
  agent: Agent | "fake"
  /** The environment variable that points the provider at its state directory. */
  homeVariable: string
  /** Entries of the state directory left out of the thread record. */
  recordExclude: readonly string[]
  /** The directory whose `<name>/SKILL.md` the Worker reads as its own skills. */
  skillsDir: (home: string) => string
  /** Puts the credential where the provider reads it, and returns the environment it needs. */
  installCredential: (home: string, credential: WorkerCredential) => Promise<Record<string, string>>
  /** The credential as the provider holds it now, when the provider can refresh it. */
  readCredential: (home: string, current: WorkerCredential) => Promise<WorkerCredential | null>
  create: (context: WorkerContext) => Worker
}

export const emptyUsage = (): Usage => ({
  inputTokens: 0,
  outputTokens: 0,
  cacheReadInputTokens: 0,
  cacheCreationInputTokens: 0,
})

export function requireEnv(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name]
  if (value === undefined || value === "")
    throw new Error(`Missing required environment variable ${name}`)
  return value
}
