import { describe, expect, test } from "vite-plus/test"

import { afterTurn, deliver } from "../src/generated/Sanchoris/AgentInput.ts"

describe("deliver (generated from Sanchoris.AgentInput)", () => {
  test("starts a turn when no turn runs, whatever the delivery", () => {
    expect(deliver("idle", "steer", true)).toBe("startTurn")
    expect(deliver("idle", "turn", false)).toBe("startTurn")
  })

  test("steers only into a running turn of a Worker that takes steers", () => {
    expect(deliver("running", "steer", true)).toBe("steer")
    expect(deliver("running", "steer", false)).toBe("queue")
    expect(deliver("preparing", "steer", true)).toBe("queue")
  })

  test("keeps a turn input for the next turn while a turn is being prepared or runs", () => {
    expect(deliver("running", "turn", true)).toBe("queue")
    expect(deliver("preparing", "turn", true)).toBe("queue")
  })
})

describe("afterTurn (generated from Sanchoris.AgentInput)", () => {
  test("compacts first, then starts a turn with the waiting inputs, and stays idle when stopping", () => {
    expect(afterTurn(false, true, 2)).toBe("compact")
    expect(afterTurn(false, false, 2)).toBe("turn")
    expect(afterTurn(false, false, 0)).toBe("idle")
    expect(afterTurn(true, true, 2)).toBe("idle")
  })
})
