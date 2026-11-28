import { spawn } from "node:child_process"

/** The tail that a notification and `script_result` carry (`skill.md`「Skill の呼び出し」). */
export const OUTPUT_TAIL_BYTES = 16 * 1024

export interface CommandResult {
  exitCode: number
  stdout: string
  stderr: string
  timedOut: boolean
}

export function tail(text: string, bytes = OUTPUT_TAIL_BYTES): string {
  const buffer = Buffer.from(text, "utf8")
  if (buffer.length <= bytes) return text
  return buffer
    .subarray(buffer.length - bytes)
    .toString("utf8")
    .replace(/^�+/, "")
}

/** Runs `command` with `bash -c`, gives it `stdin`, and kills its process group at the timeout. */
export function runBash(options: {
  command: string
  cwd: string
  env: NodeJS.ProcessEnv
  stdin: string
  timeoutSeconds: number
  maxOutputBytes?: number
}): Promise<CommandResult> {
  const limit = options.maxOutputBytes ?? 4 * 1024 * 1024
  return new Promise((resolve) => {
    const child = spawn("bash", ["-c", options.command], {
      cwd: options.cwd,
      env: options.env,
      stdio: ["pipe", "pipe", "pipe"],
      detached: true,
    })
    let stdout = ""
    let stderr = ""
    let timedOut = false
    child.stdout.setEncoding("utf8")
    child.stderr.setEncoding("utf8")
    child.stdout.on("data", (chunk: string) => {
      stdout = tail(stdout + chunk, limit)
    })
    child.stderr.on("data", (chunk: string) => {
      stderr = tail(stderr + chunk, limit)
    })
    child.stdin.on("error", () => {})
    child.stdin.end(options.stdin)
    const timer = setTimeout(() => {
      timedOut = true
      if (child.pid !== undefined) {
        try {
          process.kill(-child.pid, "SIGKILL")
        } catch {
          child.kill("SIGKILL")
        }
      }
    }, options.timeoutSeconds * 1000)
    child.on("error", (error) => {
      clearTimeout(timer)
      resolve({ exitCode: 127, stdout, stderr: stderr + error.message, timedOut })
    })
    child.on("close", (code, signal) => {
      clearTimeout(timer)
      resolve({ exitCode: code ?? (signal ? 128 + 9 : 1), stdout, stderr, timedOut })
    })
  })
}
