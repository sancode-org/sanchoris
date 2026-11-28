# AGENTS.md

## Design documents

The product design lives outside this repository, in the private `conao3/idea` repository. Read these before changing code:

- `projects/sanchoris/README.md`: the core value (predictable skills), terms, data, accounts, billing and plans, screens.
- `projects/sanchoris/skill.md`: predictable skills, nesting, user launch, threads, projects, triggers, the engine.
- `projects/sanchoris/architecture.md`: components and stack, data, boundaries, hosts and the driver, secrets, security and audit, build order, tests.
- `projects/design/sanchoris-flow/README.md`: the thread screen with the flow panel (design A).
- `projects/design/sanchoris-poc/README.md`: the other screens.

The previous implementation at `sancode-org/sanchoris-old` can be read for reference.

## Conventions

- Documentation, code comments, test names and commit messages are in English. User-facing text is written in English and translated through translation files.
- Stack: follow the shared new-project stack (`notes/new-project-stack.md`, `notes/lang/rust.md`, `notes/lang/typescript.md` in agents-share) and the section on components and stack in `projects/sanchoris/architecture.md`.
  - Rust (edition 2024) for the backend. Business logic, state transitions and input constraints are written in Lean, proved, and generated into Rust with leanfran (`conao3/lean-leanfran`). Every generated function called from Rust has at least one useful theorem, audited with leanassay (`#assay`).
  - TypeScript (Node 26, pnpm, vite-plus) for the UI with React 19, react-aria-components, Tailwind CSS v4 and lucide-react. Pure computations in the UI and the workspace agent are written in Lean and generated with the leanfran TypeScript backend, with theorems audited the same way.
  - Nix flake for everything; `make check` is the only verification entry point and runs the Rust checks and `vp run check`.
- Commit directly to `master` after the check command passes.

## Lean, Rust and TypeScript

- Lean owns business logic, state transitions and input constraints: types, specifications, implementations and theorems. The code lives in the Lake package `lean/` (package `sanchoris`, modules `Sanchoris.*`).
- `leanfran.toml` compiles the Lean modules with `leanfran generate` (leanfran 0.14.0). It has one `[[generate]]` table per destination, and each table lists its modules by name. A module goes to the crate or app that the table of what Lean covers in `projects/sanchoris/architecture.md` names: a Rust crate gets `crates/<crate>/src/generated` (module `crate::generated`), a TypeScript app gets `apps/<app>/src/generated`. A module that both languages call is listed in a Rust table and a TypeScript table. The generated code is committed and never edited by hand; `make check` regenerates it and fails on a diff or an untracked file.
- Rust owns input and output (HTTP, DynamoDB, S3, SQS, GitHub, Stripe, Firecracker) and the integration with crates, and calls the generated functions of its own crate.
- TypeScript owns the screens (`apps/frontend`) and the workspace program (`apps/sanchoris-agent`), and calls the generated modules of its own app for pure computations. The Thread channel between `sanchoris-agent` and the control plane is defined by `apps/sanchoris-agent/src/protocol.ts` and `apps/sanchoris-agent/PROTOCOL.md`, with an example of every message in `apps/sanchoris-agent/protocol/`; the control plane tests against the same examples.
- `sanchoris-config` checks the formats of the configuration files and the references to the outside in Rust, and the required keys, the references to States, reachability from `StartAt`, the terminal State and the place of `States.ALL` of a `skill.yaml` with `Sanchoris.SkillDefinition.validate`. The helpers `missingFields` and `misplacedStatesAll` have no theorems and are checked by the tests of `crates/config/tests/validate.rs`; adding them to `#assay` leaves 3 holes each (the empty list, `default` and swapping the operands of `++`).
- `audit/` is a separate Lake package that requires `lean/` by path and leanassay by commit. `audit/Audit/<Module>.lean` has one `#assay` line per generated function that Rust or TypeScript calls. A change that adds such a function adds its theorems and its `#assay` line.

| Function | Called from | Theorems | `#assay` |
|---|---|---|---|
| `Sanchoris.Host.canStartWorkspace` | `sanchoris-core`, `apps/frontend` | `canStartWorkspace_within_limit` (a host that can start one more workspace stays within `maxWorkspaces`), `canStartWorkspace_iff` (the decision as a condition) | 8 mutants, 8 killed, 0 holes |
| `Sanchoris.SkillDefinition.validate`, with `unreachableStates` and `unknownStates` | `sanchoris-config` | `validate_all_reachable` (a definition that passes reaches every State from `StartAt`, by the inductive `Reachable`), `validate_reaches_terminal` (it has a terminal State that `StartAt` reaches), `validate_startAt_defined` and `validate_successors_defined` (`StartAt`, `Next`, the `Next` of `Choices`, `Default` and the `Next` of `Catch` name States), `unreachable_mem_validate`, `unknownState_mem_validate`, `startAt_unknown_mem_validate` and `noTerminal_mem_validate` (each issue as a condition) | 12 mutants, 11 killed, 0 holes, 1 stillborn |
| `Sanchoris.AgentInput.deliver` and `afterTurn` | `apps/sanchoris-agent` | `deliver_steer_iff` (an input goes into the turn only when the turn runs, the delivery is `steer` and the Worker takes steers), `deliver_startTurn_iff` (an input starts a turn exactly when no turn runs), `deliver_queue_iff` (every other input waits for the next turn), `afterTurn_compact_iff`, `afterTurn_turn_iff` and `afterTurn_idle_iff` (after a turn, a waiting compact comes first, then a turn with the waiting inputs, and nothing while the agent stops) | 22 mutants, 20 killed, 0 holes, 2 stillborn |

## Commands

- `nix develop`: the development shell (Rust, Lean and Lake, leanfran, Node 26, pnpm, `vp`, DynamoDB Local).
- `make check`: the only verification entry point. It runs `nix fmt -- --ci`, `lake -d lean build --wfail`, `lake -d audit build --wfail`, `leanfran generate` with the diff and untracked check of the generated code, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `vp run check`. The pre-commit hook and CI run it.
- `make generate`: regenerate Rust and TypeScript from `lean/`.
- `make format`: `nix fmt` (Nix and Rust) and `vp fmt` (TypeScript).
- `nix run`: the control plane and the screens in one process on an in-memory DynamoDB Local, at `SANCHORIS_BIND_ADDR` (default `127.0.0.1:8080`).
- `nix run .#driver`: `sanchoris-driver`.
- A change to `pnpm-lock.yaml` updates the `fetchPnpmDeps` hash in `flake.nix`.
- The type-aware lint of vite-plus runs the static musl build of tsrs: `pnpm-workspace.yaml` installs the musl packages and the `vp` wrapper of the flake sets `TSRS_BINARY`. The `vite` override in `pnpm-workspace.yaml` follows the `vite` dependency of the `vite-plus` version.
