# Contributing to kvn

Thank you for contributing. These guidelines keep changes reviewable and make
the generated changelog useful to users.

## Before You Start

- Open an issue before beginning a large feature or architectural change.
- Base your work on the latest `develop` branch. Pull requests target
  `develop`; `master` receives `develop` when a release is made. CI runs on
  every pull request, on pushes to `master`, and on release tags.
- Keep each pull request focused on one logical change. Unrelated refactors,
  parsers, dependency changes, and behavior changes belong in separate pull
  requests.
- Do not commit subscription URLs, access tokens, credentials, real server
  addresses, device identifiers, or other private data. Use clearly fictional
  values in tests and examples.

## Branch Names

Use a lowercase `<type>/<short-description>` name. Recommended types match
Conventional Commits:

```text
feat/subscription-headers
fix/subscription-retry
docs/contributing-guidelines
refactor/config-parser
test/subscription-import
```

Avoid vague names such as `changes`, `updates`, or `add-stuff`.

## Commits

Write commit messages in English and follow
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```text
type(scope): imperative description
```

Examples:

```text
fix(subscription): send headers only to their configured source
feat(ui): add subscription settings overlay
test(subscription): cover HTTP retry behavior
docs: document custom DNS servers
```

Use one of `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
`ci`, `chore`, or `revert`. Keep the summary specific and imperative: use
`add`, `fix`, or `prevent`, not `added`, `fixed`, or `misc changes`.

## Pull Requests

Pull request titles and descriptions must be written in English. The title
must use the same Conventional Commits format as a commit message:

```text
fix(subscription): support per-source request headers
```

`git-cliff` uses commit subjects to generate release notes. Since pull requests
are squash-merged, their titles become those subjects. Write the title as a
concise, user-facing changelog entry; `update code` would produce an unhelpful
public release note.

Release notes remove the Conventional Commit prefix and group pull requests as
follows: `feat` becomes **New Features**; `fix` becomes **Bug Fixes**;
`perf`, `refactor`, `docs`, and dependency chores become **Improvements**; and
the remaining types become **Other**. `chore(release)` version bumps are omitted.

The description must explain:

- the user-visible problem or motivation;
- what the pull request changes;
- important design and security decisions;
- related issues, when applicable.

Only the **Summary** and **Changes** sections of the template are required.
The **Testing** section and the contribution checklist are optional for every
pull request; leaving them out does not waive the checks in
[Tests and Quality Checks](#tests-and-quality-checks).

Update the pull request when its scope changes. Reviewers should not have to
infer important behavior from the diff.

## Code and Architecture

- Follow the architecture and project conventions in `AGENTS.md` and in the
  subsystem guide (`src/*/AGENTS.md`) for the code you change.
- Keep `app::update::update` free of I/O, threads, and system calls. Declare
  side effects as `Effect` values and execute them in the daemon.
- Use atomic writes for configuration and persistent state.
- Use `tracing` for logging in the daemon and the TUI; do not use `println!`
  there. One-shot CLI commands print their output with `println!` /
  `eprintln!`.
- Support Arch Linux unless an issue explicitly expands the platform scope.
- Use the existing `src/foo.rs` plus `src/foo/bar.rs` module layout; do not add
  `mod.rs` files.

## Tests and Quality Checks

Every behavior change and bug fix must include tests that fail without the
change and pass with it. Exercise error paths and boundary cases as well as the
happy path. Network-dependent logic should use a local test server or be split
so that its decisions can be tested as pure functions; tests must not depend on
an external subscription provider.

Total region and line coverage must remain at or above 85%. Before opening a
pull request, run:

```bash
cargo fmt --check \
  && cargo clippy --all-targets --all-features -- -D warnings \
  && cargo test --locked \
  && cargo insta test --check --unreferenced=reject \
  && cargo llvm-cov --locked --summary-only \
  && cargo deny check all
```

`cargo insta test` fails on snapshots that changed or that no test references
any more; `cargo deny` checks dependency licenses, advisories, and sources
against `deny.toml`. They require `cargo-insta`, `cargo-llvm-cov`, and
`cargo-deny`. CI runs the same gates.

### Documentation-Only Pull Requests

A pull request is documentation-only when every changed file is a Markdown
file (`*.md`, including `AGENTS.md`, `.agents/skills/` and
`.github/pull_request_template.md`) or an image under `assets/`. Any other
file — source, tests, snapshots, scripts, `contrib/`, `pkg/`, workflows —
makes it a regular pull request with every gate above.

A documentation-only pull request may skip the Rust gates; it always requires:

- `git diff --check origin/develop...HEAD` passes;
- every relative link and `#anchor` added or changed in the diff resolves
  (`lychee --offline --include-fragments <files>` when available);
- every claim it adds or changes about behavior, commands, paths, or
  configuration was checked against the current source.

## Review Checklist

- The branch starts from the current `develop` branch.
- The pull request contains one logical change.
- The title follows Conventional Commits and is suitable for release notes.
- The title and description are in English.
- Tests cover all new behavior and regressions.
- Formatting, Clippy, tests, and coverage pass without warnings, or the pull
  request is documentation-only and passes its own checks.
- No secrets or real subscription data are included.
- User-facing documentation is updated when behavior or configuration changes.
