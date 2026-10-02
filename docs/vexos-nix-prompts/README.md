# vexos-nix companion prompts

**Rule:** the vexnas repo never changes code in other repos. Every vexos-nix change needed by vexnas
is delivered as a prompt in this directory. You paste one into a Claude session opened in
`~/Projects/vexos-nix`; that session does the work under vexos-nix's own `CLAUDE.md` workflow
(research → spec → implement → review → preflight → commit message for you to run).

Each prompt is self-contained: it restates what it needs from `docs/spec.md` so the vexos-nix
session does not need access to this repo. `docs/spec.md` stays the design source of truth; if a
prompt and the spec disagree, fix the prompt.

## Order and dependencies

| Prompt | Spec ref | Needed by vexnas phase | Depends on |
|---|---|---|---|
| [c4-vexnas-input-and-module.md](c4-vexnas-input-and-module.md) | C4 | 1 (dashboard) | vexnas repo Phase 0 pushed to GitHub (flake outputs exist) |
| [c3-nas-nix-import.md](c3-nas-nix-import.md) | C3 | 2 (apply engine) | none |
| [c1-declarative-shares.md](c1-declarative-shares.md) | C1 (+C7 option stubs) | 3 (shares) | C3 |
| [c7-nas-users-groups.md](c7-nas-users-groups.md) | C7 | 4 (users/jobs) | C1 (same module file) |
| [c6-pool-scripts-noninteractive.md](c6-pool-scripts-noninteractive.md) | C6 | 5 (pool wizard) | none |
| [later-optional.md](later-optional.md) | C1.4, C2, C5, C8 | 6 (hardening) | C1, real-host proof |

## Per-prompt hand-off checklist (for you)

1. Open Claude in `~/Projects/vexos-nix`, paste the prompt.
2. Let it run its normal phases; it must not run the commands your `CLAUDE.md` forbids and must not
   `git add/commit/push`. Run the commit message it hands you.
3. For C4: run `nix flake lock --update-input vexnas` yourself, then rebuild on a real host yourself.
4. Tell the vexnas session when the change has landed so the matching phase can start.
