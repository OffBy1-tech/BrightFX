# Docs

Design records for BrightFX: the spec behind a change and the plan it was
built from. They describe the work as it was decided; once it ships, the
code and the READMEs are the reference for how things work now
(`core/README.md`, `core/brightfx-core/README.md`, the package READMEs).

## Conventions

- Specs live in `specs/`, plans in `plans/`, named `YYYY-MM-DD-<topic>.md`.
- Work in progress is written under `docs/superpowers/`, which is
  gitignored and never committed. When the work ships, its finished spec
  and plan move here.
- Before a doc moves here, take out anything local to one machine: absolute
  paths, worktree names, private repositories.
- A shipped doc is not rewritten to track the code. If review changes a
  decision, note it in the spec and add a status line to the plan.

## Index

| Doc | Kind | State |
| --- | --- | --- |
| [Cull particles outside the viewport](specs/2026-10-02-cull-particles-outside-viewport-design.md) | spec | shipped |
| [Cull particles outside the viewport](plans/2026-10-02-cull-particles-outside-viewport.md) | plan | shipped |
| [Cull entry exemption](specs/2026-10-02-cull-entry-exemption-design.md) | spec | shipped |
| [Preset cull margin](specs/2026-10-04-preset-cull-margin-design.md) | spec | shipped |
| [Emitter track pre-roll](specs/2026-10-08-emitter-track-preroll-design.md) | spec | shipped with PR #36 |
| [Emitter track pre-roll](plans/2026-10-09-emitter-track-preroll.md) | plan | shipped with PR #36 |
| [Sub-stepping advance](specs/2026-10-17-advance-substep-design.md) | spec | shipped with PR #50 |
| [Sub-stepping advance](plans/2026-10-17-advance-substep.md) | plan | shipped with PR #50 |
| [Phase 0 release workflow](plans/2026-10-21-phase-0-release-workflow.md) | plan | shipped with PR #55 (`v0.4.0`) |
