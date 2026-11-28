import { createHash } from "node:crypto"
import { readFile } from "node:fs/promises"
import { join } from "node:path"

import { afterEach, beforeEach, describe, expect, test } from "vite-plus/test"

import { FakeControl, type ControlPayload } from "./fake-control.ts"
import { git, makeRemote, spawnAgent, tarball, tempDir, type AgentProcess } from "./helpers.ts"

const BRANCH = "sanchoris/th_1"
const sha256 = (bytes: Buffer) => createHash("sha256").update(bytes).digest("hex")

const OPEN_PR = `---
name: open-pr-green-ci
description: Open a pull request and wait for CI.
---
Run \`sanchoris-agent skill run open-pr-green-ci --input '{"Repo":"<repo>","Issue":<n>}'\`.
`

function startMessage(
  control: FakeControl,
  overrides: Partial<Extract<ControlPayload, { type: "start" }>> = {},
): ControlPayload {
  return {
    type: "start",
    thread: { name: "th_1", title: "Fix the checkout test" },
    project: {
      name: "shop",
      repos: [{ name: "app", repo: "github.com/acme/app", branch: BRANCH }],
      scripts: [],
    },
    worker: {
      agent: "claude",
      model: "fake",
      credential: { kind: "claude_oauth_token", value: "cred-1" },
    },
    gitAuthor: { name: "sanchoris", email: "agent@sanchoris.invalid" },
    skills: {
      url: `${control.httpUrl}/files/skills.tar.gz`,
      sha256: sha256(control.files.get("skills.tar.gz") ?? Buffer.alloc(0)),
      entries: [
        {
          name: "open-pr-green-ci",
          predictable: true,
          inputs: { Repo: "string", Issue: "number" },
        },
        { name: "notes", predictable: false, inputs: {} },
      ],
    },
    record: null,
    instruction: null,
    heartbeatSeconds: 1,
    ...overrides,
  }
}

let root: { path: string; remove: () => Promise<void> }
let controls: FakeControl[]
let agents: AgentProcess[]

beforeEach(async () => {
  root = await tempDir()
  controls = []
  agents = []
  await makeRemote(root.path, BRANCH)
})

afterEach(async () => {
  for (const agent of agents) agent.child.kill("SIGKILL")
  for (const control of controls) await control.close()
  await root.remove()
})

async function workspace(name: string): Promise<{ control: FakeControl; agent: AgentProcess }> {
  const control = await FakeControl.listen()
  controls.push(control)
  control.files.set(
    "skills.tar.gz",
    await tarball(root.path, {
      "open-pr-green-ci/SKILL.md": OPEN_PR,
      "notes/SKILL.md": "---\nname: notes\n---\n",
    }),
  )
  control.answer = (message) => {
    if (message.type === "skill_request" && message.request.action === "run")
      return {
        type: "skill_response",
        requestId: message.requestId,
        ok: true,
        result: { RunId: 1, Status: "RUNNING" },
      }
    return null
  }
  const agent = spawnAgent(control, root.path, name)
  agents.push(agent)
  return { control, agent }
}

