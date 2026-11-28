import { describe, expect, test } from "vite-plus/test"
import { canStartWorkspace } from "../src/generated/Sanchoris/Host.ts"

describe("canStartWorkspace", () => {
  test("an online host below its limit can start a workspace", () => {
    expect(canStartWorkspace(true, 1n, 2n)).toBe(true)
  })

  test("a host at its limit cannot start a workspace", () => {
    expect(canStartWorkspace(true, 2n, 2n)).toBe(false)
  })

  test("an offline host cannot start a workspace", () => {
    expect(canStartWorkspace(false, 0n, 2n)).toBe(false)
  })
})
