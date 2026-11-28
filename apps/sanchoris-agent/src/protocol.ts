import { z } from "zod"

export const PROTOCOL_VERSION = 1

const threadId = z.string().min(1)
const seq = z.number().int().positive()
const json = z.json()

export const InputType = z.enum(["string", "number", "boolean", "array", "object"])
export type InputType = z.infer<typeof InputType>

export const ErrorBody = z.strictObject({ code: z.string().min(1), message: z.string() })
export type ErrorBody = z.infer<typeof ErrorBody>

export const WorkerCredential = z.strictObject({
  kind: z.enum([
    "claude_oauth_token",
    "claude_credentials_json",
    "anthropic_api_key",
    "codex_auth_json",
    "openai_api_key",
  ]),
  value: z.string().min(1),
})
export type WorkerCredential = z.infer<typeof WorkerCredential>

export const Agent = z.enum(["claude", "codex"])
export type Agent = z.infer<typeof Agent>

const Repo = z.strictObject({
  name: z.string().regex(/^[A-Za-z0-9._-]+$/),
  repo: z.string().regex(/^github\.com\/[^/\s]+\/[^/\s]+$/),
  branch: z.string().min(1),
})

const Script = z.strictObject({
  name: z.string().min(1),
  command: z.string().min(1),
  timeoutSeconds: z.number().int().positive(),
  runOnCreate: z.boolean(),
  async: z.boolean(),
})
export type Script = z.infer<typeof Script>

export const SkillEntry = z.strictObject({
  name: z.string().regex(/^[a-z0-9][a-z0-9-]*$/),
  predictable: z.boolean(),
  inputs: z.record(z.string(), InputType),
})
export type SkillEntry = z.infer<typeof SkillEntry>

const Usage = z.strictObject({
  inputTokens: z.number().int().nonnegative(),
  outputTokens: z.number().int().nonnegative(),
  cacheReadInputTokens: z.number().int().nonnegative(),
  cacheCreationInputTokens: z.number().int().nonnegative(),
})
export type Usage = z.infer<typeof Usage>

export const RepoCommit = z.strictObject({
  repo: z.string().min(1),
  branch: z.string().min(1),
  commit: z.string().regex(/^[0-9a-f]{40}$/),
  pushed: z.boolean(),
})
export type RepoCommit = z.infer<typeof RepoCommit>

const RateLimitWindow = z.strictObject({
  utilization: z.number().nullable(),
  resetsAt: z.number().nullable(),
  status: z.string().nullable(),
})

export const TurnError = z.strictObject({
  code: z.enum(["Worker.Failed", "Worker.UsageLimit"]),
  message: z.string(),
  resetsAt: z.number().nullable(),
})
export type TurnError = z.infer<typeof TurnError>

// Control plane to agent. Every message but `ack` and `close` comes from the thread's outbox and
// carries its `seq`.

const Credentials = z.strictObject({
  type: z.literal("credentials"),
  threadId,
  seq,
  threadToken: z.string().min(1),
  threadTokenExpiresAt: z.iso.datetime(),
  github: z.strictObject({ token: z.string().min(1), expiresAt: z.iso.datetime() }),
  recordUpload: z.strictObject({
    url: z.url(),
    fields: z.record(z.string(), z.string()),
    keyPrefix: z.string().min(1),
    expiresAt: z.iso.datetime(),
  }),
})

const Start = z.strictObject({
  type: z.literal("start"),
  threadId,
  seq,
  thread: z.strictObject({ name: z.string().min(1), title: z.string() }),
  project: z.strictObject({
    name: z.string().min(1),
    repos: z.array(Repo),
    scripts: z.array(Script),
  }),
  worker: z.strictObject({ agent: Agent, model: z.string().min(1), credential: WorkerCredential }),
  gitAuthor: z.strictObject({ name: z.string().min(1), email: z.string().min(1) }),
  skills: z.strictObject({
    url: z.url().nullable(),
    sha256: z
      .string()
      .regex(/^[0-9a-f]{64}$/)
      .nullable(),
    entries: z.array(SkillEntry),
  }),
  record: z
    .strictObject({
      url: z.url(),
      sha256: z.string().regex(/^[0-9a-f]{64}$/),
      sessionId: z.string().min(1),
      turn: z.number().int().nonnegative(),
    })
    .nullable(),
  instruction: z.string().nullable(),
  heartbeatSeconds: z.number().int().positive(),
})

