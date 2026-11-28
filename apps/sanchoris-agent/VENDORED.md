# Code adapted from T3 Code

The Worker adapters are adapted from [T3 Code](https://github.com/pingdotgg/t3code) at commit `322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`, under the MIT License below. T3 Code builds its adapters on Effect; the parts taken here are rewritten as plain async TypeScript. No file is copied verbatim.

| File here | Adapted from | What was taken |
|---|---|---|
| `src/workers/claude.ts` | `apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts` | One Claude Agent SDK `query()` per conversation in streaming-input mode fed by a message queue; a text turn as an `SDKUserMessage`; `resume` for a conversation that has turns and `sessionId` for a new one (`makeClaudeQueryOptions`); `bypassPermissions` with `allowDangerouslySkipPermissions`; `thinking: { type: "adaptive", display: "summarized" }` with `showThinkingSummaries`; the `claude_code` system prompt preset; a steer as a user message with `priority: "now"` into the active turn only (`steerTurn`); `/compact` as the message of a compaction turn (`compactThread`); `rate_limit_event` with `status: "rejected"` as the usage limit and `resetsAt`; the `result` message as the end of a turn; `interrupt()` on the query |
| `src/workers/codex.ts` | `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts` | The app-server lifecycle `initialize` (with `capabilities.experimentalApi`), `initialized`, `thread/start` or `thread/resume` with `cwd`, `model` and the thread config `tools.update_plan.enabled` (`codexThreadRuntimeParams`), `turn/start` with `cwd`, `model`, `summary: "detailed"` and `collaborationMode` (`buildCodexTurnStartParams`), `turn/completed`, `turn/interrupt`; full access as `approvalPolicy: "never"` and `approvalsReviewer: "user"` with `sandboxPolicy: { type: "dangerFullAccess" }` (`codexRuntimeModeTurnDefaults`); `turn/steer` with `expectedTurnId`; `thread/compact/start`, whose turn arrives with `turn/started`; `thread/tokenUsage/updated` for the usage of a turn; `account/rateLimits/updated` merged for the main allowance with the windows named by `windowDurationMins`; `usageLimitExceeded`, `rateLimitExceeded` and HTTP 429 as the usage limit (`codexUsageLimitError`); the `agentMessage`, `commandExecution`, `reasoning` and `fileChange` items |
| `src/workers/codex-rpc.ts` | `packages/effect-codex-app-server/src/protocol.ts` | The app-server wire format: newline-delimited JSON without a `jsonrpc` field, requests `{ id, method, params }`, notifications `{ method, params }`, responses `{ id, result \| error }`, and requests from the server answered by `id` |

Left out: approvals, user questions and plan mode (a Workspace runs with full access and no one answers questions in the turn), attachments, subagent and background task tracking, moving foreground subagents to the background before a steer, MCP servers and device tools, and the settings T3 Code reads from its launch arguments.

## License

```text
MIT License

Copyright (c) 2026 T3 Tools Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
