import { randomUUID } from "node:crypto"
import { readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"

import {
  query,
  type Query,
  type SDKMessage,
  type SDKRateLimitInfo,
  type SDKResultMessage,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk"

import { AsyncQueue } from "../async-queue.ts"
import type { WorkerCredential } from "../protocol.ts"
import type {
  Worker,
  WorkerContext,
  WorkerProvider,
  WorkerRateLimit,
  WorkerTurnResult,
} from "../worker.ts"

const MAX_TOOL_RESULT_CHARS = 4000
const CREDENTIALS_FILE = ".credentials.json"

type UnifiedWindow = { utilization?: number; resetsAt?: number; status?: string }

export function claudeRateLimit(
  info: SDKRateLimitInfo & { unifiedWindows?: Record<string, UnifiedWindow> },
): WorkerRateLimit {
  const toWindow = (window: UnifiedWindow) => ({
    utilization: window.utilization ?? null,
    resetsAt: window.resetsAt ?? null,
    status: window.status ?? null,
  })
  const windows: WorkerRateLimit["windows"] = {}
  if (info.unifiedWindows)
    for (const [name, window] of Object.entries(info.unifiedWindows))
      windows[name] = toWindow(window)
  else if (info.rateLimitType) windows[info.rateLimitType] = toWindow(info)
  return { status: info.status, resetsAt: info.resetsAt ?? null, windows }
}

function textOf(content: unknown): string {
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return ""
  return content
    .map((block: { type?: string; text?: string }) =>
      block.type === "text" ? (block.text ?? "") : "",
    )
    .join("")
}

function userMessage(text: string, priority?: "now"): SDKUserMessage {
  return {
    type: "user",
    message: { role: "user", content: [{ type: "text", text }] },
    parent_tool_use_id: null,
    ...(priority ? { priority } : {}),
  }
}

/**
 * One Claude Agent SDK `query()` in streaming-input mode per conversation, fed one user message
 * per turn (T3 Code `ClaudeAdapterV2.ts`).
 */
export class ClaudeWorker implements Worker {
  readonly steerable = true
  private readonly context: WorkerContext
  private readonly input = new AsyncQueue<SDKUserMessage>()
  private session: Query | null = null
  private consumed: Promise<void> = Promise.resolve()
  private inTurn = false
  private turnStarted = false
  private interrupting = false
  private closing = false
  private rejectedRateLimit: SDKRateLimitInfo | null = null

  constructor(context: WorkerContext) {
    this.context = context
  }

  async start(resumeSessionId: string | null): Promise<{ sessionId: string }> {
    const sessionId = resumeSessionId ?? randomUUID()
    const executable = this.context.env.SANCHORIS_CLAUDE_PATH
    this.session = query({
      prompt: this.input,
      options: {
        cwd: this.context.workspaceDir,
        model: this.context.model,
        env: this.context.env,
        ...(executable ? { pathToClaudeCodeExecutable: executable } : {}),
        ...(resumeSessionId ? { resume: sessionId } : { sessionId }),
        permissionMode: "bypassPermissions",
        allowDangerouslySkipPermissions: true,
        canUseTool: async (toolName, input) =>
          toolName === "AskUserQuestion"
            ? {
                behavior: "deny",
                message: "No one answers questions in this thread. Decide and go on.",
              }
            : { behavior: "allow", updatedInput: input },
        thinking: { type: "adaptive", display: "summarized" },
        settings: { showThinkingSummaries: true },
        settingSources: ["user", "project", "local"],
        systemPrompt: { type: "preset", preset: "claude_code" },
        stderr: (data) => {
          for (const line of data.split("\n"))
            if (line !== "") this.context.handlers.onEvent("log", { line })
        },
      },
    })
    this.consumed = this.consume(this.session)
    return { sessionId }
  }

  turn(text: string): void {
    this.inTurn = true
    this.turnStarted = false
    this.input.push(userMessage(text))
  }

  async steer(text: string): Promise<boolean> {
    if (!this.inTurn || !this.turnStarted) return false
    this.input.push(userMessage(text, "now"))
    return true
  }

  compact(): void {
    this.turn("/compact")
  }

  async interrupt(): Promise<void> {
    if (!this.session || !this.inTurn) return
    this.interrupting = true
    await this.session.interrupt()
  }

  async close(): Promise<void> {
    this.closing = true
    this.input.end()
    this.session?.close()
    await this.consumed
  }

  private async consume(session: Query): Promise<void> {
    try {
      for await (const message of session) this.handle(message)
      if (!this.closing)
        this.context.handlers.onFatal(new Error("Claude session ended unexpectedly"))
    } catch (error) {
      if (!this.closing) this.context.handlers.onFatal(error as Error)
    }
  }

  private handle(message: SDKMessage): void {
    const { onEvent, onRateLimit, onTurnEnd, onTurnStarted } = this.context.handlers
    if (this.inTurn && !this.turnStarted) {
      this.turnStarted = true
      onTurnStarted()
    }
    switch (message.type) {
      case "system":
        onEvent("system", { subtype: message.subtype })
        return
      case "assistant":
        if (message.parent_tool_use_id !== null) return
        for (const block of message.message.content) {
          if (block.type === "thinking" && block.thinking !== "")
            onEvent("thinking", { text: block.thinking })
          else if (block.type === "text") onEvent("text", { text: block.text })
          else if (block.type === "tool_use")
            onEvent("tool_use", {
              id: block.id,
              name: block.name,
              input: block.input as Record<string, unknown>,
            })
        }
        return
      case "user": {
        if (message.parent_tool_use_id !== null || !Array.isArray(message.message.content)) return
        for (const block of message.message.content) {
          if (block.type !== "tool_result") continue
          onEvent("tool_result", {
            toolUseId: block.tool_use_id,
            isError: block.is_error ?? false,
            content: textOf(block.content).slice(0, MAX_TOOL_RESULT_CHARS),
          })
        }
        return
      }
      case "rate_limit_event":
        if (message.rate_limit_info.status === "rejected")
          this.rejectedRateLimit = message.rate_limit_info
        onRateLimit(claudeRateLimit(message.rate_limit_info))
        return
      case "result":
        this.inTurn = false
        onTurnEnd(this.turnResult(message))
        return
      default:
        return
    }
  }

  private turnResult(message: SDKResultMessage): WorkerTurnResult {
    const failed = message.subtype !== "success" || message.is_error
    const summary = message.subtype === "success" ? message.result : message.errors.join("\n")
    const rejected = this.rejectedRateLimit
    const interrupted = this.interrupting
    this.rejectedRateLimit = null
    this.interrupting = false
    return {
      result: interrupted ? "interrupted" : failed ? "failed" : "completed",
      summary: interrupted && message.subtype !== "success" ? "" : summary,
      error:
        interrupted || !failed
          ? null
          : rejected
            ? { code: "Worker.UsageLimit", message: summary, resetsAt: rejected.resetsAt ?? null }
            : { code: "Worker.Failed", message: summary, resetsAt: null },
      usage: {
        inputTokens: message.usage.input_tokens,
        outputTokens: message.usage.output_tokens,
        cacheReadInputTokens: message.usage.cache_read_input_tokens ?? 0,
        cacheCreationInputTokens: message.usage.cache_creation_input_tokens ?? 0,
      },
      costUsd: message.total_cost_usd,
      durationMs: message.duration_ms,
    }
  }
}

export const claudeProvider: WorkerProvider = {
  agent: "claude",
  homeVariable: "CLAUDE_CONFIG_DIR",
  recordExclude: [CREDENTIALS_FILE, "skills"],
  skillsDir: (home) => join(home, "skills"),
  installCredential: async (
    home,
    credential: WorkerCredential,
  ): Promise<Record<string, string>> => {
    switch (credential.kind) {
      case "claude_oauth_token":
        return { CLAUDE_CODE_OAUTH_TOKEN: credential.value }
      case "anthropic_api_key":
        return { ANTHROPIC_API_KEY: credential.value }
      case "claude_credentials_json":
        await writeFile(join(home, CREDENTIALS_FILE), credential.value, { mode: 0o600 })
        return {}
      default:
        throw new Error(`Claude cannot use a ${credential.kind} credential`)
    }
  },
  readCredential: async (home, current) =>
    current.kind === "claude_credentials_json"
      ? { kind: current.kind, value: await readFile(join(home, CREDENTIALS_FILE), "utf8") }
      : null,
  create: (context) => new ClaudeWorker(context),
}
