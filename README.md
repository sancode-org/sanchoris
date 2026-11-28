# sanchoris

A hosted service that runs predictable skills inside AI coding agent threads. A skill shows its flow, its limits and its track record before it runs; the engine runs the fixed steps and hands the agent only the steps that need it. The agent runs on your own Claude and Codex subscriptions.

This repository is being built from scratch. The previous implementation is kept at [sancode-org/sanchoris-old](https://github.com/sancode-org/sanchoris-old) as a reference.

## Run

```sh
nix run                                     # control plane and screens on http://127.0.0.1:8080
curl http://127.0.0.1:8080/health           # ok
curl -H 'content-type: application/json' -d '{"query":"{ threads { name status } }"}' http://127.0.0.1:8080/graphql
nix run github:sancode-org/sanchoris#driver # sanchoris-driver
```

## Layout

| Path | Contents |
|---|---|
| `crates/api` | The `sanchoris` server (axum): GraphQL, the Thread channel's WebSocket, and the local control plane in one process |
| `crates/graphql` | GraphQL schema of the screens; `apps/frontend/src/graphql/schema.graphql` is its contract |
| `crates/config` | Reading and validating the configuration repository |
| `crates/engine` | The `skill.yaml` interpreter: the transitions of `Sanchoris.Engine` run against a store that writes each transition at once and a Workspace that runs the `Command` States |
| `crates/core` | Domain types, storage, triggers and thread progress |
| `crates/worker` | The Thread channel, the dispatcher and the jobs (thread-starter, restore, engine-worker, thread-supervisor), behind ports with local implementations |
| `crates/driver` | `sanchoris-driver`, which runs workspaces on a user's machine |
| `apps/frontend` | The screens (React 19, react-aria-components, Tailwind CSS v4, lucide-react, Apollo Client, TanStack Router) |
| `apps/sanchoris-agent` | `sanchoris-agent`, the program inside a workspace: the Thread channel (`PROTOCOL.md`), the Claude and Codex Workers adapted from T3 Code (`VENDORED.md`), and the `skill` and `thread` commands |
| `e2e` | End-to-end tests (`pnpm e2e`) with local Workspaces and the scripted fake Worker |
| `infra/tables.json` | The DynamoDB tables: `main` (keys `pk` and `sk`, indexes `gsi1` and `gsi2`) and the append-only `audit` |
| `lean` | Lean definitions and theorems, compiled to Rust and TypeScript by leanfran (`leanfran.toml`) |
| `audit` | leanassay audits of the theorems |

## Develop

```sh
nix develop
make check     # format, Lean proofs, audits, generated code, clippy, cargo test, vp run check, pnpm e2e
pnpm e2e       # the end-to-end tests alone
make generate  # regenerate Rust and TypeScript from lean/
make format
```

The settings of the local control plane are in `.env.example`; `nix run` sets the required ones. See `AGENTS.md` for the conventions and the design documents.
