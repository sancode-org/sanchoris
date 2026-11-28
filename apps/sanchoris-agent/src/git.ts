import { execFile } from "node:child_process"
import { access, chmod, mkdir, rename, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { promisify } from "node:util"

import type { RepoCommit } from "./protocol.ts"

const execFileAsync = promisify(execFile)

export interface RepoCheckout {
  name: string
  repo: string
  branch: string
}

async function git(cwd: string, env: NodeJS.ProcessEnv, args: string[]): Promise<string> {
  const { stdout } = await execFileAsync("git", args, { cwd, env, maxBuffer: 64 * 1024 * 1024 })
  return stdout.trim()
}

/** Writes the files through which `git` and `gh` read the GitHub token, and the git identity. */
export async function writeGitConfig(options: {
  stateDir: string
  helper: string
  token: string
  author: { name: string; email: string } | null
}): Promise<void> {
  const ghDir = join(options.stateDir, "gh")
  await mkdir(ghDir, { recursive: true, mode: 0o700 })
  const tokenPath = join(options.stateDir, "github-token")
  await writeFile(`${tokenPath}.tmp`, options.token, { mode: 0o600 })
  await rename(`${tokenPath}.tmp`, tokenPath)
  await writeFile(
    join(ghDir, "hosts.yml"),
    `github.com:\n    oauth_token: ${options.token}\n    user: x-access-token\n    git_protocol: https\n`,
    { mode: 0o600 },
  )
  const gitconfig = join(options.stateDir, "gitconfig")
  const quote = (value: string) => `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`
  await writeFile(
    `${gitconfig}.tmp`,
    [
      ...(options.author
        ? [
            "[user]",
            `\tname = ${quote(options.author.name)}`,
            `\temail = ${quote(options.author.email)}`,
          ]
        : []),
      '[credential "https://github.com"]',
      "\thelper = ",
      `\thelper = ${quote(`!${options.helper} git-credential`)}`,
      "",
    ].join("\n"),
    { mode: 0o600 },
  )
  await chmod(`${gitconfig}.tmp`, 0o600)
  await rename(`${gitconfig}.tmp`, gitconfig)
}

export function gitEnv(stateDir: string): Record<string, string> {
  return {
    GH_CONFIG_DIR: join(stateDir, "gh"),
    GIT_CONFIG_GLOBAL: join(stateDir, "gitconfig"),
    GIT_TERMINAL_PROMPT: "0",
  }
}

export function repoUrl(prefix: string, repo: string): string {
  return `${prefix}${repo}`
}

/** Clones each repository on its thread branch into `<workspaceDir>/<name>`, or keeps a clone. */
export async function cloneRepos(options: {
  workspaceDir: string
  repos: readonly RepoCheckout[]
  urlPrefix: string
  env: NodeJS.ProcessEnv
}): Promise<void> {
  await mkdir(options.workspaceDir, { recursive: true })
  for (const repo of options.repos) {
    const dir = join(options.workspaceDir, repo.name)
    const exists = await access(join(dir, ".git")).then(
      () => true,
      () => false,
    )
    if (exists) continue
    await git(options.workspaceDir, options.env, [
      "clone",
      "--branch",
      repo.branch,
      repoUrl(options.urlPrefix, repo.repo),
      dir,
    ])
  }
}

/** Commits the working tree of each repository to its thread branch and pushes it. */
export async function commitAndPush(options: {
  workspaceDir: string
  repos: readonly RepoCheckout[]
  env: NodeJS.ProcessEnv
  author: { name: string; email: string }
  message: string
}): Promise<RepoCommit[]> {
  const commits: RepoCommit[] = []
  const identity = [
    "-c",
    `user.name=${options.author.name}`,
    "-c",
    `user.email=${options.author.email}`,
  ]
  for (const repo of options.repos) {
    const dir = join(options.workspaceDir, repo.name)
    await git(dir, options.env, ["add", "--all"])
    const status = await git(dir, options.env, ["status", "--porcelain"])
    if (status !== "")
      await git(dir, options.env, [...identity, "commit", "--quiet", "-m", options.message])
    const commit = await git(dir, options.env, ["rev-parse", "HEAD"])
    let pushed = true
    try {
      await git(dir, options.env, ["push", "--quiet", "origin", `HEAD:refs/heads/${repo.branch}`])
    } catch {
      pushed = false
    }
    commits.push({ repo: repo.name, branch: repo.branch, commit, pushed })
  }
  return commits
}
