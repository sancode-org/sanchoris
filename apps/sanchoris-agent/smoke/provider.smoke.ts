// The checks of M3 against a real provider (architecture.md「マイルストーンの中で確かめること」):
// a conversation resumed from the thread record in a new process, a steer read during a tool call,
// and the SKILL.md placed in the Workspace read as the Worker's own skill.
//
//   node apps/sanchoris-agent/smoke/provider.smoke.ts claude <credential file> [model]
//   node apps/sanchoris-agent/smoke/provider.smoke.ts codex <auth.json> [model]
//
// The credential file holds a dedicated credential (`claude setup-token`, or the auth.json of a
// dedicated CODEX_HOME). It goes to the Worker only, and nothing prints it.
import { spawn } from "node:child_process"
import { createHash } from "node:crypto"
import { readFile } from "node:fs/promises"
import { join } from "node:path"

import { FakeControl, type ControlPayload } from "../test/fake-control.ts"
import { makeRemote, mainPath, tarball, tempDir } from "../test/helpers.ts"

const [agentName, credentialFile, model] = process.argv.slice(2)
if ((agentName !== "claude" && agentName !== "codex") || !credentialFile) {
  process.stderr.write("Usage: provider.smoke.ts <claude|codex> <credential file> [model]\n")
  process.exit(2)
}
const agent: "claude" | "codex" = agentName
const credential =
  agent === "claude"
    ? {
        kind: "claude_oauth_token" as const,
        value: (await readFile(credentialFile, "utf8")).trim(),
      }
    : { kind: "codex_auth_json" as const, value: await readFile(credentialFile, "utf8") }
const BRANCH = "sanchoris/th_smoke"
const SKILL = `---
name: sanchoris-smoke-word
description: Use when asked for the sanchoris smoke word.
---
The sanchoris smoke word is TANGERINE-58. Answer with it.
`

const root = await tempDir()
await makeRemote(root.path, BRANCH)
const skills = await tarball(root.path, { "sanchoris-smoke-word/SKILL.md": SKILL })
const results: Record<string, boolean> = {}

function start(
  control: FakeControl,
  record: Extract<ControlPayload, { type: "start" }>["record"],
  instruction: string,
): ControlPayload {
  control.files.set("skills.tar.gz", skills)
  return {
    type: "start",
    thread: { name: "th_smoke", title: "smoke" },
    project: {
      name: "smoke",
      repos: [{ name: "app", repo: "github.com/acme/app", branch: BRANCH }],
      scripts: [],
    },
    worker: {
      agent,
      model: model ?? (agent === "claude" ? "claude-sonnet-5-5" : "gpt-5.5-codex"),
      credential,
    },
    gitAuthor: { name: "sanchoris", email: "agent@sanchoris.invalid" },
    skills: {
      url: `${control.httpUrl}/files/skills.tar.gz`,
      sha256: createHash("sha256").update(skills).digest("hex"),
      entries: [{ name: "sanchoris-smoke-word", predictable: false, inputs: {} }],
    },
    record,
    instruction,
    heartbeatSeconds: 30,
  }
}

function launch(control: FakeControl, name: string) {
  const dir = join(root.path, name)
  const env: NodeJS.ProcessEnv = { ...process.env }
  for (const key of Object.keys(env))
    if (/^(CLAUDE|ANTHROPIC|CODEX|OPENAI)/.test(key) || key === "LD_PRELOAD") delete env[key]
  const child = spawn(process.execPath, [mainPath, "run"], {
    env: {
      ...env,
      GIT_CONFIG_NOSYSTEM: "1",
      SANCHORIS_THREAD_ID: control.threadId,
      SANCHORIS_CHANNEL_URL: control.wsUrl,
      SANCHORIS_BOOTSTRAP_TOKEN: control.bootstrapToken,
      SANCHORIS_STATE_DIR: join(dir, "state"),
      SANCHORIS_WORKSPACE_DIR: join(dir, "workspace"),
      SANCHORIS_SKILLS_DIR: join(dir, "skills"),
      SANCHORIS_SOCKET: join(dir, "s.sock"),
      SANCHORIS_GIT_URL_PREFIX: `file://${root.path}/remote/`,
      ...(process.env.SANCHORIS_CLAUDE_PATH
        ? { SANCHORIS_CLAUDE_PATH: process.env.SANCHORIS_CLAUDE_PATH }
        : {}),
      ...(process.env.SANCHORIS_CODEX_PATH
        ? { SANCHORIS_CODEX_PATH: process.env.SANCHORIS_CODEX_PATH }
        : {}),
    },
    stdio: ["ignore", "ignore", "inherit"],
  })
  return new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)))
}

