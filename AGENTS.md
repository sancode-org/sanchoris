# AGENTS.md

## Design documents

The product design lives outside this repository, in the private `conao3/idea` repository. Read these before changing code:

- `projects/sanchoris/implementation.md`: build order, components, database, boundaries, tests.
- `projects/sanchoris/README.md`: product definition, terms, data, billing.
- `projects/sanchoris/skill.md`: skills, triggers, sessions, the engine, nesting, member launch.
- `projects/sanchoris/security.md`: threats and countermeasures.
- `projects/design/sanchoris-flow/README.md`: the task screen with the flow panel (design A).
- `projects/design/sanchoris-poc/README.md`: the other screens.

The previous implementation at `sancode-org/sanchoris-old` can be read for reference.

## Conventions

- Documentation, code comments, test names and commit messages are in English. User-facing text is written in English and translated through translation files.
- Stack: follow the shared new-project stack (`notes/new-project-stack.md`, `notes/lang/rust.md`, `notes/lang/typescript.md` in agents-share) and section 1.1 of `projects/sanchoris/implementation.md`.
  - Rust (edition 2024) for the backend. Business logic, state transitions and input constraints are written in Lean, proved, and generated into Rust with leanfran (`conao3/lean-leanfran`). Every generated function called from Rust has at least one useful theorem, audited with leanassay (`#assay`).
  - TypeScript (Node 26, pnpm, vite-plus) for the UI with React 19, react-aria-components, Tailwind CSS v4 and lucide-react. Pure computations in the UI and the workspace agent are written in Lean and generated with the leanfran TypeScript backend, with theorems audited the same way.
  - Nix flake for everything; `make check` is the only verification entry point and runs the Rust checks and `vp run check`.
- Commit directly to `master` after the check command passes.
