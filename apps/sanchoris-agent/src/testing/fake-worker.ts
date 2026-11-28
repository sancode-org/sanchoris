import { randomUUID } from "node:crypto"
import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"

import { runBash } from "../exec.ts"
import { emptyUsage, type Worker, type WorkerContext, type WorkerProvider } from "../worker.ts"

interface Line {
  role: "user" | "assistant"
  text: string
}

/**
 * A Worker that follows the text it gets. Each line `$ <command>` runs with `bash -c` in the
 * workspace and adds its stdout to the reply, `recall` adds the first user message of the
 * conversation, `skills` lists the skills the Worker reads, and any other first line is echoed.
 * Steers that arrive during a turn are added to its reply.
 */
class FakeWorker implements Worker {
  readonly steerable = true
  private readonly context: WorkerContext
  private sessionId = ""
  private history: Line[] = []
  private chain: Promise<void> = Promise.resolve()
  private steers: string[] | null = null
  private interrupted = false

  constructor(context: WorkerContext) {
    this.context = context
  }

  async start(resumeSessionId: string | null): Promise<{ sessionId: string }> {
    this.sessionId = resumeSessionId ?? randomUUID()
    await mkdir(join(this.context.home, "sessions"), { recursive: true })
    if (resumeSessionId) {
      const text = await readFile(this.path(), "utf8")
      this.history = text
        .split("\n")
        .filter((line) => line !== "")
        .map((line) => JSON.parse(line) as Line)
    } else await writeFile(this.path(), "")
    return { sessionId: this.sessionId }
  }

  turn(text: string): void {
    this.enqueue(() => this.run(text))
  }

  async steer(text: string): Promise<boolean> {
    if (this.steers === null) return false
    this.steers.push(text)
    await this.append({ role: "user", text })
    return true
  }

  compact(): void {
    this.enqueue(async () => {
      this.context.handlers.onTurnStarted()
      this.history = [{ role: "assistant", text: `summary of ${this.history.length} lines` }]
      await writeFile(this.path(), `${JSON.stringify(this.history[0])}\n`)
      this.end("compacted", Date.now())
    })
  }

  async interrupt(): Promise<void> {
    this.interrupted = true
  }

  async close(): Promise<void> {
    await this.chain
  }

  private enqueue(task: () => Promise<void>): void {
    this.chain = this.chain.then(task).catch((error: Error) => this.context.handlers.onFatal(error))
  }

  private path(): string {
    return join(this.context.home, "sessions", `${this.sessionId}.jsonl`)
  }

  private async append(line: Line): Promise<void> {
    this.history.push(line)
    await appendFile(this.path(), `${JSON.stringify(line)}\n`)
  }

  private async run(text: string): Promise<void> {
    const started = Date.now()
    this.steers = []
    this.interrupted = false
    this.context.handlers.onTurnStarted()
    await this.append({ role: "user", text })
    const reply: string[] = []
    for (const line of text.split("\n")) {
      if (this.interrupted) break
      if (line.startsWith("$ ")) {
        this.context.handlers.onEvent("tool_use", {
          name: "Bash",
          input: { command: line.slice(2) },
        })
        const result = await runBash({
          command: line.slice(2),
          cwd: this.context.workspaceDir,
          env: this.context.env,
          stdin: "",
          timeoutSeconds: 60,
        })
        this.context.handlers.onEvent("tool_result", {
          exitCode: result.exitCode,
          stderr: result.stderr,
        })
        if (result.stdout.trim() !== "") reply.push(result.stdout.trim())
      } else if (line === "recall") {
        reply.push(
          `first: ${this.history.find((entry) => entry.role === "user")?.text.split("\n")[0]}`,
        )
      } else if (line === "skills") {
        const listing = await runBash({
          command: `ls "${join(this.context.home, "skills")}"`,
          cwd: this.context.workspaceDir,
          env: this.context.env,
          stdin: "",
          timeoutSeconds: 10,
        })
        reply.push(`skills: ${listing.stdout.trim().split("\n").join(",")}`)
      }
    }
    if (reply.length === 0) reply.push(`ok: ${text.split("\n")[0]}`)
    for (const steer of this.steers) reply.push(`steered: ${steer}`)
    this.steers = null
    const summary = reply.join("\n")
    await this.append({ role: "assistant", text: summary })
    this.context.handlers.onEvent("text", { text: summary })
    this.end(summary, started)
  }

  private end(summary: string, started: number): void {
    this.context.handlers.onTurnEnd({
      result: this.interrupted ? "interrupted" : "completed",
      summary,
      error: null,
      usage: { ...emptyUsage(), inputTokens: 1, outputTokens: 1 },
      costUsd: 0,
      durationMs: Date.now() - started,
    })
  }
}

const fakeProvider: WorkerProvider = {
  agent: "fake",
  homeVariable: "FAKE_WORKER_HOME",
  recordExclude: ["credential", "skills"],
  skillsDir: (home) => join(home, "skills"),
  installCredential: async (home, credential) => {
    await writeFile(join(home, "credential"), credential.value, { mode: 0o600 })
    return {}
  },
  readCredential: async (home, current) => ({
    kind: current.kind,
    value: await readFile(join(home, "credential"), "utf8"),
  }),
  create: (context) => new FakeWorker(context),
}

export default fakeProvider
