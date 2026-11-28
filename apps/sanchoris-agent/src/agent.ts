import { chmod, mkdir, rm, writeFile } from "node:fs/promises"
import type { Server } from "node:net"
import { join } from "node:path"

import { type InboundCommand, ThreadChannel } from "./channel.ts"
import { type CommandResult, runBash, tail } from "./exec.ts"
import { deliver, type Turn } from "./generated/Sanchoris/AgentInput.ts"
import { cloneRepos, commitAndPush, gitEnv, writeGitHubToken } from "./git.ts"
import type { Logger } from "./log.ts"
import type {
  CredentialsMessage,
  ExecMessage,
  RepoCommit,
  Script,
  SocketRequest,
  SocketResponse,
  StartMessage,
  WorkerCredential,
} from "./protocol.ts"
import { downloadRecord, packRecord, sha256, unpackRecord, uploadRecord } from "./record.ts"
import { serveSocket } from "./socket.ts"
import { installSkills, placeWorkerSkills, validateSkillRun } from "./skills.ts"
import type { Worker, WorkerProvider, WorkerTurnResult } from "./worker.ts"

const REQUEST_TIMEOUT_MS = 60_000
const EXEC_OUTPUT_BYTES = 64 * 1024
const CREDENTIAL_POLL_MS = 60_000

export interface AgentOptions {
  threadId: string
  channelUrl: string
  bootstrapToken: string
  workspaceDir: string
  skillsDir: string
  stateDir: string
  socketPath: string
  gitUrlPrefix: string
  /** The command line that runs `sanchoris-agent` (the node binary and the entry point). */
  command: readonly string[]
  env: NodeJS.ProcessEnv
  providers: (agent: StartMessage["worker"]["agent"]) => Promise<WorkerProvider>
  logger: Logger
  initialBackoffMs?: number
  maxBackoffMs?: number
}

interface Session {
  start: StartMessage
  provider: WorkerProvider
  worker: Worker
  home: string
  sessionId: string
  credential: WorkerCredential
}

/** The program in a Workspace: the Thread channel on one side, the Worker on the other. */
export class Agent {
  private readonly options: AgentOptions
  private readonly channel: ThreadChannel
  private readonly exited: Promise<number>
  private exit!: (code: number) => void
  private server: Server | null = null
  private session: Session | null = null
  private starting: Promise<void> | null = null
  private credentials: CredentialsMessage | null = null
  private credentialsReady!: () => void
  private readonly credentialsArrived = new Promise<void>((resolve) => {
    this.credentialsReady = resolve
  })
  private turn: Turn = "idle"
  private turnNumber = 0
  private turnFinished: Promise<void> = Promise.resolve()
  private savedRecord: { turn: number; key: string } | null = null
  private queued: string[] = []
  private compactQueued = false
  private output = false
  private stopping = false
  private readonly execs = new Map<string, Promise<CommandResult> | CommandResult>()
  private readonly continueOutputs = new Map<number, unknown>()
  private readonly awaiting = new Map<number, (output: unknown) => void>()
  private timers: NodeJS.Timeout[] = []

  constructor(options: AgentOptions) {
    this.options = options
    this.exited = new Promise((resolve) => {
      this.exit = resolve
    })
    this.channel = new ThreadChannel({
      url: options.channelUrl,
      bootstrapToken: options.bootstrapToken,
      threadId: options.threadId,
      logger: options.logger,
      started: () => this.starting !== null,
      onCommand: (message) => this.dispatch(message),
      onCredentials: (message) => void this.applyCredentials(message),
      onFatal: (error) => {
        options.logger("error", "thread channel failed", { error: error.message })
        void this.finish(1)
      },
      initialBackoffMs: options.initialBackoffMs ?? 500,
      maxBackoffMs: options.maxBackoffMs ?? 30_000,
    })
  }

  /** Runs until `shutdown`, SIGTERM or a fatal error, and returns the exit code. */
  async run(): Promise<number> {
    await mkdir(this.options.stateDir, { recursive: true, mode: 0o700 })
    await this.writeCommandWrapper()
    this.server = await serveSocket(this.options.socketPath, (request) =>
      this.handleSocket(request),
    )
    this.channel.connect()
    return this.exited
  }

  /** SIGTERM: interrupt the turn, push the thread branches, save the record, report `stopping`. */
  async terminate(): Promise<void> {
    if (this.stopping) return
    const { commits, recordKey } = await this.wrapUp()
    this.channel.send({ type: "stopping", commits, recordKey })
    await this.finish(0)
  }

  private get binDir(): string {
    return join(this.options.stateDir, "bin")
  }