const log = (label: string, value: unknown) =>
  process.stdout.write(`${label}: ${JSON.stringify(value)}\n`)

try {
  const first = await FakeControl.listen("th_smoke")
  first.start = start(first, null, "Remember the code word ZEBRA-7 for later. Reply with just OK.")
  const exited1 = launch(first, "w1")
  const ready = await first.next("ready", () => true, 120_000)
  const turn1 = await first.next("turn_end", (message) => message.turn === 1, 300_000)
  log("turn 1", { result: turn1.result, summary: turn1.summary })

  first.send({
    type: "input",
    inputId: "skill",
    text: "What is the sanchoris smoke word? Use your skills. Reply with the word only.",
    delivery: "turn",
  })
  const turn2 = await first.next("turn_end", (message) => message.turn === 2, 300_000)
  log("turn 2 (skill)", { result: turn2.result, summary: turn2.summary })
  results["Worker reads the SKILL.md placed in the Workspace"] =
    turn2.summary.includes("TANGERINE-58")

  first.received.splice(0)
  first.send({
    type: "input",
    inputId: "steer-turn",
    text: "Run the shell command `timeout 25 tail -f /dev/null; echo waited` in the foreground with your shell tool (not in the background). After it finishes, reply with the name of the fruit I tell you while it runs, or NONE.",
    delivery: "turn",
  })
  await first.next("event", (message) => message.kind === "tool_use", 300_000)
  first.send({ type: "input", inputId: "steer", text: "The fruit is MANGO.", delivery: "steer" })
  const turn3 = await first.next("turn_end", (message) => message.turn === 3, 300_000)
  log("turn 3 (steer)", { result: turn3.result, summary: turn3.summary })
  for (const message of first.received)
    if (message.type === "event" && message.kind !== "log")
      log("turn 3 event", { kind: message.kind, data: JSON.stringify(message.data).slice(0, 200) })
  results["A steer during a tool call is read in the same turn"] = /mango/i.test(turn3.summary)
  const record = await first.next("thread_record", (message) => message.turn === 3, 60_000)
  first.send({ type: "shutdown", reason: "smoke" })
  log("first process exit", await exited1)

  const second = await FakeControl.listen("th_smoke")
  second.records.set(record.key, first.records.get(record.key) ?? Buffer.alloc(0))
  second.start = start(
    second,
    {
      url: `${second.httpUrl}/records/${encodeURIComponent(record.key)}`,
      sha256: record.sha256,
      sessionId: ready.sessionId,
      turn: 3,
    },
    "What code word did I ask you to remember? Reply with the word only.",
  )
  const exited2 = launch(second, "w2")
  const turn4 = await second.next("turn_end", (message) => message.turn === 4, 300_000)
  log("turn 4 (resumed)", { result: turn4.result, summary: turn4.summary })
  for (const message of second.received)
    if (
      message.type === "event" &&
      message.kind !== "log" &&
      !(message.kind === "system" && message.data.subtype === "thinking_tokens")
    )
      log("turn 4 event", { kind: message.kind, data: JSON.stringify(message.data).slice(0, 300) })
  results["The conversation resumes from the record in a new process"] =
    turn4.summary.includes("ZEBRA-7")
  second.send({ type: "shutdown", reason: "smoke" })
  log("second process exit", await exited2)
  await first.close()
  await second.close()
} finally {
  log("results", results)
  await root.remove()
}
process.exit(Object.values(results).length === 3 && Object.values(results).every(Boolean) ? 0 : 1)