const Input = z.strictObject({
  type: z.literal("input"),
  threadId,
  seq,
  inputId: z.string().min(1),
  text: z.string().min(1),
  delivery: z.enum(["steer", "turn"]),
})

const Interrupt = z.strictObject({ type: z.literal("interrupt"), threadId, seq })
const Shutdown = z.strictObject({ type: z.literal("shutdown"), threadId, seq, reason: z.string() })
const Compact = z.strictObject({ type: z.literal("compact"), threadId, seq })

const Exec = z.strictObject({
  type: z.literal("exec"),
  threadId,
  seq,
  idempotencyKey: z.string().min(1),
  skill: z.strictObject({ name: z.string().min(1), runId: z.number().int().positive() }),
  run: z.string().min(1),
  stdin: z.string(),
  timeoutSeconds: z.number().int().positive(),
})

const ScriptMessage = z.strictObject({
  type: z.literal("script"),
  threadId,
  seq,
  scriptRunId: z.string().min(1),
  name: z.string().min(1),
})

const Response = <T extends "skill_response" | "thread_response">(type: T) =>
  z.union([
    z.strictObject({
      type: z.literal(type),
      threadId,
      seq,
      requestId: z.string().min(1),
      ok: z.literal(true),
      result: json,
    }),
    z.strictObject({
      type: z.literal(type),
      threadId,
      seq,
      requestId: z.string().min(1),
      ok: z.literal(false),
      error: ErrorBody,
    }),
  ])

const ContinueOutput = z.strictObject({
  type: z.literal("continue_output"),
  threadId,
  seq,
  runId: z.number().int().positive(),
  output: json,
})

const Ack = z.strictObject({
  type: z.literal("ack"),
  threadId,
  seq: z.number().int().nonnegative(),
})
const Close = z.strictObject({
  type: z.literal("close"),
  threadId,
  code: z.number().int().min(4000).max(4999),
  reason: z.string(),
})

export const InboundMessage = z.union([
  Credentials,
  Start,
  Input,
  Interrupt,
  Shutdown,
  Compact,
  Exec,
  ScriptMessage,
  Response("skill_response"),
  Response("thread_response"),
  ContinueOutput,
  Ack,
  Close,
])
export type InboundMessage = z.infer<typeof InboundMessage>
export type CredentialsMessage = z.infer<typeof Credentials>
export type StartMessage = z.infer<typeof Start>
export type InputMessage = z.infer<typeof Input>
export type ExecMessage = z.infer<typeof Exec>
export type ScriptRequestMessage = z.infer<typeof ScriptMessage>
export type ContinueOutputMessage = z.infer<typeof ContinueOutput>
export type SkillResponseMessage = z.infer<ReturnType<typeof Response<"skill_response">>>
export type ThreadResponseMessage = z.infer<ReturnType<typeof Response<"thread_response">>>

// Agent to control plane. Every message but `hello` and `heartbeat` carries the agent's `seq`,
// which the control plane acknowledges with `ack`.

const Hello = z.strictObject({
  type: z.literal("hello"),
  threadId,
  protocolVersion: z.literal(PROTOCOL_VERSION),
  instanceId: z.uuid(),
  started: z.boolean(),
  lastSeq: z.number().int().nonnegative(),
})

const Heartbeat = z.strictObject({
  type: z.literal("heartbeat"),
  threadId,
  turnRunning: z.boolean(),
  output: z.boolean(),
})

const Ready = z.strictObject({
  type: z.literal("ready"),
  threadId,
  seq,
  agent: Agent,
  sessionId: z.string().min(1),
})

const Event = z.strictObject({
  type: z.literal("event"),
  threadId,
  seq,
  kind: z.enum(["text", "thinking", "tool_use", "tool_result", "log", "system"]),
  data: z.record(z.string(), json),
})

const TurnEnd = z.strictObject({
  type: z.literal("turn_end"),
  threadId,
  seq,
  turn: z.number().int().positive(),
  result: z.enum(["completed", "failed", "interrupted"]),
  summary: z.string(),
  error: TurnError.nullable(),
  usage: Usage,
  costUsd: z.number().nullable(),
  durationMs: z.number().int().nonnegative(),
  commits: z.array(RepoCommit),
})

const RateLimit = z.strictObject({
  type: z.literal("rate_limit"),
  threadId,
  seq,
  status: z.string(),
  resetsAt: z.number().nullable(),
  windows: z.record(z.string(), RateLimitWindow),
})

