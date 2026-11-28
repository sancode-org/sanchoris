import { describe, expect, test } from "vite-plus/test"

import { parseCommand } from "../src/main.ts"
import type { SkillEntry } from "../src/protocol.ts"
import { validateSkillRun } from "../src/skills.ts"

describe("sanchoris-agent commands", () => {
  test("skill run, continue, await and status", () => {
    expect(
      parseCommand(["skill", "run", "open-pr-green-ci", "--input", '{"Issue":35}'], {}),
    ).toEqual({
      kind: "skill",
      request: { action: "run", name: "open-pr-green-ci", input: { Issue: 35 } },
    })
    expect(parseCommand(["skill", "continue", "3", "--output", '{"ok":true}'], {})).toEqual({
      kind: "skill",
      request: { action: "continue", runId: 3, output: { ok: true } },
    })
    expect(parseCommand(["skill", "await"], { SANCHORIS_SKILL_RUN_ID: "3" })).toEqual({
      kind: "skill",
      request: { action: "await", runId: 3 },
    })
    expect(parseCommand(["skill", "status", "3"], {})).toEqual({
      kind: "skill",
      request: { action: "status", runId: 3 },
    })
    expect(() => parseCommand(["skill", "await"], {})).toThrow("positive integer")
    expect(() => parseCommand(["skill", "continue", "3", "--output", "{"], {})).toThrow(
      "--output must be JSON",
    )
  })

  test("thread start, send, stop and status", () => {
    expect(
      parseCommand(
        ["thread", "start", "--message", "Add subtract", "--model", '{"Agent":"codex"}'],
        {},
      ),
    ).toEqual({
      kind: "thread",
      request: {
        action: "start",
        message: "Add subtract",
        project: null,
        model: { Agent: "codex" },
      },
    })
    expect(parseCommand(["thread", "send", "th_2", "--message", "Also tests"], {})).toEqual({
      kind: "thread",
      request: { action: "send", name: "th_2", message: "Also tests" },
    })
    expect(parseCommand(["thread", "stop", "th_2"], {})).toEqual({
      kind: "thread",
      request: { action: "stop", name: "th_2" },
    })
    expect(() => parseCommand(["thread", "start"], {})).toThrow("--message is required")
  })

  test("skill run is checked against the predictable skills and their Inputs", () => {
    const skills: SkillEntry[] = [
      {
        name: "open-pr-green-ci",
        predictable: true,
        inputs: { Repo: "string" as const, Issue: "number" as const },
      },
      { name: "notes", predictable: false, inputs: {} },
    ]
    expect(validateSkillRun(skills, "open-pr-green-ci", { Repo: "frontend", Issue: 35 })).toBeNull()
    expect(validateSkillRun(skills, "notes", {})?.code).toBe("unknown_skill")
    expect(validateSkillRun(skills, "open-pr-green-ci", [])?.code).toBe("invalid_input")
    expect(
      validateSkillRun(skills, "open-pr-green-ci", { Repo: "frontend", Issue: 35, Extra: 1 })
        ?.message,
    ).toContain("Extra is not an input")
  })
})
