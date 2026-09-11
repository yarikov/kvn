---
name: conventional-commit
description: Create, review, or use Git commit messages that conform to the Conventional Commits 1.0.0 specification. Select the commit type from the actual semantic effect of the diff instead of defaulting to feat or fix. Use when asked to suggest a commit message, validate one, or commit changes with Conventional Commits formatting.
---

# Conventional Commit

Produce a commit message that accurately describes the change and conforms to [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/#specification).

The commit type must describe the primary semantic nature of the change.

**Do not default to `feat` or `fix`.**
Choose the most specific applicable type from the actual diff.

## Inspect the change

Before choosing a message, inspect the relevant diff:

- For staged changes, use `git diff --cached` and verify the staged file set with `git status --short`.
- If nothing is staged and the user only wants a suggestion, inspect `git diff` and clearly say the message describes unstaged changes.
- If the user asks to commit, stage only changes that are clearly in scope. Do not include unrelated user changes.
- If the diff contains multiple independent changes, recommend separate commits when practical.
- Describe only the final state represented by the diff. Do not mention intermediate implementations, failed attempts, temporary states, reverted approaches, or the sequence of work unless that history remains materially relevant to the final change.
- Classify the change from the diff, not from wording in the task, issue title, branch name, or user request. Words such as "fix", "add", "update", or "improve" do not determine the commit type.
- When type or scope conventions are unclear, inspect recent commit subjects with `git log -20 --pretty=format:%s`. Follow established repository conventions when they do not conflict with the semantic meaning of the change.

## Choose the type

Before writing the commit description, classify the change into one primary type.

Use the following meanings:

| Type       | Use when                                                                                                                                                      |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `feat`     | The change introduces a new externally observable capability or product behavior.                                                                             |
| `fix`      | The change corrects existing product behavior that was wrong, broken, or inconsistent with its intended contract.                                             |
| `docs`     | The change affects documentation only.                                                                                                                        |
| `test`     | The change adds, removes, restructures, or corrects tests without changing production behavior.                                                               |
| `refactor` | Production code is restructured without intentionally changing externally observable behavior.                                                                |
| `perf`     | The primary purpose is improving runtime performance, memory usage, latency, throughput, or another performance characteristic without introducing a feature. |
| `style`    | The change only affects formatting, whitespace, code style, or equivalent non-semantic representation.                                                        |
| `build`    | The change affects the build system, packaging, dependency definitions, compiler/toolchain configuration, or external dependencies.                           |
| `ci`       | The change affects CI/CD configuration, workflows, or CI-specific scripts.                                                                                    |
| `chore`    | Maintenance or repository housekeeping that does not fit a more specific type and does not change product behavior.                                           |
| `revert`   | The change intentionally reverts an earlier commit or change.                                                                                                 |

Additional repository-specific types may be used only when they are clearly established by project history or explicitly requested by the user.

### Selection rules

Prefer a specialized type over `feat` or `fix` when the change is exclusively in that category.

Examples:

- correcting a README typo → `docs`, not `fix`
- adding missing unit tests → `test`, not `feat`
- correcting a broken test expectation → `test`, not `fix`
- updating a GitHub Actions workflow → `ci`, not `fix`
- changing compiler or bundler configuration → `build`, not `feat`
- updating dependencies as routine maintenance → `build` or the repository's established dependency type, not `feat`
- reorganizing implementation without changing behavior → `refactor`, not `feat`
- formatting or lint-only cleanup → `style`, not `fix`
- making an existing algorithm faster without changing its contract → `perf`, not `feat`

For production behavior changes:

1. If the change adds a capability that did not previously exist, use `feat`.
2. If the capability already existed but behaved incorrectly and the change makes it behave as intended, use `fix`.
3. If externally observable behavior is intentionally unchanged, use `refactor`.
4. If behavior is unchanged and the primary purpose is performance, use `perf`.

Do not use `fix` merely because something was "wrong" in documentation, tests, formatting, CI, build configuration, or housekeeping. Use the corresponding specialized type unless production behavior itself is being fixed.

Do not use `feat` merely because new code, files, tests, documentation, configuration, or dependencies were added. `feat` requires a new product capability or externally observable behavior.

### Mixed changes

When a commit contains supporting changes, classify it by its primary semantic change:

- production feature + its tests/docs → `feat`
- production bug fix + regression test → `fix`
- refactor + tests proving unchanged behavior → `refactor`
- performance optimization + benchmarks/tests → `perf`

Generated files should normally follow the type of the source change that caused them.

If a diff contains multiple independent semantic changes that would require different primary types, prefer separate commits rather than forcing them under one generic type.

## Format

Use this structure:

```text
<type>[optional scope][optional !]: <description>

[optional body]

[optional footer(s)]
```

Apply these rules:

- Start with a type, optionally followed by a noun scope in parentheses, optionally followed by `!`, then `: `.
- Write a short description immediately after the prefix.
- Prefer a concise, lowercase, imperative summary.
- Do not end the description with a period.
- Describe what the commit changes, not the work process used to produce it.
- Separate an optional free-form body from the description with one blank line.
- Use the body for important context or motivation that is not clear from the summary.
- Put optional footers after one blank line.
- Format each footer as `Token: value` or `Token #value`; replace spaces in tokens with hyphens.
- Mark a breaking change with `!` immediately before the colon, a `BREAKING CHANGE: <description>` footer, or both.
- `BREAKING-CHANGE` is an equivalent footer token.
- A breaking change may use any commit type.

Do not invent a scope, issue reference, breaking change, or motivation that is not supported by the diff or user context.

## Scope

Use a scope only when it adds useful information.

- Prefer an established repository scope when one exists.
- Use the affected package, module, subsystem, or other recognizable project area.
- Omit the scope when the change spans unrelated areas or no clear scope exists.
- Do not invent generic scopes such as `app`, `code`, `misc`, or `changes` merely to fill the field.

## Validate the result

Before returning or committing the message, verify:

- the type matches the semantic effect of the diff;
- `feat` is used only for an actual new capability;
- `fix` is used only for an actual correction to product behavior;
- a more specific type such as `docs`, `test`, `refactor`, `perf`, `style`, `build`, or `ci` does not describe the change better;
- the scope, if present, is supported by the repository or diff;
- the description accurately summarizes the final change.

## Deliver or commit

- If asked only for a message, return the final message in a plain text code block, without XML or placeholder syntax.
- If asked to validate a message, distinguish Conventional Commits specification violations from optional repository/style improvements.
- If asked to commit, show or state the selected message and run `git commit` only after the requested changes are staged.
- Preserve multiline bodies and footers as separate paragraphs.
- Do not push, amend, rebase, or otherwise rewrite history unless explicitly requested.
