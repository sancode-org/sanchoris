# Thread channel (protocol version 1)

The Thread channel connects one `sanchoris-agent` process in a Workspace to the control plane. The schema in `src/protocol.ts` is the source of truth. `protocol/control-to-agent/*.json` and `protocol/agent-to-control/*.json` hold one example of every message type; `test/protocol.test.ts` parses each against the schema, and the control plane's tests read the same files.

## Environment

| Variable | Required | Value |
|---|---|---|
| `SANCHORIS_THREAD_ID` | yes | The thread's `Name`; every message carries it as `threadId` |
| `SANCHORIS_CHANNEL_URL` | yes | The WebSocket URL of the Thread channel (`wss://`, `ws://` locally) |
| `SANCHORIS_BOOTSTRAP_TOKEN` | yes | The bootstrap token: one connection, 15 minutes. The agent removes it from the environment of everything it starts |
| `SANCHORIS_WORKSPACE_DIR` | no (`/workspace`) | The working directory; repository `<name>` is cloned to `<dir>/<name>` |
| `SANCHORIS_SKILLS_DIR` | no (`/skills`) | Where the skill directories are unpacked (`/skills/<name>/`) |
| `SANCHORIS_STATE_DIR` | no (`~/.sanchoris-agent`) | The agent's files: the provider's state directory (`home/<agent>`), the GitHub token, the `gh` and `git` configuration, the `sanchoris-agent` wrapper |
| `SANCHORIS_SOCKET` | no (`<state>/agent.sock`) | The Unix socket of the `sanchoris-agent` commands |
| `SANCHORIS_GIT_URL_PREFIX` | no (`https://`) | Put before `repo` (`github.com/owner/name`) to clone |
| `SANCHORIS_CLAUDE_PATH`, `SANCHORIS_CODEX_PATH` | no | The `claude` and `codex` executables |

The Worker and the commands run with the agent's environment without the bootstrap token, plus `PATH` with the `sanchoris-agent` wrapper first, `GH_CONFIG_DIR` and `GIT_CONFIG_GLOBAL` (the GitHub token for `gh` and the credential helper `sanchoris-agent git-credential`), `SANCHORIS_SOCKET`, `SANCHORIS_THREAD_NAME`, and the provider's home variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) with the credential of `start`. `exec` adds `SANCHORIS_SKILL_RUN_ID` and `SANCHORIS_SKILL_DIR`.

## Transport

- The agent opens the WebSocket with `Authorization: Bearer <token>`: the bootstrap token first, and the thread token of the latest `credentials` on every reconnect. It sends a WebSocket ping every 30 s.
- The control plane rejects a bad or expired token with HTTP 401, and a spent bootstrap token with 403; the agent exits with code 1. A close code in 4000–4999, or `close`, also ends the agent with code 1. Any other close makes the agent reconnect with backoff (0.5 s doubling to 30 s), unless no `credentials` arrived yet.
- One text frame is one JSON object with `type` and `threadId`. Unknown fields, unknown types and another `threadId` are protocol errors: the agent exits with code 1.

## Delivery

- **Control plane to agent:** every message but `ack` and `close` comes from the thread's outbox with its `seq` (1, 2, … for one Workspace). The agent drops a `seq` it has already processed. `hello` carries `lastSeq`, the highest `seq` processed, and the control plane sends the outbox after it again.
- **Agent to control plane:** every message but `hello` and `heartbeat` carries `seq` (1, 2, … for one `instanceId`). The control plane answers `hello` with `ack` (`seq` = the highest of that `instanceId` it has stored, 0 for a new one) and each message with a cumulative `ack`. The agent keeps one message in flight, because the WebSocket API may hand frames to Lambda out of order, and sends the unacknowledged ones again after a reconnect. The control plane ignores a `(instanceId, seq)` it has stored and acknowledges it again.
- `hello` comes first on every connection. `started: false`: this process has not had `start`, so the control plane sends it. The first `hello` on the bootstrap token is answered with `credentials` before `start`.

## Control plane to agent