const SkillRequest = z.strictObject({
  type: z.literal("skill_request"),
  threadId,
  seq,
  requestId: z.string().min(1),
  request: z.discriminatedUnion("action", [
    z.strictObject({ action: z.literal("run"), name: z.string().min(1), input: json }),
    z.strictObject({
      action: z.literal("continue"),
      runId: z.number().int().positive(),
      output: json,
    }),
    z.strictObject({ action: z.literal("await"), runId: z.number().int().positive() }),
    z.strictObject({ action: z.literal("status"), runId: z.number().int().positive() }),
  ]),
})

const ThreadRequest = z.strictObject({
  type: z.literal("thread_request"),
  threadId,
  seq,
  requestId: z.string().min(1),
  request: z.discriminatedUnion("action", [
    z.strictObject({
      action: z.literal("start"),
      message: z.string().min(1),
      project: z.string().min(1).nullable(),
      model: json.nullable(),
      commits: z.array(RepoCommit),
    }),
    z.strictObject({ action: z.literal("send"), name: z.string().min(1), message: z.string() }),
    z.strictObject({ action: z.literal("stop"), name: z.string().min(1) }),
    z.strictObject({ action: z.literal("status"), name: z.string().min(1) }),
  ]),
})

const ExecResult = z.strictObject({
  type: z.literal("exec_result"),
  threadId,
  seq,
  idempotencyKey: z.string().min(1),
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
  timedOut: z.boolean(),
})

const ScriptResult = z.strictObject({
  type: z.literal("script_result"),
  threadId,
  seq,
  scriptRunId: z.string().min(1),
  name: z.string().min(1),
  exitCode: z.number().int(),
  stderrTail: z.string(),
  timedOut: z.boolean(),
})

const CredentialUpdate = z.strictObject({
  type: z.literal("credential_update"),
  threadId,
  seq,
  credential: WorkerCredential,
})

const ThreadRecord = z.strictObject({
  type: z.literal("thread_record"),
  threadId,
  seq,
  turn: z.number().int().nonnegative(),
  sessionId: z.string().min(1),
  key: z.string().min(1),
  sha256: z.string().regex(/^[0-9a-f]{64}$/),
  bytes: z.number().int().nonnegative(),
})

const Stopping = z.strictObject({
  type: z.literal("stopping"),
  threadId,
  seq,
  commits: z.array(RepoCommit),
  recordKey: z.string().min(1).nullable(),
})

export const OutboundMessage = z.discriminatedUnion("type", [
  Hello,
  Heartbeat,
  Ready,
  Event,
  TurnEnd,
  RateLimit,
  SkillRequest,
  ThreadRequest,
  ExecResult,
  ScriptResult,
  CredentialUpdate,
  ThreadRecord,
  Stopping,
])
export type OutboundMessage = z.infer<typeof OutboundMessage>
export type SequencedMessage = Exclude<OutboundMessage, { type: "hello" | "heartbeat" }>
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never
export type SequencedPayload = DistributiveOmit<SequencedMessage, "threadId" | "seq">
export type SkillRequestBody = z.infer<typeof SkillRequest>["request"]
export type ThreadRequestBody = z.infer<typeof ThreadRequest>["request"]

// The Unix socket between the `sanchoris-agent` commands and the running agent: one JSON line
// each way.

export const SocketRequest = z.discriminatedUnion("kind", [
  z.strictObject({
    kind: z.literal("skill"),
    request: SkillRequest.shape.request,
  }),
  z.strictObject({
    kind: z.literal("thread"),
    request: z.discriminatedUnion("action", [
      z.strictObject({
        action: z.literal("start"),
        message: z.string().min(1),
        project: z.string().min(1).nullable(),
        model: json.nullable(),
      }),
      z.strictObject({ action: z.literal("send"), name: z.string().min(1), message: z.string() }),
      z.strictObject({ action: z.literal("stop"), name: z.string().min(1) }),
      z.strictObject({ action: z.literal("status"), name: z.string().min(1) }),
    ]),
  }),
])
export type SocketRequest = z.infer<typeof SocketRequest>

export const SocketResponse = z.union([
  z.strictObject({ ok: z.literal(true), result: json }),
  z.strictObject({ ok: z.literal(false), error: ErrorBody }),
])
export type SocketResponse = z.infer<typeof SocketResponse>
