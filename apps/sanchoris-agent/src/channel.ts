import { randomUUID } from "node:crypto"

import WebSocket from "ws"

import { fitFrame } from "./frame.ts"
import type { Logger } from "./log.ts"
import {
  type CredentialsMessage,
  InboundMessage,
  type OutboundMessage,
  PROTOCOL_VERSION,
  type SequencedMessage,
  type SequencedPayload,
  type SkillResponseMessage,
  type ThreadResponseMessage,
} from "./protocol.ts"

type ReplyMessage = SkillResponseMessage | ThreadResponseMessage
export type InboundCommand = Exclude<
  InboundMessage,
  ReplyMessage | { type: "ack" | "close" | "credentials" }
>

interface PendingReply {
  resolve: (message: ReplyMessage) => void
  reject: (error: Error) => void
  timer: NodeJS.Timeout
}

export interface ThreadChannelOptions {
  url: string
  bootstrapToken: string
  threadId: string
  logger: Logger
  started: () => boolean
  onCommand: (message: InboundCommand) => void
  onCredentials: (message: CredentialsMessage) => void
  onFatal: (error: Error) => void
  initialBackoffMs: number
  maxBackoffMs: number
}

export class ProtocolError extends Error {}

const PING_INTERVAL_MS = 30_000

/** The agent's end of the Thread channel. */
export class ThreadChannel {
  private readonly options: ThreadChannelOptions
  private readonly instanceId = randomUUID()
  private token: string
  private credentialed = false
  private socket: WebSocket | null = null
  private seq = 0
  private lastInboundSeq = 0
  private unacked: SequencedMessage[] = []
  private inFlight = 0
  private helloAcked = false
  private readonly replies = new Map<string, PendingReply>()
  private backoffMs: number
  private reconnectTimer: NodeJS.Timeout | null = null
  private pingTimer: NodeJS.Timeout | null = null
  private closed = false
  private readonly ackWaiters: Array<() => void> = []

  constructor(options: ThreadChannelOptions) {
    this.options = options
    this.token = options.bootstrapToken
    this.backoffMs = options.initialBackoffMs
  }

  connect(): void {
    if (this.closed) return
    const socket = new WebSocket(this.options.url, {
      headers: { Authorization: `Bearer ${this.token}` },
    })
    this.socket = socket
    socket.on("open", () => {
      this.backoffMs = this.options.initialBackoffMs
      if (this.pingTimer) clearInterval(this.pingTimer)
      this.pingTimer = setInterval(() => socket.ping(), PING_INTERVAL_MS)
      this.helloAcked = false
      this.inFlight = 0
      this.write({
        type: "hello",
        threadId: this.options.threadId,
        protocolVersion: PROTOCOL_VERSION,
        instanceId: this.instanceId,
        started: this.options.started(),
        lastSeq: this.lastInboundSeq,
      })
    })
    socket.on("message", (data: Buffer) => this.receive(data.toString("utf8")))
    socket.on("unexpected-response", (_request, response) => {
      const status = response.statusCode ?? 0
      if (status === 401 || status === 403) {
        this.fail(new ProtocolError(`Thread channel rejected the token with HTTP ${status}`))
        return
      }
      this.options.logger("warn", "thread channel upgrade failed", { status })
      socket.terminate()
    })
    socket.on("error", (error) => {
      this.options.logger("warn", "thread channel error", { error: error.message })
    })
    socket.on("close", (code, reason) => {
      if (this.socket === socket) this.socket = null
      if (this.pingTimer && this.socket === null) {
        clearInterval(this.pingTimer)
        this.pingTimer = null
      }
      if (this.closed) return
      if (code >= 4000 && code <= 4999) {
        this.fail(new ProtocolError(`Thread channel closed: ${code} ${reason.toString()}`))
        return
      }
      if (!this.credentialed) {
        this.fail(new ProtocolError("Thread channel closed before credentials arrived"))
        return
      }
      this.scheduleReconnect()
    })
  }

  /** Sends a sequenced message; it is sent again after a reconnect until acknowledged. */
  send(payload: SequencedPayload): void {
    this.seq += 1
    this.unacked.push(
      fitFrame({ ...payload, threadId: this.options.threadId, seq: this.seq } as SequencedMessage),
    )
    this.flush()
  }

  request(
    build: (requestId: string) => SequencedPayload,
    timeoutMs: number,
  ): Promise<ReplyMessage> {
    const requestId = randomUUID()
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.replies.delete(requestId)
        reject(new Error(`No reply from the control plane within ${timeoutMs} ms`))
      }, timeoutMs)
      this.replies.set(requestId, { resolve, reject, timer })
      this.send(build(requestId))
    })
  }

  heartbeat(turnRunning: boolean, output: boolean): void {
    if (this.helloAcked)
      this.write({ type: "heartbeat", threadId: this.options.threadId, turnRunning, output })
  }

  /** Waits until the control plane has acknowledged every sequenced message. */
  async drain(timeoutMs: number): Promise<void> {
    if (this.unacked.length === 0) return
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`${this.unacked.length} messages were not acknowledged`)),
        timeoutMs,
      )
      this.ackWaiters.push(() => {
        clearTimeout(timer)
        resolve()
      })
    })
  }

  close(): void {
    this.closed = true
    if (this.pingTimer) clearInterval(this.pingTimer)
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    for (const reply of this.replies.values()) {
      clearTimeout(reply.timer)
      reply.reject(new Error("Thread channel closed"))
    }
    this.replies.clear()
    this.socket?.close(1000, "shutdown")
  }

  /** Keeps one sequenced message in flight: the WebSocket API may process frames out of order. */
  private flush(): void {
    if (this.socket?.readyState !== WebSocket.OPEN || !this.helloAcked) return
    const next = this.unacked[0]
    if (next === undefined || next.seq <= this.inFlight) return
    this.write(next)
    this.inFlight = next.seq
  }

  private write(message: OutboundMessage): void {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(JSON.stringify(message))
  }

  private receive(raw: string): void {
    let message: InboundMessage
    try {
      message = InboundMessage.parse(JSON.parse(raw))
    } catch (error) {
      this.fail(new ProtocolError(`Invalid inbound message: ${(error as Error).message}`))
      return
    }
    if (message.threadId !== this.options.threadId) {
      this.fail(new ProtocolError(`Message for thread ${message.threadId}`))
      return
    }
    switch (message.type) {
      case "ack":
        this.helloAcked = true
        this.unacked = this.unacked.filter((sent) => sent.seq > message.seq)
        this.flush()
        if (this.unacked.length === 0) for (const waiter of this.ackWaiters.splice(0)) waiter()
        return
      case "close":
        this.fail(new ProtocolError(`Thread channel closed: ${message.code} ${message.reason}`))
        return
      default:
        break
    }
    if (message.seq <= this.lastInboundSeq) return
    this.lastInboundSeq = message.seq
    switch (message.type) {
      case "credentials":
        this.credentialed = true
        this.token = message.threadToken
        this.options.onCredentials(message)
        return
      case "skill_response":
      case "thread_response": {
        const reply = this.replies.get(message.requestId)
        if (!reply) return
        this.replies.delete(message.requestId)
        clearTimeout(reply.timer)
        reply.resolve(message)
        return
      }
      default:
        this.options.onCommand(message)
    }
  }

  private scheduleReconnect(): void {
    const delay = this.backoffMs
    this.backoffMs = Math.min(this.backoffMs * 2, this.options.maxBackoffMs)
    this.options.logger("info", "thread channel reconnecting", { delayMs: delay })
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, delay)
  }

  private fail(error: Error): void {
    this.close()
    this.options.onFatal(error)
  }
}
