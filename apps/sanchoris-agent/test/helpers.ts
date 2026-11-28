import { type ChildProcess, execFileSync, spawn } from "node:child_process"
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"

import type { FakeControl } from "./fake-control.ts"

const appDir = resolve(import.meta.dirname, "..")
export const mainPath = join(appDir, "src", "main.ts")
export const fakeWorkerPath = join(appDir, "src", "testing", "fake-worker.ts")

/** Git without the user's configuration, with an identity, for the test repositories. */
export const gitEnv: NodeJS.ProcessEnv = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_AUTHOR_NAME: "test",
  GIT_AUTHOR_EMAIL: "test@example.com",
  GIT_COMMITTER_NAME: "test",
  GIT_COMMITTER_EMAIL: "test@example.com",
}

export function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, env: gitEnv, encoding: "utf8" }).trim()
}

/** A short temporary directory: Unix socket paths are limited to about 107 bytes. */
export async function tempDir(): Promise<{ path: string; remove: () => Promise<void> }> {
  const path = await mkdtemp("/tmp/sa-")
  return { path, remove: () => rm(path, { recursive: true, force: true }) }
}

/** A bare repository `github.com/acme/app` under `root` with the thread branch. */
export async function makeRemote(root: string, branch: string): Promise<string> {
  const bare = join(root, "remote", "github.com", "acme", "app")
  await mkdir(bare, { recursive: true })
  git(bare, "init", "--quiet", "--bare", "--initial-branch=main")
  const seed = join(root, "seed")
  await mkdir(seed)
  git(seed, "init", "--quiet", "--initial-branch=main")
  await writeFile(join(seed, "README.md"), "app\n")
  git(seed, "add", ".")
  git(seed, "commit", "--quiet", "-m", "init")
  git(seed, "push", "--quiet", bare, `main:main`, `main:${branch}`)
  return bare
}

/** A gzipped tar of `files` (path to content), as the control plane serves the skills. */
export async function tarball(root: string, files: Record<string, string>): Promise<Buffer> {
  const dir = await mkdtemp(join(root, "tar-"))
  for (const [path, content] of Object.entries(files)) {
    await mkdir(join(dir, path, ".."), { recursive: true })
    await writeFile(join(dir, path), content)
  }
  return execFileSync("tar", ["-czf", "-", "-C", dir, "."])
}

export interface AgentProcess {
  child: ChildProcess
  stderr: () => string
  exited: Promise<number | null>
  stateDir: string
  workspaceDir: string
  skillsDir: string
}

/** The test's environment without the file tracer that `vp run` puts into its tasks. */
function agentBaseEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  for (const name of ["LD_PRELOAD", "FSPY", "FSPY_PAYLOAD"]) delete env[name]
  return env
}

export function spawnAgent(control: FakeControl, root: string, name: string): AgentProcess {
  const dir = join(root, name)
  const stateDir = join(dir, "state")
  const workspaceDir = join(dir, "workspace")
  const skillsDir = join(dir, "skills")
  const child = spawn(process.execPath, [mainPath, "run"], {
    env: {
      ...agentBaseEnv(),
      GIT_CONFIG_NOSYSTEM: "1",
      SANCHORIS_THREAD_ID: control.threadId,
      SANCHORIS_CHANNEL_URL: control.wsUrl,
      SANCHORIS_BOOTSTRAP_TOKEN: control.bootstrapToken,
      SANCHORIS_STATE_DIR: stateDir,
      SANCHORIS_WORKSPACE_DIR: workspaceDir,
      SANCHORIS_SKILLS_DIR: skillsDir,
      SANCHORIS_SOCKET: join(dir, "s.sock"),
      SANCHORIS_GIT_URL_PREFIX: `file://${root}/remote/`,
      SANCHORIS_WORKER_MODULE: fakeWorkerPath,
    },
    stdio: ["ignore", "ignore", "pipe"],
  })
  let stderr = ""
  child.stderr?.on("data", (chunk: Buffer) => {
    stderr += chunk.toString("utf8")
  })
  const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)))
  return { child, stderr: () => stderr, exited, stateDir, workspaceDir, skillsDir }
}
