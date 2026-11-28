# Architecture

The design of sanchoris is in `projects/sanchoris/` of the `conao3/idea` repository. This document holds what that design leaves to the implementation: the keys of the DynamoDB tables, how the processes and screens read them, and the rules of the writes.

## Tables

`infra/tables.json` defines the tables, and `sanchoris_core::schema::create_tables` creates them from it for DynamoDB Local.

- `main`: the entities of `README.md`「データ」, the outbox of the Thread channel, the jobs, the marks, and the counters. Keys `pk` and `sk`; `gsi1` (lists within a User) and `gsi2` (sweeps); TTL on `expires_at` (Unix seconds); a stream of new images.
- `audit`: the audit events, appended only.

Every item has `kind`, the name of what it holds. Times in items are milliseconds since the Unix epoch, except `expires_at`. Numbers in sort keys have 20 digits, so that they sort as numbers.

## Keys

Every key of a User is in the partition `U#<user>`, or in a partition that starts with `U#<user>#`. The unit of work of a User (`sanchoris_core::db::UserTx`) rejects any other key, and an action on an item that the transaction already writes.

| `sk` | `kind` | What it holds |
|---|---|---|
| `USER` | `user` | The User, with the counters of its period: `period_threads`, `period_workspace_seconds`, `running_workspaces` |
| `CRED#<name>` | `credential` | A Credential |
| `CRED#<name>#SNAP#<taken_at>` | `usage_snapshot` | A Usage snapshot of the Credential |
| `INTEG#<kind>#<id>` | `integration` | An Integration |
| `HOST#<name>` | `host` | A Host, with `running_workspaces` |
| `CONFIG#<repository ID>` | `config` | A Config, by the GitHub ID of its repository |
| `CONFIG#<repository ID>#PROJECT#<name>` | `project` | A Project |
| `CONFIG#<repository ID>#PROJECT#<project>#REPO#<name>` | `repository` | A Repository of a Project |
| `CONFIG#<repository ID>#SKILL#<name>` | `skill` | A Skill |
| `CONFIG#<repository ID>#TRIGGER#<name>` | `trigger` | A Trigger |
| `CONFIG#<repository ID>#TRIGGER#<name>#EVENT#<delivery>` | `trigger_event` | The Trigger event of a delivery, written once |
| `TKEY#<repository ID>#<trigger>#<thread key>` | `thread_key` | The thread that holds a thread key of a Trigger, until it is `ABORTED`, or `expires_at` (the `RetentionDays` of the Plan) passes |
| `TRIGHOUR#<repository ID>#<trigger>#<hour>` | `trigger_hour` | `started`: the threads the Trigger started in the hour (UTC) |
| `THREAD#<name>` | `thread` | A Thread, with `version` |
| `THREAD#<name>#MSG#<seq>` | `message` | A Message |
| `THREAD#<name>#TURN#<seq>` | `turn` | A Turn |
| `THREAD#<name>#WS#<seq>` | `workspace` | A Workspace; the last one is the Workspace of the thread |
| `THREAD#<name>#USAGE` | `usage` | The Usage of the thread |
| `THREAD#<name>#PR#<repository ID>#<number>` | `pull_request` | A Pull request |
| `THREAD#<parent>#CHILD#<child>` | `child` | A child thread under its parent, with how it was made (`subagent`, `fork`) |
| `THREAD#<name>#OUT#<seq>` | `outbox` | Messages of the Thread channel from `first_seq`, one item for each write |
| `THREAD#<name>#OUTSEQ` | `outbox_seq` | `last`: the last `seq` of the outbox |
| `THREAD#<name>#INST#<instance>` | `inbound` | `seq`: the last message of an `instanceId` of `sanchoris-agent` that the control plane took |
| `THREAD#<name>#RUN#<run>` | `thread_run` | A SkillRun of the thread |
| `THREAD#<name>#AWAIT` | `await_order` | `order`: the order of the last `await` of the thread |
| `RUN#<run>` | `skill_run` | A SkillRun: its record (`body`), `thread`, `status`, `version` |
| `RUN#<run>#E#<page>` | `state_entries` | The StateEntries `16 × page` to `16 × page + 15` of a SkillRun |
| `RUN#<run>#EXEC#<seq>#<attempt>` | `exec_result` | The result of a command by its idempotency key, written once |
| `RUN#<run>#XREQ#<seq>#<attempt>` | `exec_request` | The command a transition sent with the key |
| `REQ#<request>` | `engine_response` | The answer to a `skill_request` |
| `EJOB#<id>` | `engine_job` | A job of the engine: a command to send, or a timer, with `due` and `thread` |
| `SEQ#RUN`, `SEQ#EJOB` | `sequence` | `last`: the last SkillRun and the last job of the engine |
| `JOB#<id>` | `job` | A job of a process, such as `start#<thread>` for the thread-starter |
| `MARK#<kind>#<id>` | `mark` | A mark that makes an operation happen once: a GitHub delivery, a Stripe event, a spent bootstrap token (`bootstrap#<token ID>`), a message of an agent (`in#<thread>#<instance>#<seq>`), a notification delivered to a thread (`notice#<thread>#<outbox seq>`) |

The audit table keys an audit event by `U#<user>` and `AUDIT#<at>#<id>`.

S3 keeps the objects of a User under `u/<user>/`:

| Key | What it holds |
|---|---|
| `u/<user>/runs/<run>/<SHA-256>` | The values of SkillRuns |
| `u/<user>/skills/<SHA-256>` | The document and definition of a Skill of a Config, and the skills archive of a Workspace |
| `u/<user>/threads/<name>/definition.json` | The `Definition` of a thread: its Project and callable Skills at the start |
| `u/<user>/threads/<name>/records/<turn>.tar.gz` | The thread records, which `sanchoris-agent` uploads |
| `u/<user>/threads/<name>/log/<at>-<instance>-<seq>.json` | The log of a thread: the `event`, `script_result`, `rate_limit` and `stopping` messages |

## Writes

- One write is one `TransactWriteItems` of one User. A write over an item it read requires the `version` it read; a new item requires that it does not exist. A write whose condition fails reads again and decides again.
- Counters change with `ADD` under a condition: a new thread counts in `period_threads` while it is below the threads of the Plan, and a cloud Workspace counts in `running_workspaces` while it is below the concurrency of the Plan. A Workspace gives back its slot when it becomes `stopped`, under the condition that it was not.
- A transition of the engine is one `TransactWriteItems` (`sanchoris_core::engine_store`):
  - The values of a SkillRun and its StateEntries, the output of commands and the `skill.yaml` are slots: `{"$v": value}` in the item, or `{"$b": key}` with the value in S3, named by its SHA-256. A slot over 32 KiB goes to S3. Then, while an item is over 380,000 bytes or the transaction over 3,900,000 bytes, the largest slot left in it goes to S3. The objects are written before the transaction; a transaction that fails leaves objects that nothing names.
  - StateEntries are kept 16 to an item, so that the 256 StateEntries of the largest transition are 17 items. The messages of a transition are one outbox item.
  - A transition writes the SkillRun (with its place in the thread and the count of SkillRuns when it is new), the pages of its StateEntries, its command and timers with the count of jobs, the outbox and its count, the answer and the order of `await`: 25 items for 256 StateEntries, far under the 100 actions of a transaction.
- The outbox numbers its messages in order: a write reads `last` and requires it. A `skill_notice` in the outbox is a notification of a SkillRun, which the thread-supervisor delivers as an input of the thread.

## Thread starts

The thread-starter (`sanchoris_core::thread::start_pending`) decides with `Sanchoris.Thread.decideStart`, in this order: an open blocked-by issue (`blocked_by`), the subscription (`subscription`), the threads and the Workspace time of the period of the Plan (`plan_limit`), then the place of the Workspace (`Sanchoris.Host.place`): the concurrency of the Plan for `cloud` (`concurrency`), the room of the Host otherwise (`host_full`; a Host that is `offline` has no room). A Project whose Host has no Host of that name makes the thread `NEEDS_YOU` with `Host.Missing`.

## Trigger evaluation

`sanchoris_core::trigger::evaluate` takes an event and the Triggers of a Config. For each Trigger whose `EventPattern` matches, with `detail.repository` matched by the repository IDs of the sync, it records one Trigger event for the delivery:

1. A delivery with a Trigger event is a duplicate, and nothing is written.
2. An event without a value of `InputPathsMap`, or without its thread key, is dropped (evaluation).
3. A thread key that a thread holds is skipped (existing thread).
4. A Trigger that started 60 threads in the hour drops the event (Trigger limit).
5. An issue whose dependencies cannot be read is skipped (dependencies).
6. Otherwise one transaction writes the Trigger event, the thread key, the count of the hour, the thread, its first Message and the job of the thread-starter.

The thread of a delivery is named by the SHA-256 of the Config, the Trigger and the delivery.

## Thread channel

The control side of the Thread channel is `sanchoris_worker::channel`: `connect` checks the token, `receive` takes one frame and `disconnect` ends a connection. A handler keeps nothing between calls; the User, thread and Workspace of a connection come from its token.

- Tokens are HMAC-SHA256 under `THREAD_TOKEN_SIGNING_KEY` and name the User, the thread and the Workspace. A bootstrap token is spent by its mark; a token for a Workspace that is not the last of its thread, or is `stopped`, is refused with 403.
- A message with `seq` takes effect once: its writes and the mark `in#<thread>#<instance>#<seq>` are one transaction, and a message at or below the `inbound` cursor is acknowledged again.
- The dispatcher is the one sender of the outbox. A Workspace keeps its `connection`, the `instance` of its agent, `outbox_from` (the outbox before it, which it does not get) and `sent` (the last message sent on the connection). `hello` sets them, and the dispatcher sends the outbox after `sent` in order whenever the outbox or the Workspace changes.
- `start` and `credentials` are written as stubs and made when they are sent: the tokens, the provider credential and the presigned URLs are never stored. The first `start` of a thread takes its queued inputs as the instruction; a restored Workspace takes the inputs that waited, its thread record, and the commands of the SkillRuns again on `ready`.
- `turn_end` writes the Turn, the reply as a Message, the counts and the Usage and the step of the thread in one transaction. A `skill_notice` of the outbox becomes an input of the thread with its Message.

## Local control plane

`sanchoris_api::local` runs the control plane in one process: the dispatcher reads the stream of DynamoDB Local, one queue in memory runs the jobs in order, a timer stands in for each schedule, the Workspaces are `sanchoris-agent` processes (`sanchoris_worker::local::LocalLauncher`, behind `Launcher` with the ECS and Host ones), the presigned URLs are signed routes of the server (`/local/blobs/<key>`, `/local/records`), and bare repositories stand in for GitHub. `.env.example` has its settings.