  private async writeCommandWrapper(): Promise<void> {
    await mkdir(this.binDir, { recursive: true })
    const quote = (value: string) => `'${value.replaceAll("'", `'\\''`)}'`
    await writeFile(
      join(this.binDir, "sanchoris-agent"),
      [
        "#!/bin/sh",
        `export SANCHORIS_STATE_DIR=${quote(this.options.stateDir)}`,
        `export SANCHORIS_SOCKET=${quote(this.options.socketPath)}`,
        `exec ${this.options.command.map(quote).join(" ")} "$@"`,
        "",
      ].join("\n"),
    )
    await chmod(join(this.binDir, "sanchoris-agent"), 0o755)
  }

  /** The environment of the Worker and the commands: no Thread channel token. */
  private commandEnv(extra: Record<string, string> = {}): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = { ...this.options.env }
    delete env.SANCHORIS_BOOTSTRAP_TOKEN
    return {
      ...env,
      ...gitEnv(this.options.stateDir),
      PATH: `${this.binDir}:${env.PATH ?? "/usr/bin:/bin"}`,
      SANCHORIS_STATE_DIR: this.options.stateDir,
      SANCHORIS_SOCKET: this.options.socketPath,
      SANCHORIS_WORKSPACE_DIR: this.options.workspaceDir,
      SANCHORIS_SKILLS_DIR: this.options.skillsDir,
      ...(this.session ? { SANCHORIS_THREAD_NAME: this.session.start.thread.name } : {}),
      ...extra,
    }
  }

  private async applyCredentials(message: CredentialsMessage): Promise<void> {
    this.credentials = message
    await writeGitHubToken({
      stateDir: this.options.stateDir,
      helper: join(this.binDir, "sanchoris-agent"),
      token: message.github.token,
    })
    this.credentialsReady()
  }

  private dispatch(message: InboundCommand): void {
    switch (message.type) {
      case "start":
        if (this.starting === null)
          this.starting = this.startSession(message).catch((error: Error) => this.fatal(error))
        return
      case "input":
        void this.afterStart(() => this.input(message.text, message.delivery))
        return
      case "interrupt":
        void this.afterStart(async () => {
          if (this.turn !== "idle") await this.session?.worker.interrupt()
        })
        return
      case "compact":
        void this.afterStart(async () => {
          if (this.turn === "idle") this.beginCompact()
          else this.compactQueued = true
        })
        return
      case "shutdown":
        void this.shutdown()
        return
      case "exec":
        void this.afterStart(() => this.exec(message))
        return
      case "script":
        void this.afterStart(async () => {
          const script = this.session?.start.project.scripts.find(
            (candidate) => candidate.name === message.name,
          )
          if (!script) {
            this.channel.send({
              type: "script_result",
              scriptRunId: message.scriptRunId,
              name: message.name,
              exitCode: 127,
              stderrTail: `No script named ${message.name} in project ${this.session?.start.project.name}`,
              timedOut: false,
            })
            return
          }
          await this.runScript(script, message.scriptRunId)
        })
        return
      case "continue_output": {
        const waiter = this.awaiting.get(message.runId)
        if (waiter) {
          this.awaiting.delete(message.runId)
          waiter(message.output)
        } else this.continueOutputs.set(message.runId, message.output)
        return
      }
    }
  }

  private async afterStart(action: () => Promise<void>): Promise<void> {
    if (this.starting === null) {
      this.fatal(new Error("Received a command before start"))
      return
    }
    await this.starting
    if (this.session) await action().catch((error: Error) => this.fatal(error))
  }

  private async startSession(start: StartMessage): Promise<void> {
    await this.credentialsArrived
    const provider = await this.options.providers(start.worker.agent)
    const home = join(this.options.stateDir, "home", provider.agent)
    await mkdir(home, { recursive: true, mode: 0o700 })
    if (start.record)
      await unpackRecord(home, await downloadRecord(start.record.url, start.record.sha256))
    await cloneRepos({
      workspaceDir: this.options.workspaceDir,
      repos: start.project.repos,
      urlPrefix: this.options.gitUrlPrefix,
      env: this.commandEnv(),
    })
    await installSkills(
      this.options.skillsDir,
      start.skills.url && start.skills.sha256
        ? { url: start.skills.url, sha256: start.skills.sha256 }
        : null,
    )
    await placeWorkerSkills(this.options.skillsDir, provider.skillsDir(home), start.skills.entries)
    const credentialEnv = await provider.installCredential(home, start.worker.credential)
    const worker = provider.create({
      model: start.worker.model,
      workspaceDir: this.options.workspaceDir,
      home,
      env: this.commandEnv({
        SANCHORIS_THREAD_NAME: start.thread.name,
        [provider.homeVariable]: home,
        ...credentialEnv,
      }),
      logger: this.options.logger,
      handlers: {
        onTurnStarted: () => {
          if (this.turn === "preparing") this.turn = "running"
        },
        onEvent: (kind, data) => {
          this.output = true
          this.channel.send({ type: "event", kind, data: data as Record<string, never> })
        },
        onTurnEnd: (result) => {
          this.turnFinished = this.endTurn(result).catch((error: Error) => this.fatal(error))
        },
        onRateLimit: (rateLimit) => this.channel.send({ type: "rate_limit", ...rateLimit }),
        onFatal: (error) => this.fatal(error),
      },
    })
    const { sessionId } = await worker.start(start.record?.sessionId ?? null)
    this.turnNumber = start.record?.turn ?? 0
    this.session = { start, provider, worker, home, sessionId, credential: start.worker.credential }
    this.channel.send({ type: "ready", agent: start.worker.agent, sessionId })
    this.timers.push(
      setInterval(() => {
        this.channel.heartbeat(this.turn === "running", this.output)
        this.output = false
      }, start.heartbeatSeconds * 1000),
      setInterval(() => void this.checkCredential(), CREDENTIAL_POLL_MS),
    )
    const scripts = start.project.scripts.filter((script) => script.runOnCreate)
    for (const script of scripts.filter((candidate) => candidate.async))
      void this.runScript(script, `create:${script.name}`)
    let failed = false
    for (const script of scripts.filter((candidate) => !candidate.async))
      if ((await this.runScript(script, `create:${script.name}`)).exitCode !== 0) failed = true
    if (start.instruction !== null) {
      if (failed) this.queued.push(start.instruction)
      else this.beginTurn(start.instruction)
    }
  }

  private async input(text: string, delivery: "steer" | "turn"): Promise<void> {
    const session = this.session
    if (!session) return
    switch (deliver(this.turn, delivery, session.worker.steerable)) {
      case "startTurn":
        this.beginTurn([...this.queued.splice(0), text].join("\n\n"))
        return
      case "steer":
        if (!(await session.worker.steer(text))) this.queued.push(text)
        return
      case "queue":
        this.queued.push(text)
        return
    }
  }

  private beginTurn(text: string): void {
    this.turn = "preparing"
    this.session?.worker.turn(text)
  }

  private beginCompact(): void {
    this.compactQueued = false
    this.turn = "preparing"
    this.session?.worker.compact()
  }

  private async endTurn(result: WorkerTurnResult): Promise<void> {
    const session = this.session
    if (!session) return
    this.turn = "preparing"
    this.turnNumber += 1
    const turn = this.turnNumber
    const commits = await this.commit(`sanchoris: turn ${turn} of ${session.start.thread.name}`)
    await this.saveRecord(turn)
    this.channel.send({ type: "turn_end", turn, ...result, commits })
    await this.checkCredential()
    this.turn = "idle"
    if (this.stopping) return
    if (this.compactQueued) this.beginCompact()
    else if (this.queued.length > 0) this.beginTurn(this.queued.splice(0).join("\n\n"))
  }

  private async commit(message: string): Promise<RepoCommit[]> {
    const session = this.session
    if (!session) return []
    return commitAndPush({
      workspaceDir: this.options.workspaceDir,
      repos: session.start.project.repos,
      env: this.commandEnv(),
      author: session.start.gitAuthor,
      message,
    })
  }

  /** Saves the record of `turn` once; a turn that the record of `start` ends is not saved again. */
  private async saveRecord(turn: number): Promise<string | null> {
    const session = this.session
    const upload = this.credentials?.recordUpload
    if (!session || !upload) return null
    if (this.savedRecord?.turn === turn) return this.savedRecord.key
    if (turn === (session.start.record?.turn ?? 0)) return null
    const bytes = await packRecord(session.home, session.provider.recordExclude)
    const key = `${upload.keyPrefix}${String(turn).padStart(6, "0")}.tar.gz`
    await uploadRecord(upload, key, bytes)
    this.channel.send({
      type: "thread_record",
      turn,
      sessionId: session.sessionId,
      key,
      sha256: sha256(bytes),
      bytes: bytes.length,
    })
    this.savedRecord = { turn, key }
    return key
  }

  private async checkCredential(): Promise<void> {
    const session = this.session
    if (!session) return
    const credential = await session.provider.readCredential(session.home, session.credential)
    if (!credential || credential.value === session.credential.value) return
    session.credential = credential
    this.channel.send({ type: "credential_update", credential })
  }

  private async exec(message: ExecMessage): Promise<void> {
    const known = this.execs.get(message.idempotencyKey)
    if (known instanceof Promise) return
    const result =
      known ??
      (await (() => {
        const running = runBash({
          command: message.run,
          cwd: this.options.workspaceDir,
          env: this.commandEnv({
            SANCHORIS_SKILL_RUN_ID: String(message.skill.runId),
            SANCHORIS_SKILL_DIR: join(this.options.skillsDir, message.skill.name),
          }),
          stdin: message.stdin,
          timeoutSeconds: message.timeoutSeconds,
        })
        this.execs.set(message.idempotencyKey, running)
        return running
      })())
    this.execs.set(message.idempotencyKey, result)
    this.channel.send({
      type: "exec_result",
      idempotencyKey: message.idempotencyKey,
      exitCode: result.exitCode,
      stdout: tail(result.stdout, EXEC_OUTPUT_BYTES),
      stderr: tail(result.stderr, EXEC_OUTPUT_BYTES),
      timedOut: result.timedOut,
    })
  }

  private async runScript(script: Script, scriptRunId: string): Promise<CommandResult> {
    const result = await runBash({
      command: script.command,
      cwd: this.options.workspaceDir,
      env: this.commandEnv(),
      stdin: "",
      timeoutSeconds: script.timeoutSeconds,
    })
    this.channel.send({
      type: "event",
      kind: "log",
      data: {
        script: script.name,
        scriptRunId,
        stdout: tail(result.stdout),
        stderr: tail(result.stderr),
      },
    })
    this.channel.send({
      type: "script_result",
      scriptRunId,
      name: script.name,
      exitCode: result.exitCode,
      stderrTail: tail(result.stderr),
      timedOut: result.timedOut,
    })
    return result
  }

  private async handleSocket(request: SocketRequest): Promise<SocketResponse> {
    if (this.starting === null)
      return { ok: false, error: { code: "not_started", message: "The thread has not started" } }
    await this.starting
    const session = this.session
    if (!session)
      return { ok: false, error: { code: "not_started", message: "The thread has not started" } }
    if (request.kind === "skill") {
      const body = request.request
      if (body.action === "run") {
        const problem = validateSkillRun(session.start.skills.entries, body.name, body.input)
        if (problem) return { ok: false, error: problem }
      }
      if (body.action === "await") {
        if (this.continueOutputs.has(body.runId)) {
          const output = this.continueOutputs.get(body.runId)
          this.continueOutputs.delete(body.runId)
          return { ok: true, result: output as never }
        }
        const output = new Promise<unknown>((resolve) => this.awaiting.set(body.runId, resolve))
        const reply = await this.channel.request(
          (requestId) => ({ type: "skill_request", requestId, request: body }),
          REQUEST_TIMEOUT_MS,
        )
        if (!reply.ok) {
          this.awaiting.delete(body.runId)
          return { ok: false, error: reply.error }
        }
        return { ok: true, result: (await output) as never }
      }
      const reply = await this.channel.request(
        (requestId) => ({ type: "skill_request", requestId, request: body }),
        REQUEST_TIMEOUT_MS,
      )
      return reply.ok ? { ok: true, result: reply.result } : { ok: false, error: reply.error }
    }
    const body = request.request
    const commits =
      body.action === "start"
        ? await this.commit(`sanchoris: before starting a child of ${session.start.thread.name}`)
        : []
    const reply = await this.channel.request(
      (requestId) => ({
        type: "thread_request",
        requestId,
        request: body.action === "start" ? { ...body, commits } : body,
      }),
      REQUEST_TIMEOUT_MS,
    )
    return reply.ok ? { ok: true, result: reply.result } : { ok: false, error: reply.error }
  }

  private async wrapUp(): Promise<{ commits: RepoCommit[]; recordKey: string | null }> {
    this.stopping = true
    if (this.starting) await this.starting
    const session = this.session
    if (!session) return { commits: [], recordKey: null }
    if (this.turn !== "idle") {
      await session.worker.interrupt().catch(() => {})
      await this.waitForIdle()
    }
    const commits = await this.commit(`sanchoris: stop ${session.start.thread.name}`)
    const recordKey = await this.saveRecord(this.turnNumber)
    return { commits, recordKey }
  }

  private async waitForIdle(): Promise<void> {
    const idle = () => this.turn === "idle"
    for (let i = 0; i < 600 && !idle(); i += 1) {
      await this.turnFinished
      if (!idle()) await new Promise((resolve) => setTimeout(resolve, 50))
    }
  }

  private async shutdown(): Promise<void> {
    if (this.stopping) return
    await this.wrapUp()
    await this.finish(0)
  }

  private fatal(error: Error): void {
    this.options.logger("error", "agent failed", { error: error.message, stack: error.stack })
    void this.finish(1)
  }

  private finishing: Promise<void> | null = null

  private async finish(code: number): Promise<void> {
    if (this.finishing) return this.finishing
    this.finishing = (async () => {
      this.stopping = true
      for (const timer of this.timers) clearInterval(timer)
      if (code === 0) await this.channel.drain(10_000).catch(() => {})
      await this.session?.worker.close().catch(() => {})
      this.channel.close()
      this.server?.close()
      await rm(this.options.socketPath, { force: true })
      this.exit(code)
    })()
    return this.finishing
  }
}
