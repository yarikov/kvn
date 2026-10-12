---
name: create-pull-request
description: Create a GitHub pull request for kvn-tui after validating the branch, diff, and repository rules. Use when asked to open, create, or publish a PR; do not use for merely drafting PR text or reviewing an existing PR.
---

# Create Pull Request

Open a reviewable GitHub pull request from the current branch into `develop`. Default to a draft PR unless the user explicitly asks for a ready-for-review PR.

## Read the repository rules

Before deriving the PR title or description, read:

- `AGENTS.md` for architecture and project constraints, and the nested `src/*/AGENTS.md` guide of every subsystem the diff touches;
- `CONTRIBUTING.md` for branch, commit, and PR requirements;
- `.github/pull_request_template.md` for the required description structure;
- `.agents/skills/conventional-commit/SKILL.md` for title formatting.

Repository files are the source of truth if these instructions become stale.

## Validate the branch

Perform read-only local checks before contacting GitHub:

1. Inspect `git status --short --branch`, the current branch, configured remotes, and its upstream.
2. Stop if HEAD is detached or the current branch is `develop` or `master`.
3. Stop if tracked or untracked changes are present. List the files and ask the user to commit, discard, or otherwise handle them separately. Do not stage or commit them as part of this skill.
4. Confirm that `origin` points to the intended GitHub repository and that the branch contains at least one change intended for the PR.

Then verify the remote state:

1. Run `gh auth status`. If authentication is invalid, stop and ask the user to run `gh auth login -h github.com`; do not start an interactive login on their behalf.
2. Fetch the current `develop` reference from `origin`.
3. Require `origin/develop` to be an ancestor of HEAD. If it is not, report that the branch must be updated and stop. Do not merge, rebase, reset, or force-push automatically.
4. Check GitHub for an existing open, draft, closed, or merged PR with the same head branch. Return an existing open or draft PR instead of creating a duplicate. For a closed or merged match, stop and ask how the user wants to proceed.

## Inspect the proposed change

Review `origin/develop...HEAD`, including the changed file list, full diff, and commit list.

- Confirm the PR represents one logical change and that its commits and branch name follow `CONTRIBUTING.md`.
- Run `git diff --check origin/develop...HEAD`.
- Look for secrets, tokens, credentials, real subscription URLs, server addresses, and device identifiers in added content. Stop and identify the affected files if anything suspicious is found; never reproduce a secret in the response.
- Derive all claims from the actual diff and available user context. Do not invent motivation, issue references, tests, security properties, or user-visible behavior.

## Check documentation changes

Do not run the Rust quality gates (`cargo fmt`, `clippy`, `test`, `insta`, `llvm-cov`, `deny`); CI runs them on every pull request.

When the diff changes Markdown files, require that every relative link and anchor added or changed in the diff resolves (use `lychee --offline --include-fragments` on the changed files when it is installed, otherwise check each one by hand), and that each changed claim about behavior, commands, paths, or configuration matches the current source. CI does not cover these checks.

Do not edit files to fix a failure within this workflow. Report it with a concise diagnosis so the user can address it separately.

## Prepare the PR

Create an English title and description.

- Format the title as `type(scope): imperative description` according to the conventional-commit skill and make it suitable for generated release notes.
- Fill the template's Summary and Changes sections with the user-visible motivation, complete scope, and important design or security decisions.
- Include related issues only when the user or repository history provides a real issue reference.
- Recheck that title and body describe the complete diff rather than only the latest commit.

Show the user the base branch, head branch, draft/ready status, proposed title, and complete body. Obtain explicit confirmation immediately before pushing or calling the GitHub API. Treat edits requested at this point as changes to the proposal and reconfirm the final version.

## Publish

After confirmation:

1. Push the current branch normally and set its upstream when needed. Never force-push.
2. Store the approved body in a temporary file outside the worktree and pass it to `gh pr create` with `--base develop`, the explicit head branch, title, and body file.
3. Pass `--draft` unless the user explicitly requested a ready-for-review PR.
4. Query the created PR and report its number, URL, base/head branches, and draft status.

If the push succeeds but PR creation fails, preserve and report that distinction. Do not push again unnecessarily; diagnose the `gh pr create` failure and request only the action needed to resume. Never create, edit, close, reopen, or mark a PR ready beyond what the user confirmed.
