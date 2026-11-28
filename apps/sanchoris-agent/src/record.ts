import { execFile } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdir } from "node:fs/promises"
import { promisify } from "node:util"

const execFileAsync = promisify(execFile)

export function sha256(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex")
}

/** The thread record: the provider's state directory as a gzipped tar, without `exclude`. */
export async function packRecord(home: string, exclude: readonly string[]): Promise<Buffer> {
  const args = [
    "-czf",
    "-",
    "--warning=no-file-changed",
    "-C",
    home,
    ...exclude.map((name) => `--exclude=./${name}`),
    ".",
  ]
  try {
    const { stdout } = await execFileAsync("tar", args, {
      encoding: "buffer",
      maxBuffer: 1024 * 1024 * 1024,
    })
    return stdout
  } catch (error) {
    const failed = error as { code?: unknown; stdout?: Buffer }
    if (failed.code === 1 && failed.stdout && failed.stdout.length > 0) return failed.stdout
    throw error
  }
}

export async function unpackRecord(home: string, bytes: Buffer): Promise<void> {
  await mkdir(home, { recursive: true, mode: 0o700 })
  await new Promise<void>((resolve, reject) => {
    const child = execFile("tar", ["-xzf", "-", "-C", home], (error) =>
      error ? reject(error) : resolve(),
    )
    child.stdin?.end(bytes)
  })
}

export async function downloadRecord(url: string, expectedSha256: string): Promise<Buffer> {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`Thread record download failed with HTTP ${response.status}`)
  const bytes = Buffer.from(await response.arrayBuffer())
  const actual = sha256(bytes)
  if (actual !== expectedSha256)
    throw new Error(`Thread record checksum mismatch: expected ${expectedSha256}, got ${actual}`)
  return bytes
}

/** Uploads with the presigned POST of `credentials` (`recordUpload`). */
export async function uploadRecord(
  upload: { url: string; fields: Record<string, string> },
  key: string,
  bytes: Buffer,
): Promise<void> {
  const form = new FormData()
  for (const [name, value] of Object.entries(upload.fields)) form.append(name, value)
  form.append("key", key)
  form.append(
    "file",
    new Blob([new Uint8Array(bytes)], { type: "application/gzip" }),
    "record.tar.gz",
  )
  const response = await fetch(upload.url, { method: "POST", body: form })
  if (!response.ok) throw new Error(`Thread record upload failed with HTTP ${response.status}`)
}
