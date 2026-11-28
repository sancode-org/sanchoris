import { readdir, readFile } from "node:fs/promises"
import { join, resolve } from "node:path"

import { describe, expect, test } from "vite-plus/test"

import { InboundMessage, OutboundMessage } from "../src/protocol.ts"

const dir = resolve(import.meta.dirname, "..", "protocol")

async function examples(direction: string): Promise<{ name: string; message: unknown }[]> {
  const names = (await readdir(join(dir, direction)))
    .filter((name) => name.endsWith(".json"))
    .sort()
  return Promise.all(
    names.map(async (name) => ({
      name,
      message: JSON.parse(await readFile(join(dir, direction, name), "utf8")) as unknown,
    })),
  )
}

describe("Thread channel examples", () => {
  test("every control-to-agent example parses, and every type has one", async () => {
    const parsed = (await examples("control-to-agent")).map(({ message }) =>
      InboundMessage.parse(message),
    )
    expect(new Set(parsed.map((message) => message.type))).toEqual(
      new Set([
        "credentials",
        "start",
        "input",
        "interrupt",
        "shutdown",
        "compact",
        "exec",
        "script",
        "skill_response",
        "thread_response",
        "continue_output",
        "ack",
        "close",
      ]),
    )
  })

  test("every agent-to-control example parses, and every type has one", async () => {
    const parsed = (await examples("agent-to-control")).map(({ message }) =>
      OutboundMessage.parse(message),
    )
    expect(new Set(parsed.map((message) => message.type))).toEqual(
      new Set(OutboundMessage.options.map((option) => option.shape.type.value)),
    )
  })

  test("a message with a field the schema does not know is rejected", () => {
    expect(() =>
      OutboundMessage.parse({
        type: "heartbeat",
        threadId: "th_1",
        turnRunning: true,
        output: false,
        extra: 1,
      }),
    ).toThrow()
  })
})
