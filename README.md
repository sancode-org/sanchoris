# sanchoris

A hosted service that runs predictable skills inside AI coding agent threads. A skill shows its flow, its limits and its track record before it runs; the engine runs the fixed steps and hands the agent only the steps that need it. The agent runs on your own Claude and Codex subscriptions.

This repository is being built from scratch. The previous implementation is kept at [sancode-org/sanchoris-old](https://github.com/sancode-org/sanchoris-old) as a reference.

## Run

```sh
nix run                                     # control plane and screens on http://127.0.0.1:8080
curl http://127.0.0.1:8080/health           # ok
nix run github:sancode-org/sanchoris#driver # sanchoris-driver
```

## Layout

| Path | Contents |
|---|---|
| `crates/api` | HTTP API and the `sanchoris` server (axum) |
| `crates/graphql` | GraphQL schema of the screens |
| `crates/config` | Reading and validating the configuration repository |
| `crates/engine` | The `skill.yaml` interpreter |
| `crates/core` | Domain types, storage, triggers and thread progress |
| `crates/worker` | Background jobs |
| `crates/driver` | `sanchoris-driver`, which runs workspaces on a user's machine |
| `apps/frontend` | The screens (React 19, react-aria-components, Tailwind CSS v4, lucide-react, Apollo Client, TanStack Router) |
| `apps/sanchoris-agent` | `sanchoris-agent`, the program inside a workspace |
| `infra/tables.json` | The DynamoDB tables: `main` (keys `pk` and `sk`, indexes `gsi1` and `gsi2`) and the append-only `audit` |
| `lean` | Lean definitions and theorems, compiled to Rust and TypeScript by leanfran (`leanfran.toml`) |
| `audit` | leanassay audits of the theorems |

## Develop

```sh
nix develop
make check     # format, Lean proofs, audits, generated code, clippy, cargo test, vp run check
make generate  # regenerate Rust and TypeScript from lean/
make format
```

See `AGENTS.md` for the conventions and the design documents.
