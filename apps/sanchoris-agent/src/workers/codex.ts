import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process"
import { readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"

import type { WorkerCredential } from "../protocol.ts"
import {
  emptyUsage,
  type Worker,
  type WorkerContext,
  type WorkerProvider,
  type WorkerRateLimit,
  type WorkerTurnResult,
} from "../worker.ts"
import { CodexAppServerClient } from "./codex-rpc.ts"

const AUTH_FILE = "auth.json"
const SESSION_MINUTES = 5 * 60
const WEEK_MINUTES = 7 * 24 * 60
const MAX_TOOL_RESULT_CHARS = 4000

interface CodexRateLimitWindow {
  usedPercent: number
  resetsAt?: number | null
  windowDurationMins?: number | null
}

export interface CodexRateLimitSnapshot {
  limitId?: string | null
  rateLimitReachedType?: string | null
  primary?: CodexRateLimitWindow | null
  secondary?: CodexRateLimitWindow | null
}

interface CodexTokenUsage {
  inputTokens: number
  cachedInputTokens: number
  outputTokens: number
  cacheWriteInputTokens?: number
}

interface CodexTurn {
  id: string
  status: "completed" | "interrupted" | "failed" | "inProgress"
  error?: { message: string; codexErrorInfo?: unknown } | null
  durationMs?: number | null
}

interface CodexItem {
  type: string
  id?: string
  text?: string
  command?: string
  exitCode?: number | null
  aggregatedOutput?: string | null
  summary?: string[]
  content?: string[]
  changes?: { path: string; kind: string | { type: string } }[]
}

function windowName(window: CodexRateLimitWindow, fallbackMinutes: number): string {
  const minutes =
    typeof window.windowDurationMins === "number" ? window.windowDurationMins : fallbackMinutes
  if (minutes === SESSION_MINUTES) return "five_hour"
  if (minutes === WEEK_MINUTES) return "seven_day"
  return `${minutes}_minutes`
}

/** Keeps the main allowance (`limitId` `codex`) across partial updates (T3 Code `mergeCodexRateLimits`). */
export function mergeCodexRateLimits(
  previous: CodexRateLimitSnapshot | null,
  update: CodexRateLimitSnapshot,
): CodexRateLimitSnapshot | null {
  if (update.limitId && update.limitId !== "codex") return previous
  if (!previous) return update
  const merged: CodexRateLimitSnapshot = { ...previous }
  for (const key of ["limitId", "rateLimitReachedType", "primary", "secondary"] as const)
    if (update[key] !== undefined) Object.assign(merged, { [key]: update[key] })
  return merged
}

export function codexRateLimit(snapshot: CodexRateLimitSnapshot): WorkerRateLimit {
  const windows: WorkerRateLimit["windows"] = {}
  let latestReset: number | null = null
  for (const [window, fallback] of [
    [snapshot.primary, SESSION_MINUTES],
    [snapshot.secondary, WEEK_MINUTES],
  ] as const) {
    if (!window || !Number.isFinite(window.usedPercent)) continue
    const resetsAt =
      typeof window.resetsAt === "number" && window.resetsAt > 0 ? window.resetsAt : null
    windows[windowName(window, fallback)] = {
      utilization: window.usedPercent / 100,
      resetsAt,
      status: window.usedPercent >= 100 ? "rejected" : "allowed",
    }
    if (window.usedPercent >= 100 && resetsAt !== null)
      latestReset = Math.max(latestReset ?? 0, resetsAt)
  }
  return {
    status: snapshot.rateLimitReachedType ? "rejected" : "allowed",
    resetsAt: latestReset,
    windows,
  }
}

/** `usageLimitExceeded`, `rateLimitExceeded` or HTTP 429 (T3 Code `codexUsageLimitError`). */
export function isCodexUsageLimit(info: unknown): boolean {
  if (info === "usageLimitExceeded" || info === "rateLimitExceeded") return true
  if (typeof info !== "object" || info === null) return false
  if ("usageLimitExceeded" in info || "rateLimitExceeded" in info) return true
  const failed = (info as { responseTooManyFailedAttempts?: { httpStatusCode?: number | null } })
    .responseTooManyFailedAttempts
  return failed?.httpStatusCode === 429
}

const textInput = (text: string) => [{ type: "text", text, text_elements: [] }]

/** A Codex app-server over stdio for one thread (T3 Code `CodexAdapterV2.ts`). */
export class CodexWorker implements Worker {
  readonly steerable = true
  private readonly context: WorkerContext
  private child: ChildProcessWithoutNullStreams | null = null
  private client: CodexAppServerClient | null = null
  private threadId: string | null = null
  private activeTurnId: string | null = null
  private readonly completedTurnIds = new Set<string>()
  private compacting = false
  private closing = false
  private lastText = ""
  private lastUsage: CodexTokenUsage | null = null
  private rateLimits: CodexRateLimitSnapshot | null = null
  private exited: Promise<void> = Promise.resolve()

  constructor(context: WorkerContext) {
    this.context = context
  }

  async start(resumeSessionId: string | null): Promise<{ sessionId: string }> {
    const child = spawn(this.context.env.SANCHORIS_CODEX_PATH || "codex", ["app-server"], {
      cwd: this.context.workspaceDir,
      env: this.context.env,
      stdio: ["pipe", "pipe", "pipe"],
    })
    this.child = child
    child.stderr.setEncoding("utf8")
    child.stderr.on("data", (data: string) => {
      for (const line of data.split("\n"))
        if (line !== "") this.context.handlers.onEvent("log", { line })
    })
    this.exited = new Promise((resolve) => child.once("exit", () => resolve()))
    child.on("exit", (code, signal) => {
      const error = new Error(
        `Codex app-server exited (code ${String(code)}, signal ${String(signal)})`,
      )
      this.client?.rejectAll(error)
      if (!this.closing) this.context.handlers.onFatal(error)
    })
    const client = new CodexAppServerClient(child, {
      onNotification: (method, params) => this.handleNotification(method, params),
      onRequest: async (method) => {
        this.context.handlers.onEvent("log", { line: `Codex request ${method} was declined` })
        throw new Error(`sanchoris-agent does not answer ${method}`)
      },
      onProtocolError: (error) => this.context.handlers.onFatal(error),
    })
    this.client = client
    await client.request("initialize", {
      clientInfo: { name: "sanchoris-agent", title: "sanchoris-agent", version: "0.1.0" },
      capabilities: { experimentalApi: true },
    })
    client.notify("initialized")
    const threadParams = {
      cwd: this.context.workspaceDir,
      model: this.context.model,
      config: { "tools.update_plan.enabled": true },
    }
    const response = (
      resumeSessionId
        ? await client.request("thread/resume", { threadId: resumeSessionId, ...threadParams })
        : await client.request("thread/start", threadParams)
    ) as { thread: { id: string } }
    this.threadId = response.thread.id
    return { sessionId: response.thread.id }
  }

  turn(text: string): void {
    const client = this.client
    if (!client || !this.threadId) throw new Error("Codex thread is not running")
    this.lastText = ""
    this.lastUsage = null
    client
      .request("turn/start", {
        threadId: this.threadId,
        input: textInput(text),
        cwd: this.context.workspaceDir,
        model: this.context.model,
        summary: "detailed",
        approvalsReviewer: "user",
        approvalPolicy: "never",
        sandboxPolicy: { type: "dangerFullAccess" },
        collaborationMode: { mode: "default", settings: { model: this.context.model } },
      })
      .then(
        (result) => this.begin((result as { turn: { id: string } }).turn.id),
        (error: Error) => this.context.handlers.onFatal(error),
      )
  }

  async steer(text: string): Promise<boolean> {
    if (!this.client || !this.threadId || !this.activeTurnId) return false
    try {
      await this.client.request("turn/steer", {
        threadId: this.threadId,
        expectedTurnId: this.activeTurnId,
        input: textInput(text),
      })
      return true
    } catch (error) {
      this.context.handlers.onEvent("log", {
        line: `turn/steer failed: ${(error as Error).message}`,
      })
      return false
    }
  }

  compact(): void {
    if (!this.client || !this.threadId) throw new Error("Codex thread is not running")
    this.lastText = ""
    this.compacting = true
    this.client
      .request("thread/compact/start", { threadId: this.threadId })
      .catch((error: Error) => this.context.handlers.onFatal(error))
  }

  async interrupt(): Promise<void> {
    if (!this.client || !this.threadId || !this.activeTurnId) return
    await this.client.request("turn/interrupt", {
      threadId: this.threadId,
      turnId: this.activeTurnId,
    })
  }

  async close(): Promise<void> {
    this.closing = true
    this.child?.kill("SIGTERM")
    await this.exited
  }

  private begin(turnId: string): void {
    if (this.activeTurnId === turnId || this.completedTurnIds.has(turnId)) return
    this.activeTurnId = turnId
    this.context.handlers.onTurnStarted()
  }

  private handleNotification(method: string, params: unknown): void {
    const { onEvent, onRateLimit } = this.context.handlers
    switch (method) {
      case "turn/started": {
        const turn = (params as { turn: CodexTurn }).turn
        if (this.compacting) {
          this.compacting = false
          this.begin(turn.id)
        }
        return
      }
      case "item/started": {
        const item = (params as { item: CodexItem }).item
        if (item.type === "commandExecution")
          onEvent("tool_use", {
            id: item.id ?? null,
            name: "commandExecution",
            input: { command: item.command ?? null },
          })
        return
      }
      case "item/completed": {
        const item = (params as { item: CodexItem }).item
        if (item.type === "agentMessage") {
          this.lastText = item.text ?? ""
          onEvent("text", { text: this.lastText })
        } else if (item.type === "commandExecution") {
          onEvent("tool_result", {
            toolUseId: item.id ?? null,
            isError: (item.exitCode ?? 0) !== 0,
            content: (item.aggregatedOutput ?? "").slice(0, MAX_TOOL_RESULT_CHARS),
          })
        } else if (item.type === "reasoning") {
          const text = (item.summary?.length ? item.summary : (item.content ?? []))
            .join("\n\n")
            .trim()
          if (text !== "") onEvent("thinking", { text })
        } else if (item.type === "fileChange") {
          const changes = (item.changes ?? []).map((change) => ({
            path: change.path,
            kind: typeof change.kind === "string" ? change.kind : change.kind.type,
          }))
          onEvent("tool_use", { id: item.id ?? null, name: "fileChange", input: { changes } })
        }
        return
      }
      case "thread/tokenUsage/updated":
        this.lastUsage = (params as { tokenUsage: { last: CodexTokenUsage } }).tokenUsage.last
        return
      case "account/rateLimits/updated": {
        const merged = mergeCodexRateLimits(
          this.rateLimits,
          (params as { rateLimits: CodexRateLimitSnapshot }).rateLimits,
        )
        if (!merged) return
        this.rateLimits = merged
        onRateLimit(codexRateLimit(merged))
        return
      }
      case "turn/completed":
        this.complete((params as { turn: CodexTurn }).turn)
        return
      default:
        return
    }
  }

  private complete(turn: CodexTurn): void {
    this.activeTurnId = null
    this.completedTurnIds.add(turn.id)
    const usage = this.lastUsage
    let result: WorkerTurnResult["result"] = "completed"
    let error: WorkerTurnResult["error"] = null
    if (turn.status === "interrupted") result = "interrupted"
    else if (turn.status === "failed") {
      result = "failed"
      const message = turn.error?.message ?? "Codex turn failed"
      error = isCodexUsageLimit(turn.error?.codexErrorInfo)
        ? {
            code: "Worker.UsageLimit",
            message,
            resetsAt: this.rateLimits ? codexRateLimit(this.rateLimits).resetsAt : null,
          }
        : { code: "Worker.Failed", message, resetsAt: null }
    }
    this.context.handlers.onTurnEnd({
      result,
      summary: error ? error.message : this.lastText,
      error,
      usage: usage
        ? {
            inputTokens: usage.inputTokens,
            outputTokens: usage.outputTokens,
            cacheReadInputTokens: usage.cachedInputTokens,
            cacheCreationInputTokens: usage.cacheWriteInputTokens ?? 0,
          }
        : emptyUsage(),
      costUsd: null,
      durationMs: turn.durationMs ?? 0,
    })
  }
}

export const codexProvider: WorkerProvider = {
  agent: "codex",
  homeVariable: "CODEX_HOME",
  recordExclude: [AUTH_FILE, "skills"],
  skillsDir: (home) => join(home, "skills"),
  installCredential: async (
    home,
    credential: WorkerCredential,
  ): Promise<Record<string, string>> => {
    switch (credential.kind) {
      case "codex_auth_json":
        await writeFile(join(home, AUTH_FILE), credential.value, { mode: 0o600 })
        return {}
      case "openai_api_key":
        return { OPENAI_API_KEY: credential.value }
      default:
        throw new Error(`Codex cannot use a ${credential.kind} credential`)
    }
  },
  readCredential: async (home, current) =>
    current.kind === "codex_auth_json"
      ? { kind: current.kind, value: await readFile(join(home, AUTH_FILE), "utf8") }
      : null,
  create: (context) => new CodexWorker(context),
}
