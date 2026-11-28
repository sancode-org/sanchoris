import { cp, mkdir, readdir, rm } from "node:fs/promises"
import { join } from "node:path"

import type { ErrorBody, InputType, SkillEntry } from "./protocol.ts"
import { downloadRecord, unpackRecord } from "./record.ts"

function typeOf(value: unknown): InputType | "null" | "other" {
  if (value === null) return "null"
  if (Array.isArray(value)) return "array"
  switch (typeof value) {
    case "string":
      return "string"
    case "number":
      return "number"
    case "boolean":
      return "boolean"
    case "object":
      return "object"
    default:
      return "other"
  }
}

/** Checks `skill run` against the skills of the thread (`architecture.md`「`sanchoris-agent` のコマンド」). */
export function validateSkillRun(
  skills: readonly SkillEntry[],
  name: string,
  input: unknown,
): ErrorBody | null {
  const skill = skills.find((candidate) => candidate.name === name && candidate.predictable)
  if (!skill) {
    const names = skills
      .filter((candidate) => candidate.predictable)
      .map((candidate) => candidate.name)
    return {
      code: "unknown_skill",
      message: `Skill ${name} is not available in this thread. Skills: ${names.length === 0 ? "(none)" : names.join(", ")}`,
    }
  }
  if (typeOf(input) !== "object")
    return { code: "invalid_input", message: "--input must be a JSON object" }
  const fields = input as Record<string, unknown>
  const problems: string[] = []
  for (const [field, expected] of Object.entries(skill.inputs)) {
    if (!(field in fields)) problems.push(`${field} is required (${expected})`)
    else if (typeOf(fields[field]) !== expected)
      problems.push(`${field} must be ${expected}, got ${typeOf(fields[field])}`)
  }
  for (const field of Object.keys(fields))
    if (!(field in skill.inputs)) problems.push(`${field} is not an input of ${skill.name}`)
  return problems.length === 0
    ? null
    : { code: "invalid_input", message: `Invalid input for ${skill.name}: ${problems.join("; ")}` }
}

/** Unpacks the skill directories into `skillsRoot` (`/skills/<name>/`). */
export async function installSkills(
  skillsRoot: string,
  archive: { url: string; sha256: string } | null,
): Promise<void> {
  await mkdir(skillsRoot, { recursive: true })
  if (archive) await unpackRecord(skillsRoot, await downloadRecord(archive.url, archive.sha256))
}

/** Copies every skill directory that has a `SKILL.md` to where the Worker reads its skills. */
export async function placeWorkerSkills(
  skillsRoot: string,
  workerSkillsDir: string,
  entries: readonly SkillEntry[],
): Promise<string[]> {
  await rm(workerSkillsDir, { recursive: true, force: true })
  await mkdir(workerSkillsDir, { recursive: true })
  const present = new Set(await readdir(skillsRoot))
  const placed: string[] = []
  for (const entry of entries) {
    if (!present.has(entry.name)) continue
    const files = new Set(await readdir(join(skillsRoot, entry.name)))
    if (!files.has("SKILL.md")) continue
    await cp(join(skillsRoot, entry.name), join(workerSkillsDir, entry.name), { recursive: true })
    placed.push(entry.name)
  }
  return placed
}
