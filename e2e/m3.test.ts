// M3 end to end: the control plane (`sanchoris`) on DynamoDB Local runs threads in local
// Workspaces (`sanchoris-agent` processes with the scripted fake Worker), against a bare git
// repository that stands in for GitHub. Everything is driven and observed through GraphQL.

import assert from "node:assert/strict"
import { type ChildProcess, execFileSync, spawn } from "node:child_process"
import { randomBytes } from "node:crypto"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { createServer } from "node:net"
import { join, resolve } from "node:path"
import { after, before, test } from "node:test"

const repo = resolve(import.meta.dirname, "..")
const agentMain = join(repo, "apps", "sanchoris-agent", "src", "main.ts")
const fakeWorker = join(repo, "apps", "sanchoris-agent", "src", "testing", "fake-worker.ts")

const gitEnv = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_AUTHOR_NAME: "test",
  GIT_AUTHOR_EMAIL: "test@example.com",
  GIT_COMMITTER_NAME: "test",
  GIT_COMMITTER_EMAIL: "test@example.com",
}

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, env: gitEnv, encoding: "utf8" }).trim()
}

function freePort(): Promise<number> {
  return new Promise((done) => {
    const server = createServer().listen(0, "127.0.0.1", () => {
      const port = (server.address() as { port: number }).port
      server.close(() => done(port))
    })
  })
}

/** The environment without the file tracer that `vp run` puts into its tasks. */
function baseEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  for (const name of ["LD_PRELOAD", "FSPY", "FSPY_PAYLOAD"]) delete env[name]
  return env
}

async function waitFor<T>(
  what: string,
  check: () => Promise<T | undefined | null | false>,
  timeoutMs = 60_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  let last: unknown
  while (Date.now() < deadline) {
    try {
      const value = await check()
      if (value) return value
    } catch (error) {
      last = error
    }
    await new Promise((done) => setTimeout(done, 200))
  }
  throw new Error(
    `${what} within ${timeoutMs} ms${last instanceof Error ? `: ${last.message}` : ""}`,
  )
}

let root = ""
let dynamo: ChildProcess | undefined
let server: ChildProcess | undefined
let serverLog = ""
let url = ""