| `type` | Fields | Meaning |
|---|---|---|
| `credentials` | `threadToken`, `threadTokenExpiresAt`, `github` (`token`, `expiresAt`), `recordUpload` (`url`, `fields`, `keyPrefix`, `expiresAt`) | After the first `hello`, and again before the thread token (15 minutes) or the GitHub token (5 minutes before its hour) runs out. `recordUpload` is an S3 presigned POST whose policy allows keys under `keyPrefix` (`u/<user>/threads/<Name>/records/`) and expires with the thread token |
| `start` | `thread` (`name`, `title`), `project` (`name`, `repos`, `scripts`), `worker` (`agent`, `model`, `credential`), `gitAuthor`, `skills` (`url`, `sha256`, `entries`), `record`, `instruction`, `heartbeatSeconds` | Once per process. The agent unpacks `record` (the thread record to resume, with the provider `sessionId` and the `turn` it ends), clones each repository on its branch (`sanchoris/<Name>`), unpacks the skills archive into `/skills`, places the `SKILL.md` of every entry where the Worker reads its skills, puts the credential where the provider reads it, starts the Worker, answers `ready`, runs the `RunOnCreate` scripts, and starts the first turn with `instruction` (after the scripts with `async: false`; when one fails, `instruction` waits for the next `input`) |
| `input` | `inputId`, `text`, `delivery` (`steer`, `turn`) | An input of the thread. What the agent does is `Sanchoris.AgentInput.deliver`: with no turn it starts one; `steer` goes into a running turn of a Worker that takes steers; everything else waits and becomes the input of the next turn, joined by blank lines. A steer that the turn ends before also waits |
| `interrupt` | | Interrupts the turn; it ends with `turn_end` `result: "interrupted"` |
| `shutdown` | `reason` | Interrupts the turn, commits and pushes, waits up to 10 s for the `ack`s and exits 0 |
| `compact` | | Compacts the conversation in a turn of its own (Claude `/compact`, Codex `thread/compact/start`); during a turn it waits for the turn to end |
| `exec` | `idempotencyKey`, `skill` (`name`, `runId`), `run`, `stdin`, `timeoutSeconds` | Runs `run` with `bash -c` in the workspace directory with `stdin` on standard input, next to the turn. At `timeoutSeconds` it kills the process group. Answers `exec_result`; the same key is answered from the first run |
| `script` | `scriptRunId`, `name` | Runs the Project script `name` (from the screen) and answers `script_result` |
| `skill_response`, `thread_response` | `requestId`, `ok`, `result` or `error` (`code`, `message`) | The answer to `skill_request` and `thread_request` |
| `continue_output` | `runId`, `output` | The Output of the `continue` that `skill await` of SkillRun `runId` waits for. One that arrives before the `await` is kept for it |
| `ack` | `seq` | See "Delivery" |
| `close` | `code` (4000–4999), `reason` | Ends the agent with code 1 |

## Agent to control plane

| `type` | Fields | Meaning |
|---|---|---|
| `hello` | `protocolVersion`, `instanceId`, `started`, `lastSeq` | See "Delivery" |
| `heartbeat` | `turnRunning`, `output` | Every `heartbeatSeconds`: whether a turn runs and whether the Worker wrote anything since the last heartbeat |
| `ready` | `agent`, `sessionId` | The Worker runs; `sessionId` is the provider's conversation |
| `event` | `kind` (`text`, `thinking`, `tool_use`, `tool_result`, `log`, `system`), `data` | Worker output, tool calls, log lines; script output is a `log` with `script`, `scriptRunId`, `stdout`, `stderr` |
| `turn_end` | `turn`, `result` (`completed`, `failed`, `interrupted`), `summary`, `error` (`Worker.Failed`, `Worker.UsageLimit` with `resetsAt`), `usage`, `costUsd`, `durationMs`, `commits` | The end of a turn, after the agent committed each repository to its branch and pushed it (`commits`: `repo`, `branch`, `commit`, `pushed`), and after its `thread_record` |
| `rate_limit` | `status`, `resetsAt`, `windows` | The usage windows as the adapter reads them |
| `skill_request` | `requestId`, `request` (`run` with `name` and `input`, `continue` with `runId` and `output`, `await` with `runId`, `status` with `runId`) | From `sanchoris-agent skill ...` |
| `thread_request` | `requestId`, `request` (`start` with `message`, `project`, `model` and the `commits` pushed just before, `send` with `name` and `message`, `stop` and `status` with `name`) | From `sanchoris-agent thread ...` |
| `exec_result` | `idempotencyKey`, `exitCode`, `stdout`, `stderr` (the last 64 KiB of each), `timedOut` | |
| `script_result` | `scriptRunId` (`create:<name>` for `RunOnCreate`), `name`, `exitCode`, `stderrTail` (16 KiB), `timedOut` | |
| `credential_update` | `credential` | The provider refreshed its credential file (`claude_credentials_json`, `codex_auth_json`); checked after each turn and every minute |
| `thread_record` | `turn`, `sessionId`, `key`, `sha256`, `bytes` | The provider's state directory, without its credential file and its skills, as a gzipped tar uploaded to `<keyPrefix><turn as 6 digits>.tar.gz` once per turn, at its end. `shutdown` and `stopping` point at the record of the last turn |
| `stopping` | `commits`, `recordKey` | SIGTERM: the agent interrupted the turn, pushed, saved the record, and exits 0 |

## Commands

`sanchoris-agent skill ...` and `sanchoris-agent thread ...` write one JSON line to `SANCHORIS_SOCKET` and read one back: `{ "ok": true, "result": ... }` goes to stdout with exit code 0, `{ "ok": false, "error": ... }` to stderr with exit code 1. `skill run` is checked against the predictable skills of `start` and their `Inputs` before it reaches the control plane. `thread start` commits and pushes first. `skill await` takes the SkillRun from `SANCHORIS_SKILL_RUN_ID` and returns when `continue_output` arrives.
