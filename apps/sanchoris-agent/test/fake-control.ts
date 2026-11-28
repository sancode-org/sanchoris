import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http"
import type { AddressInfo } from "node:net"

import WebSocket, { WebSocketServer } from "ws"

import { type InboundMessage, OutboundMessage } from "../src/protocol.ts"

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never
export type ControlPayload = DistributiveOmit<
  Exclude<InboundMessage, { type: "ack" | "close" }>,
  "threadId" | "seq"
>
type Received = OutboundMessage

interface Waiter {
  match: (message: Received) => boolean
  resolve: (message: Received) => void
}

/** A control plane for one Workspace: the Thread channel, record storage and file downloads. */
export class FakeControl {
  readonly threadId: string
  readonly bootstrapToken = "bootstrap-token"
  threadToken = "thread-token-1"
  githubToken = "ghs_fake_1"
  readonly records = new Map<string, Buffer>()
  readonly files = new Map<string, Buffer>()
  readonly received: Received[] = []
  readonly connections: string[] = []
  start: ControlPayload | null = null
  /** Replies to `skill_request` and `thread_request`; `null` leaves the request unanswered. */
  answer: (message: Received) => ControlPayload | null = () => null
  /** Sees every message after `hello`, before `next` does. */
  observe: (message: Received) => void = () => {}
  private readonly server: Server
  private readonly wss = new WebSocketServer({ noServer: true, maxPayload: 128 * 1024 })
  private socket: WebSocket | null = null
  private bootstrapUsed = false
  private readonly outbox: InboundMessage[] = []
  private acked = 0
  private readonly waiters: Waiter[] = []

  private constructor(threadId: string) {
    this.threadId = threadId
    this.server = createServer((request, response) => void this.http(request, response))
    this.server.on("upgrade", (request, socket, head) => {
      const token = request.headers.authorization?.replace(/^Bearer /, "")
      const bootstrap = token === this.bootstrapToken && !this.bootstrapUsed
      if (!bootstrap && token !== this.threadToken) {
        socket.end("HTTP/1.1 401 Unauthorized\r\n\r\n")
        return
      }
      if (bootstrap) this.bootstrapUsed = true
      this.connections.push(bootstrap ? "bootstrap" : "thread")
      this.wss.handleUpgrade(request, socket, head, (ws) => this.accept(ws, bootstrap))
    })
  }

  static async listen(threadId = "th_1"): Promise<FakeControl> {
    const control = new FakeControl(threadId)
    await new Promise<void>((resolve) => control.server.listen(0, "127.0.0.1", resolve))
    return control
  }

  get httpUrl(): string {
    return `http://127.0.0.1:${(this.server.address() as AddressInfo).port}`
  }

  get wsUrl(): string {
    return `ws://127.0.0.1:${(this.server.address() as AddressInfo).port}/threads/channel`
  }

  credentials(): ControlPayload {
    const expiresAt = new Date(Date.now() + 15 * 60_000).toISOString()
    return {
      type: "credentials",
      threadToken: this.threadToken,
      threadTokenExpiresAt: expiresAt,
      github: { token: this.githubToken, expiresAt },
      recordUpload: {
        url: `${this.httpUrl}/records`,
        fields: { policy: "fake" },
        keyPrefix: `u/user1/threads/${this.threadId}/records/`,
        expiresAt,
      },
    }
  }

  send(payload: ControlPayload): void {
    const message = {
      ...payload,
      threadId: this.threadId,
      seq: this.outbox.length + 1,
    } as InboundMessage
    this.outbox.push(message)
    this.socket?.send(JSON.stringify(message))
  }

  next<T extends Received["type"]>(
    type: T,
    match: (message: Extract<Received, { type: T }>) => boolean = () => true,
    timeoutMs = 20_000,
  ): Promise<Extract<Received, { type: T }>> {
    const matches = (message: Received) =>
      message.type === type && match(message as Extract<Received, { type: T }>)
    const found = this.received.find(matches)
    if (found) {
      this.received.splice(this.received.indexOf(found), 1)
      return Promise.resolve(found as Extract<Received, { type: T }>)
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`No ${type} within ${timeoutMs} ms`)),
        timeoutMs,
      )
      this.waiters.push({
        match: matches,
        resolve: (message) => {
          clearTimeout(timer)
          resolve(message as Extract<Received, { type: T }>)
        },
      })
    })
  }

  /** Drops the connection, as API Gateway does after its idle and lifetime limits. */
  drop(): void {
    this.socket?.terminate()
    this.socket = null
  }

  async close(): Promise<void> {
    this.socket?.terminate()
    this.wss.close()
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }

  private accept(ws: WebSocket, bootstrap: boolean): void {
    this.socket = ws
    ws.on("message", (data: Buffer) => {
      const message = OutboundMessage.parse(JSON.parse(data.toString("utf8")))
      if (message.type === "hello") {
        ws.send(JSON.stringify({ type: "ack", threadId: this.threadId, seq: this.acked }))
        for (const pending of this.outbox.slice(message.lastSeq)) ws.send(JSON.stringify(pending))
        if (bootstrap) this.send(this.credentials())
        if (!message.started && this.start) this.send(this.start)
        return
      }
      if ("seq" in message) {
        if (message.seq <= this.acked) {
          ws.send(JSON.stringify({ type: "ack", threadId: this.threadId, seq: this.acked }))
          return
        }
        this.acked = message.seq
        ws.send(JSON.stringify({ type: "ack", threadId: this.threadId, seq: this.acked }))
      }
      this.observe(message)
      if (message.type === "skill_request" || message.type === "thread_request") {
        const reply = this.answer(message)
        if (reply) this.send(reply)
      }
      const waiter = this.waiters.find((candidate) => candidate.match(message))
      if (waiter) {
        this.waiters.splice(this.waiters.indexOf(waiter), 1)
        waiter.resolve(message)
      } else this.received.push(message)
    })
  }

  private async http(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const url = new URL(request.url ?? "/", this.httpUrl)
    if (request.method === "POST" && url.pathname === "/records") {
      const chunks: Buffer[] = []
      for await (const chunk of request) chunks.push(chunk as Buffer)
      const form = await new Request(this.httpUrl, {
        method: "POST",
        headers: { "content-type": request.headers["content-type"] ?? "" },
        body: Buffer.concat(chunks),
      }).formData()
      const key = form.get("key")
      const file = form.get("file")
      if (typeof key !== "string" || !(file instanceof Blob) || form.get("policy") !== "fake") {
        response.writeHead(400).end()
        return
      }
      this.records.set(key, Buffer.from(await file.arrayBuffer()))
      response.writeHead(204).end()
      return
    }
    const name = decodeURIComponent(url.pathname.replace(/^\/(records|files)\//, ""))
    const body = url.pathname.startsWith("/records/")
      ? this.records.get(name)
      : this.files.get(name)
    if (request.method === "GET" && body) {
      response.writeHead(200, { "content-type": "application/octet-stream" }).end(body)
      return
    }
    response.writeHead(404).end()
  }
}
