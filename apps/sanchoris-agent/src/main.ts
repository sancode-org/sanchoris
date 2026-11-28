#!/usr/bin/env node
import { readFile } from "node:fs/promises"
import { homedir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { parseArgs } from "node:util"

import { Agent } from "./agent.ts"
import { stderrLogger } from "./log.ts"
import type { SocketRequest } from "./protocol.ts"
import { callSocket } from "./socket.ts"
import { requireEnv, type WorkerProvider } from "./worker.ts"

export const usage = `Usage:
  sanchoris-agent run
  sanchoris-agent skill run <name> --input '<json>'
  sanchoris-agent skill continue <id> --output '<json>'
  sanchoris-agent skill await
  sanchoris-agent skill status <id>
  sanchoris-agent thread start --message '<text>' [--project <name>] [--model '<json>']
  sanchoris-agent thread send <Name> --message '<text>'
  sanchoris-agent thread stop <Name>
  sanchoris-agent thread status <Name>
  sanchoris-agent git-credential <get|store|erase>
`

class UsageError extends Error {}

function parseJson(flag: string, value: string | undefined): unknown {
  if (value === undefined) throw new UsageError(`${flag} is required`)
  try {
    return JSON.parse(value) as unknown
  } catch {
    throw new UsageError(`${flag} must be JSON`)
  }
}

function runId(value: string | undefined): number {
  const id = Number(value)
  if (!Number.isSafeInteger(id) || id <= 0)
    throw new UsageError("The SkillRun ID must be a positive integer")
  return id
}

/** Turns the command line of a `skill` or `thread` command into a request to the agent. */
export function parseCommand(argv: readonly string[], env: NodeJS.ProcessEnv): SocketRequest {
  const { values, positionals } = parseArgs({
    args: [...argv],
    allowPositionals: true,
    options: {
      input: { type: "string" },
      output: { type: "string" },
      message: { type: "string" },
      project: { type: "string" },
      model: { type: "string" },
    },
  })
  const [group, action, target, ...rest] = positionals
  if (rest.length > 0) throw new UsageError(`Unexpected argument ${rest[0]}`)
  if (group === "skill") {
    switch (action) {
      case "run":
        if (target === undefined) throw new UsageError("skill run needs a skill name")
        return {
          kind: "skill",
          request: {
            action: "run",
            name: target,
            input: parseJson("--input", values.input ?? "{}") as never,
          },
        }
      case "continue":
        return {
          kind: "skill",
          request: {
            action: "continue",
            runId: runId(target),
            output: parseJson("--output", values.output) as never,
          },
        }
      case "await":
        return {
          kind: "skill",
          request: { action: "await", runId: runId(target ?? env.SANCHORIS_SKILL_RUN_ID) },
        }
      case "status":
        return { kind: "skill", request: { action: "status", runId: runId(target) } }
    }
  }
  if (group === "thread") {
    switch (action) {
      case "start":
        if (values.message === undefined) throw new UsageError("--message is required")
        return {
          kind: "thread",
          request: {
            action: "start",
            message: values.message,
            project: values.project ?? null,
            model:
              values.model === undefined ? null : (parseJson("--model", values.model) as never),
          },
        }
      case "send":
        if (target === undefined || values.message === undefined)
          throw new UsageError("thread send needs a thread Name and --message")
        return {
          kind: "thread",
          request: { action: "send", name: target, message: values.message },
        }
      case "stop":
      case "status":
        if (target === undefined) throw new UsageError(`thread ${action} needs a thread Name`)
        return { kind: "thread", request: { action, name: target } }
    }
  }
  throw new UsageError(`Unknown command: ${positionals.join(" ")}`)
}

async function readStdin(): Promise<string> {
  let text = ""
  for await (const chunk of process.stdin) text += String(chunk)
  return text
}

/** The git credential helper: answers `get` for https://github.com with the GitHub token. */
async function gitCredential(
  operation: string | undefined,
  env: NodeJS.ProcessEnv,
): Promise<number> {
  const input = await readStdin()
  if (operation !== "get") return 0
  const fields = new Map(
    input
      .split("\n")
      .filter((line) => line.includes("="))
      .map(
        (line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)] as const,
      ),
  )
  if (fields.get("protocol") !== "https" || fields.get("host") !== "github.com") return 0
  const token = await readFile(join(requireEnv(env, "SANCHORIS_STATE_DIR"), "github-token"), "utf8")
  process.stdout.write(`username=x-access-token\npassword=${token}\n`)
  return 0
}

async function providerFor(agent: string, env: NodeJS.ProcessEnv): Promise<WorkerProvider> {
  const override = env.SANCHORIS_WORKER_MODULE
  if (override) return ((await import(override)) as { default: WorkerProvider }).default
  switch (agent) {
    case "claude":
      return (await import("./workers/claude.ts")).claudeProvider
    case "codex":
      return (await import("./workers/codex.ts")).codexProvider
    default:
      throw new Error(`No Worker for ${agent}`)
  }
}

async function runAgent(env: NodeJS.ProcessEnv): Promise<number> {
  const bootstrapToken = requireEnv(env, "SANCHORIS_BOOTSTRAP_TOKEN")
  delete process.env.SANCHORIS_BOOTSTRAP_TOKEN
  const stateDir = env.SANCHORIS_STATE_DIR || join(homedir(), ".sanchoris-agent")
  const agentEnv = { ...env }
  delete agentEnv.SANCHORIS_BOOTSTRAP_TOKEN
  const agent = new Agent({
    threadId: requireEnv(env, "SANCHORIS_THREAD_ID"),
    channelUrl: requireEnv(env, "SANCHORIS_CHANNEL_URL"),
    bootstrapToken,
    workspaceDir: env.SANCHORIS_WORKSPACE_DIR || "/workspace",
    skillsDir: env.SANCHORIS_SKILLS_DIR || "/skills",
    stateDir,
    socketPath: env.SANCHORIS_SOCKET || join(stateDir, "agent.sock"),
    gitUrlPrefix: env.SANCHORIS_GIT_URL_PREFIX || "https://",
    command: [process.execPath, fileURLToPath(import.meta.url)],
    env: agentEnv,
    providers: (name) => providerFor(name, env),
    logger: stderrLogger,
  })
  process.once("SIGTERM", () => void agent.terminate())
  return agent.run()
}

export async function main(argv: readonly string[], env: NodeJS.ProcessEnv): Promise<number> {
  try {
    if (argv[0] === "run") return await runAgent(env)
    if (argv[0] === "git-credential") return await gitCredential(argv[1], env)
    if (argv.length === 0 || argv[0] === "--help") {
      process.stdout.write(usage)
      return argv.length === 0 ? 1 : 0
    }
    const response = await callSocket(requireEnv(env, "SANCHORIS_SOCKET"), parseCommand(argv, env))
    if (!response.ok) {
      process.stderr.write(`${response.error.message}\n`)
      return 1
    }
    process.stdout.write(`${JSON.stringify(response.result)}\n`)
    return 0
  } catch (error) {
    process.stderr.write(`${(error as Error).message}\n${error instanceof UsageError ? usage : ""}`)
    return 1
  }
}

if (import.meta.main) {
  const code = await main(process.argv.slice(2), process.env)
  if (process.argv[2] === "run") process.exit(code)
  process.exitCode = code
}
