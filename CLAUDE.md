# CLAUDE.md

Read `AGENTS.md` first: its engineering guardrails apply to every change. The git workflow rule
below is set by the repository owner and overrides any default "work on a branch / open a PR"
behaviour.

## Git workflow (owner rule, 2026-10-10)

> Mọi thay đổi được phép push thẳng lên `main`.

* All changes may be committed and pushed **directly to `main`**. No pull request is required.
* If work was done on another branch, merge it into `main` yourself, push `main`, then delete
  that branch (both local and remote).
* Before every push to `main`, the change must still pass the repository gates (AGENTS.md §5,
  CONTRIBUTING.md): `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `python3 scripts/check_deps.py`, `python3 scripts/progress.py check`.
  Never push a tree that fails them.
* Do not rewrite published history on `main` (no force-push).