async function gql<T = Record<string, unknown>>(
  query: string,
  variables: Record<string, unknown> = {},
): Promise<T> {
  const response = await fetch(`${url}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query, variables }),
  })
  const body = (await response.json()) as { data?: T; errors?: { message: string }[] }
  if (body.errors?.length) throw new Error(body.errors.map((e) => e.message).join("; "))
  return body.data as T
}

interface ThreadView {
  name: string
  status: string
  error: string | null
  branch: string
  messages: { seq: number; sender: string; text: string }[]
  turns: { seq: number; result: string | null; recordVersion: string | null }[]
  workspaces: { seq: number; state: string; stopReason: string | null }[]
  skillRuns: { id: number; skill: string; origin: string; status: string }[]
  log: { type: string; data: string }[]
}

const threadFields = `name status error branch
  messages { seq sender text }
  turns { seq result recordVersion }
  workspaces { seq state stopReason }
  skillRuns { id skill origin status }
  log { type data }`

async function thread(name: string): Promise<ThreadView> {
  const data = await gql<{ thread: ThreadView }>(
    `query($name: String!) { thread(name: $name) { ${threadFields} } }`,
    { name },
  )
  return data.thread
}

async function start(project: string, message: string): Promise<string> {
  const data = await gql<{ startThread: { name: string } }>(
    `mutation($project: String!, $message: String!) { startThread(project: $project, message: $message) { name } }`,
    { project, message },
  )
  return data.startThread.name
}

async function send(
  name: string,
  text: string,
): Promise<{ delivery: string; runId: number | null }> {
  const data = await gql<{ sendMessage: { delivery: string; runId: number | null } }>(
    `mutation($name: String!, $text: String!) { sendMessage(thread: $name, text: $text) { delivery runId } }`,
    { name, text },
  )
  return data.sendMessage
}

/** Waits until the thread is idle (`SUCCEEDED`) and a message matches. */
async function waitMessage(
  name: string,
  match: (m: ThreadView["messages"][number]) => boolean,
  what: string,
): Promise<ThreadView> {
  return waitFor(`${what} in ${name}`, async () => {
    const t = await thread(name)
    return t.messages.some(match) ? t : null
  })
}

before(async () => {
  root = await mkdtemp("/tmp/sanchoris-e2e-")
  const bare = join(root, "remote", "github.com", "acme", "app")
  await mkdir(bare, { recursive: true })
  git(bare, "init", "--quiet", "--bare", "--initial-branch=main")
  const seed = join(root, "seed")
  await mkdir(seed)
  git(seed, "init", "--quiet", "--initial-branch=main")
  await writeFile(join(seed, "README.md"), "app\n")
  git(seed, "add", ".")
  git(seed, "commit", "--quiet", "-m", "init")
  git(seed, "push", "--quiet", bare, "main:main")

  const credentials = join(root, "credentials.json")
  await writeFile(
    credentials,
    JSON.stringify({
      local: { provider: "claude", kind: "claude_oauth_token", value: "local-secret" },
    }),
  )
  const frontend = join(root, "frontend")
  await mkdir(frontend)
  await writeFile(join(frontend, "index.html"), "<!doctype html>\n")

  execFileSync("cargo", ["build", "--quiet", "-p", "sanchoris-api", "--bin", "sanchoris"], {
    cwd: repo,
    stdio: "inherit",
    env: baseEnv(),
  })

  const dynamoPort = await freePort()
  dynamo = spawn(
    "dynamodb-local",
    ["-inMemory", "-disableTelemetry", "-port", String(dynamoPort)],
    { stdio: "ignore" },
  )
  const port = await freePort()
  url = `http://127.0.0.1:${port}`
  const env: NodeJS.ProcessEnv = {
    ...baseEnv(),
    AWS_REGION: "ap-northeast-1",
    AWS_ACCESS_KEY_ID: "local",
    AWS_SECRET_ACCESS_KEY: "local",
    AWS_ENDPOINT_URL_DYNAMODB: `http://127.0.0.1:${dynamoPort}`,
    AWS_ENDPOINT_URL_DYNAMODB_STREAMS: `http://127.0.0.1:${dynamoPort}`,
    SANCHORIS_BIND_ADDR: `127.0.0.1:${port}`,
    SANCHORIS_FRONTEND_DIR: frontend,
    SANCHORIS_TABLE_MAIN: "main",
    SANCHORIS_TABLE_AUDIT: "audit",
    THREAD_TOKEN_SIGNING_KEY: randomBytes(32).toString("hex"),
    SANCHORIS_CREDENTIAL_KEY: randomBytes(32).toString("base64"),
    SANCHORIS_LOCAL_USER: "local",
    SANCHORIS_LOCAL_CONFIG_DIR: join(import.meta.dirname, "fixtures", "config"),
    SANCHORIS_LOCAL_CREDENTIALS: credentials,
    SANCHORIS_LOCAL_GIT_ROOT: join(root, "remote"),
    SANCHORIS_LOCAL_WORKSPACES_DIR: join(root, "ws"),
    SANCHORIS_AGENT_COMMAND: `${process.execPath} ${agentMain}`,
    SANCHORIS_GIT_URL_PREFIX: `file://${root}/remote/`,
    SANCHORIS_WORKER_MODULE: fakeWorker,
    GIT_CONFIG_NOSYSTEM: "1",
  }
  await waitFor("DynamoDB Local listens", async () => {
    await fetch(`http://127.0.0.1:${dynamoPort}`)
    return true
  })
  server = spawn(join(repo, "target", "debug", "sanchoris"), [], {
    env,
    stdio: ["ignore", "ignore", "pipe"],
  })
  server.stderr?.on("data", (chunk: Buffer) => {
    serverLog += chunk.toString("utf8")
  })
  const exited = new Promise<never>((_, reject) =>
    server?.on("exit", (code) => reject(new Error(`sanchoris exited with ${code}:\n${serverLog}`))),
  )
  await Promise.race([
    waitFor("the server listens", async () => (await fetch(`${url}/health`)).ok),
    exited,
  ])
})

after(async () => {
  const pids = execFileSync(
    "sh",
    ["-c", `for f in ${root}/ws/*/*/agent.pid; do [ -f "$f" ] && cat "$f" && echo; done; true`],
    {
      encoding: "utf8",
    },
  )
  for (const pid of pids.split(/\s+/).filter(Boolean)) {
    try {
      process.kill(Number(pid), "SIGKILL")
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error
    }
  }
  server?.kill("SIGTERM")
  dynamo?.kill("SIGTERM")
  if (process.env.SANCHORIS_E2E_LOG) {
    process.stderr.write(serverLog)
    process.stderr.write(
      execFileSync("sh", ["-c", `tail -n 40 ${root}/ws/*/*/agent.log || true`], {
        encoding: "utf8",
      }),
    )
  }
  if (root) await rm(root, { recursive: true, force: true })
})