describe("sanchoris-agent against a fake control plane", { timeout: 60_000 }, () => {
  test("takes an instruction, runs a skill, takes the notification, ends turns, and resumes in a new process", async () => {
    const first = await workspace("w1")
    first.control.start = startMessage(first.control, {
      instruction: [
        "remember apple",
        `$ sanchoris-agent skill run open-pr-green-ci --input '{"Repo":"app","Issue":35}'`,
        "$ echo changed > app/change.txt",
      ].join("\n"),
    })

    const ready = await first.control.next("ready")
    expect(ready.agent).toBe("claude")
    const request = await first.control.next("skill_request")
    expect(request.request).toEqual({
      action: "run",
      name: "open-pr-green-ci",
      input: { Repo: "app", Issue: 35 },
    })

    const record1 = await first.control.next("thread_record", (message) => message.turn === 1)
    const turn1 = await first.control.next("turn_end", (message) => message.turn === 1)
    expect(turn1.result).toBe("completed")
    expect(turn1.summary).toContain('{"RunId":1,"Status":"RUNNING"}')
    expect(turn1.commits).toHaveLength(1)
    expect(turn1.commits[0]).toMatchObject({ repo: "app", branch: BRANCH, pushed: true })
    const remote = join(root.path, "remote", "github.com", "acme", "app")
    expect(git(remote, "rev-parse", BRANCH)).toBe(turn1.commits[0]?.commit)
    expect(git(remote, "show", `${BRANCH}:change.txt`)).toBe("changed")
    expect(record1.key).toBe("u/user1/threads/th_1/records/000001.tar.gz")
    expect(sha256(first.control.records.get(record1.key) ?? Buffer.alloc(0))).toBe(record1.sha256)

    first.control.send({
      type: "input",
      inputId: "in-1",
      text: "[sanchoris] Skill open-pr-green-ci (run 1): SUCCEEDED.\nskills",
      delivery: "turn",
    })
    const turn2 = await first.control.next("turn_end", (message) => message.turn === 2)
    expect(turn2.summary).toBe("skills: notes,open-pr-green-ci")
    const record2 = await first.control.next("thread_record", (message) => message.turn === 2)

    first.control.send({ type: "shutdown", reason: "idle" })
    expect(await first.agent.exited).toBe(0)
    expect([...first.control.records.keys()]).toEqual([record1.key, record2.key])

    const second = await workspace("w2")
    second.control.records.set(
      record2.key,
      first.control.records.get(record2.key) ?? Buffer.alloc(0),
    )
    second.control.start = startMessage(second.control, {
      record: {
        url: `${second.control.httpUrl}/records/${encodeURIComponent(record2.key)}`,
        sha256: record2.sha256,
        sessionId: ready.sessionId,
        turn: 2,
      },
      instruction: "recall\n$ cat app/change.txt",
    })
    const resumed = await second.control.next("ready")
    expect(resumed.sessionId).toBe(ready.sessionId)
    const turn3 = await second.control.next("turn_end", (message) => message.turn === 3)
    expect(turn3.summary).toBe("first: remember apple\nchanged")
    second.control.send({ type: "shutdown", reason: "done" })
    expect(await second.agent.exited).toBe(0)
  })

  test("steers an input into the running turn and queues a turn input for the next turn", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control, { instruction: "$ sleep 2\n$ echo slow" })
    await control.next("ready")
    await new Promise((resolve) => setTimeout(resolve, 500))
    control.send({ type: "input", inputId: "in-1", text: "use a smaller fix", delivery: "steer" })
    control.send({ type: "input", inputId: "in-2", text: "next please", delivery: "turn" })
    const turn1 = await control.next("turn_end", (message) => message.turn === 1)
    expect(turn1.summary).toBe("slow\nsteered: use a smaller fix")
    const turn2 = await control.next("turn_end", (message) => message.turn === 2)
    expect(turn2.summary).toBe("ok: next please")
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("runs exec with the State input on stdin once per idempotency key, and hands continue to await", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control)
    await control.next("ready")
    control.answer = (message) =>
      message.type === "skill_request"
        ? {
            type: "skill_response",
            requestId: message.requestId,
            ok: true,
            result: { RunId: 7, Status: "RUNNING" },
          }
        : null
    const exec = {
      type: "exec" as const,
      idempotencyKey: "7:1:1",
      skill: { name: "open-pr-green-ci", runId: 7 },
      run: 'echo "$SANCHORIS_SKILL_RUN_ID $SANCHORIS_THREAD_NAME $(pwd)"; cat; echo oops >&2; exit 3',
      stdin: '{"Repo":"app"}',
      timeoutSeconds: 10,
    }
    control.send(exec)
    const result = await control.next("exec_result")
    expect(result).toMatchObject({
      idempotencyKey: "7:1:1",
      exitCode: 3,
      stderr: "oops\n",
      timedOut: false,
    })
    expect(result.stdout).toBe(`7 th_1 ${agent.workspaceDir}\n{"Repo":"app"}`)
    control.send(exec)
    const again = await control.next("exec_result")
    expect(again.stdout).toBe(result.stdout)

    control.send({
      ...exec,
      idempotencyKey: "7:2:1",
      run: "sanchoris-agent skill await",
      stdin: "{}",
    })
    const awaitRequest = await control.next("skill_request")
    expect(awaitRequest.request).toEqual({ action: "await", runId: 7 })
    control.send({ type: "continue_output", runId: 7, output: { fixed: true } })
    const awaited = await control.next(
      "exec_result",
      (message) => message.idempotencyKey === "7:2:1",
    )
    expect(awaited).toMatchObject({ exitCode: 0, stdout: '{"fixed":true}\n' })

    control.send({ ...exec, idempotencyKey: "7:3:1", run: "sleep 5", timeoutSeconds: 1 })
    const timedOut = await control.next(
      "exec_result",
      (message) => message.idempotencyKey === "7:3:1",
    )
    expect(timedOut.timedOut).toBe(true)
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("rejects a skill that the thread cannot call and inputs that do not match", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control, {
      instruction: [
        "$ sanchoris-agent skill run notes --input '{}' || echo exit=$?",
        `$ sanchoris-agent skill run open-pr-green-ci --input '{"Repo":1}' 2>&1 || echo exit=$?`,
      ].join("\n"),
    })
    const turn = await control.next("turn_end")
    expect(turn.summary).toContain("exit=1")
    expect(turn.summary).toContain("Repo must be string, got number; Issue is required (number)")
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("commits and pushes before thread start, and sends the child thread's answer back", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control, {
      instruction:
        "$ echo base > app/base.txt && sanchoris-agent thread start --message 'Add subtract'",
    })
    control.answer = (message) =>
      message.type === "thread_request"
        ? {
            type: "thread_response",
            requestId: message.requestId,
            ok: true,
            result: { Name: "th_2", Status: "PENDING", PendingReason: "concurrency" },
          }
        : null
    const request = await control.next("thread_request")
    expect(request.request).toMatchObject({
      action: "start",
      message: "Add subtract",
      project: null,
      model: null,
    })
    const remote = join(root.path, "remote", "github.com", "acme", "app")
    expect(request.request.action === "start" && request.request.commits[0]?.commit).toBe(
      git(remote, "rev-parse", BRANCH),
    )
    expect(git(remote, "show", `${BRANCH}:base.txt`)).toBe("base")
    const turn = await control.next("turn_end")
    expect(turn.summary).toBe('{"Name":"th_2","Status":"PENDING","PendingReason":"concurrency"}')
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("reconnects with the thread token, takes new credentials, and reports a refreshed worker credential", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control)
    await control.next("ready")
    control.drop()
    control.githubToken = "ghs_fake_2"
    control.send(control.credentials())
    control.send({
      type: "input",
      inputId: "in-1",
      text: [
        `$ printf 'protocol=https\\nhost=github.com\\n\\n' | git credential fill | grep password`,
        "$ printf refreshed > $FAKE_WORKER_HOME/credential",
      ].join("\n"),
      delivery: "turn",
    })
    const turn = await control.next("turn_end")
    expect(turn.summary).toBe("password=ghs_fake_2")
    const update = await control.next("credential_update")
    expect(update.credential).toEqual({ kind: "claude_oauth_token", value: "refreshed" })
    expect(control.connections[0]).toBe("bootstrap")
    expect(control.connections.slice(1)).toContain("thread")
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("runs RunOnCreate scripts and a script from the screen, and reports their results", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control, {
      project: {
        name: "shop",
        repos: [{ name: "app", repo: "github.com/acme/app", branch: BRANCH }],
        scripts: [
          {
            name: "setup",
            command: "echo installing; echo missing tool >&2; exit 2",
            timeoutSeconds: 10,
            runOnCreate: true,
            async: false,
          },
          {
            name: "lint",
            command: "echo lint ok",
            timeoutSeconds: 10,
            runOnCreate: false,
            async: true,
          },
        ],
      },
      instruction: "first task",
    })
    const setup = await control.next("script_result", (message) => message.name === "setup")
    expect(setup).toMatchObject({
      scriptRunId: "create:setup",
      exitCode: 2,
      stderrTail: "missing tool\n",
    })
    control.send({ type: "script", scriptRunId: "sr-1", name: "lint" })
    const lint = await control.next("script_result", (message) => message.name === "lint")
    expect(lint.exitCode).toBe(0)
    const log = await control.next(
      "event",
      (message) => message.kind === "log" && message.data.script === "lint",
    )
    expect(log.data.stdout).toBe("lint ok\n")
    control.send({
      type: "input",
      inputId: "in-1",
      text: "[sanchoris] Script setup failed.",
      delivery: "turn",
    })
    const turn = await control.next("turn_end")
    expect(turn.summary).toBe("ok: first task")
    control.send({ type: "shutdown", reason: "done" })
    expect(await agent.exited).toBe(0)
  })

  test("on SIGTERM interrupts the turn, pushes the branch, saves the record and reports stopping", async () => {
    const { control, agent } = await workspace("w1")
    control.start = startMessage(control, {
      instruction: "$ echo wip > app/wip.txt\n$ sleep 2\n$ echo never",
    })
    await control.next("ready")
    await new Promise((resolve) => setTimeout(resolve, 700))
    agent.child.kill("SIGTERM")
    const stopping = await control.next("stopping")
    expect(stopping.commits[0]?.pushed).toBe(true)
    expect(stopping.recordKey).not.toBeNull()
    const remote = join(root.path, "remote", "github.com", "acme", "app")
    expect(git(remote, "show", `${BRANCH}:wip.txt`)).toBe("wip")
    const turn = await control.next("turn_end")
    expect(turn.result).toBe("interrupted")
    expect(await agent.exited).toBe(0)
    expect(await readFile(join(agent.stateDir, "github-token"), "utf8")).toBe("ghs_fake_1")
  })
})
