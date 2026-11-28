import { rm } from "node:fs/promises"
import { createConnection, createServer, type Server } from "node:net"

import { SocketRequest, SocketResponse } from "./protocol.ts"

const MAX_LINE = 1024 * 1024

/** Serves the `sanchoris-agent` commands: one JSON request line, one JSON response line. */
export async function serveSocket(
  path: string,
  handle: (request: SocketRequest) => Promise<SocketResponse>,
): Promise<Server> {
  await rm(path, { force: true })
  const server = createServer((connection) => {
    let buffer = ""
    connection.setEncoding("utf8")
    connection.on("data", (chunk: string) => {
      buffer += chunk
      if (buffer.length > MAX_LINE) {
        connection.destroy()
        return
      }
      const end = buffer.indexOf("\n")
      if (end < 0) return
      const line = buffer.slice(0, end)
      buffer = ""
      connection.pause()
      let request: SocketRequest
      try {
        request = SocketRequest.parse(JSON.parse(line))
      } catch (error) {
        connection.end(
          `${JSON.stringify({ ok: false, error: { code: "invalid_request", message: (error as Error).message } })}\n`,
        )
        return
      }
      void handle(request)
        .catch((error: Error) => ({
          ok: false as const,
          error: { code: "agent_error", message: error.message },
        }))
        .then((response) => connection.end(`${JSON.stringify(response)}\n`))
    })
    connection.on("error", () => {})
  })
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject)
    server.listen(path, () => resolve())
  })
  return server
}

export function callSocket(path: string, request: SocketRequest): Promise<SocketResponse> {
  return new Promise((resolve, reject) => {
    const connection = createConnection(path)
    let buffer = ""
    connection.setEncoding("utf8")
    connection.on("connect", () => connection.write(`${JSON.stringify(request)}\n`))
    connection.on("data", (chunk: string) => {
      buffer += chunk
    })
    connection.on("end", () => {
      try {
        resolve(SocketResponse.parse(JSON.parse(buffer)))
      } catch (error) {
        reject(error as Error)
      }
    })
    connection.on("error", reject)
  })
}
