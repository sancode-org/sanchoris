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
- Stack: Rust for the backend (engine, API, workers) and TypeScript with React for the UI, following the shared new-project stack (Nix flake, one check command, CI on GitHub Actions).
- Commit directly to `master` after the check command passes.