void test("a thread takes its instruction, runs a skill, takes the notifications and ends its turns", async () => {
  const name = await start("app", "Start\n$ sanchoris-agent skill run where --input '{}'")
  const t = await waitFor(`the SkillRun of ${name} succeeds and the thread is done`, async () => {
    const current = await thread(name)
    return current.status === "SUCCEEDED" &&
      current.skillRuns.some((r) => r.status === "SUCCEEDED") &&
      current.messages.some((m) => m.sender === "agent" && m.text.includes("Skill where (run "))
      ? current
      : null
  })
  const run = t.skillRuns[0]
  assert.equal(run.skill, "where")
  assert.equal(run.origin, "thread")
  const notices = t.messages.filter((m) => m.sender === "sanchoris")
  assert.ok(
    notices.some((m) =>
      m.text.startsWith(`[sanchoris] Skill where (run ${run.id}): Where -> Done`),
    ),
    JSON.stringify(notices),
  )
  assert.ok(
    notices.some((m) => m.text.includes(`(run ${run.id}): `) && m.text.includes("SUCCEEDED")),
  )
  assert.ok(t.turns.length >= 1 && t.turns.every((turn) => turn.result === "completed"))
  assert.ok(t.turns.some((turn) => turn.recordVersion?.includes(`/threads/${name}/records/`)))
  const first = t.messages.find((m) => m.sender === "agent")
  assert.match(first?.text ?? "", /RunId/)
  // The RunOnCreate script of the project fails, and the failure is an input of the thread.
  await waitMessage(
    name,
    (m) =>
      m.sender === "sanchoris" &&
      m.text.startsWith("[sanchoris] Script setup (RunOnCreate) failed. Exit 3.") &&
      m.text.includes("setup is broken"),
    "the RunOnCreate failure",
  )

  // A Skill that the User launches runs in the Workspace and on the branch of the thread.
  const launched = await send(name, "/where")
  assert.equal(launched.delivery, "launched")
  const userRun = await waitFor(`the SkillRun ${launched.runId} of the User`, async () => {
    const current = await thread(name)
    const found = current.skillRuns.find((r) => r.id === launched.runId)
    return found?.status === "SUCCEEDED" ? { t: current, r: found } : null
  })
  assert.equal(userRun.r.origin, "user")
  const transition = await waitMessage(
    name,
    (m) => m.sender === "sanchoris" && m.text.includes(`(run ${launched.runId}): Where -> Done`),
    "the notice of the SkillRun of the User",
  )
  const text = transition.messages.find((m) =>
    m.text.includes(`(run ${launched.runId}): Where -> Done`),
  )?.text
  assert.ok(text?.includes(join(root, "ws", name, "1", "workspace")), text)
  assert.ok(text?.includes(`sanchoris/${name}`), text)
  assert.ok(transition.messages.some((m) => m.sender === "user" && m.text === "/where"))
  await waitMessage(
    name,
    (m) => m.sender === "agent" && m.text.includes(`Skill where (run ${launched.runId})`),
    "the reply of the Worker to the notice of the SkillRun of the User",
  )

  // A script run from the screen leaves its result in the log of the thread.
  const ran = await gql<{ runScript: string }>(
    `mutation($name: String!) { runScript(thread: $name, name: "report") }`,
    { name },
  )
  const logged = await waitFor("the result of the script in the log", async () => {
    const t = await thread(name)
    const lines = t.log.filter((l) => l.data.includes(ran.runScript))
    return lines.some((l) => l.type === "script_result") &&
      lines.some((l) => l.data.includes("report-output"))
      ? lines
      : null
  })
  assert.ok(logged.length >= 2)

  // The Workspace is interrupted; the next message restores the conversation in a new one.
  await waitFor(`${name} is idle`, async () => (await thread(name)).status === "SUCCEEDED")
  const pid = Number(await readFile(join(root, "ws", name, "1", "agent.pid"), "utf8"))
  process.kill(pid, "SIGTERM")
  await waitFor("the Workspace stops", async () => {
    const t = await thread(name)
    return t.workspaces[0]?.state === "stopped" ? t : null
  })
  const restored = await send(name, "recall")
  assert.equal(restored.delivery, "restoring")
  const recalled = await waitMessage(
    name,
    (m) =>
      m.sender === "agent" &&
      m.text.includes("first: [sanchoris] The user started this thread: Start."),
    "the restored conversation",
  )
  assert.equal(recalled.workspaces.length, 2)
  assert.equal(recalled.workspaces[0].stopReason, "interrupted")
  assert.equal(
    git(join(root, "remote", "github.com", "acme", "app"), "branch", "--list", `sanchoris/${name}`),
    `sanchoris/${name}`,
  )
})

void test("a RunOnCreate script with Async false that fails makes the thread need the User", async () => {
  const name = await start("strict", "Start strict")
  const t = await waitFor(`${name} needs the User`, async () => {
    const current = await thread(name)
    return current.status === "NEEDS_YOU" &&
      current.messages.some((m) =>
        m.text.startsWith("[sanchoris] Script setup (RunOnCreate) failed. Exit 4."),
      )
      ? current
      : null
  })
  assert.equal(t.error, "Script.Failed")
  assert.ok(!t.messages.some((m) => m.sender === "agent"))
})

void test("a New thread that starts with /<name> launches the Skill in the new thread", async () => {
  const name = await start("app", "/where")
  const t = await waitFor(`the SkillRun of ${name}`, async () => {
    const current = await thread(name)
    return current.skillRuns[0]?.status === "SUCCEEDED" &&
      current.messages.some((m) => m.sender === "sanchoris" && m.text.includes("Skill where (run "))
      ? current
      : null
  })
  assert.equal(t.skillRuns[0].origin, "user")
})
