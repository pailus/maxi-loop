# ENGINEERING.md — Maintainable Code Rules

> Read this before creating new files/modules, changing structure, touching
> the database schema or a public API, or adding tests. Goal: code the next
> person (or the next AI session) can understand and change safely.

## 1. Consistency First

- **Follow the existing patterns** of this project (structure, naming, error
  handling, test style) before introducing a new one. Check
  `.harness/ARCHITECTURE.md` and neighbouring files first.
- Never mix two styles for the same thing (two HTTP clients, two date
  libraries, two ways to load config). If the existing way is really bad,
  propose the change to the user instead of adding a second way.
- Respect the project's formatter and linter config; do not reformat files you
  are not otherwise changing.

## 2. Structure

- **One responsibility per file/module.** Group code by feature/domain, not by
  "utils"/"helpers" dumping grounds.
- Size signals (not hard limits): a file over ~400 lines or a function over
  ~50 lines, or nesting deeper than 3 levels, is a hint to split — when you
  are already changing it for the task.
- **Dependency direction**: core/business logic does not import HTTP handlers,
  UI or DB drivers directly; it gets them through parameters/interfaces.
  Handlers stay thin: parse → call logic → respond.
- Configuration comes from env/config files in one place, not scattered
  `getenv` calls or hard-coded URLs, ports and paths.
- Names say what things are (`invoiceTotal`, not `data2`); booleans read as
  questions (`isActive`, `hasAccess`).

## 3. Keep It Simple

- **No speculative code**: no features, options, abstractions or extension
  points "for later". Build what the current feature needs.
- **Rule of three**: duplicate once if needed; extract a shared abstraction
  when the third copy appears.
- No dead code, no commented-out code, no unused parameters/imports left
  behind. Git keeps history.
- Prefer boring, explicit code over clever code.

## 4. Errors

- Never swallow errors silently (`catch {}`, `_ = err`, bare `except:`).
  Handle it, return it with context, or log it — on purpose.
- Add context when passing errors up ("load invoice 42: connection refused").
- Fail fast on invalid state at the boundaries (startup config, request input).
- Every network/IO call has a timeout; retries are bounded with backoff.

## 5. Tests

- **Every bug fix starts with a test that reproduces the bug** (fails before,
  passes after).
- Test behaviour through the public interface, not private implementation
  details — refactors should not break tests.
- Unit tests are deterministic: no real network, no real clock/randomness
  without control, no order dependence.
- Each test checks one behaviour and its name says which one. Include at least
  one failure/edge case, not only the happy path.

## 6. Database & Data

- **Never edit a migration that has already run** anywhere shared; add a new one.
- Schema changes stay backward compatible with the running code (add column →
  deploy code → remove old column later), so deploys and rollbacks are safe.
- Watch for N+1 queries; list endpoints are paginated/limited; add an index
  for new frequent filters.
- Multi-step writes that must succeed together use a transaction.

## 7. APIs & Contracts

- Do not rename/remove fields, endpoints, events or CLI flags that clients use
  without a compatibility period or a version bump — tell the user first.
- New fields are optional/additive. Document request/response changes where
  the project documents its API.

## 8. Dependencies

- Prefer the standard library or what the project already uses. A new
  dependency needs a stated reason (see `.harness/SECURITY.md` section 8).
- Pin versions through the lockfile; no unrelated upgrades in a feature commit.

## 9. Documentation

- Comments explain **why** (a constraint, a workaround, a non-obvious
  decision), not what the next line does.
- Update README / API docs / `.harness/ARCHITECTURE.md` **in the same commit**
  as the change that makes them outdated.
- Record decisions with lasting impact in `.harness/decisions.md`.

## 10. Stack Notes

**Rust:**
- No `unwrap()` / `expect()` on paths that can fail in production; return
  `Result` with context. Keep them for tests and proven invariants.
- `cargo fmt` + `cargo clippy` clean; no new `#[allow]` without a comment why.
- No `unsafe` without a `// SAFETY:` comment and the user's OK.
- Do not hold a lock or a blocking call across `.await`.
