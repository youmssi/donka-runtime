# Contributing to Donka Runtime

The workflow is the same as in Donka Studio (`youmssi/donka`, `CONTRIBUTING.md`), with the
fork-specific rules from `AGENTS.md`.

## Branches

| Branch | Role | Who writes to it |
|---|---|---|
| `main` | What customers run | Promotion PRs (`develop` → `main`, merge commit) and the release PR |
| `develop` | The next release, always green | Squash-merged story PRs only |
| `dnk-<n>-<slug>` | One story | Its author |

Repository settings (GitHub → Settings): squash and merge commits allowed, rebase disabled, head
branches deleted automatically; rulesets on `main` and `develop` requiring a pull request and
green checks, blocking force pushes and deletion; default branch `develop`.

## Story flow

1. `git fetch origin develop && git checkout -b dnk-<n>-<slug> origin/develop`
2. Build the story with its tests and docs; run the checks in `AGENTS.md` §5.
3. Open a draft PR into `develop`; mark it ready when checks are green.
4. Squash-merge, delete the branch, then start the next story.

A story that also changes Studio uses the same branch name in both repos; **this repo merges
first** when the artifact format or the Runtime API changes.

## Commits

`<type>(<scope>): <description>`, a body explaining what and why, and `Refs: DNK-<n>`. No AI
authorship trace.

## Definition of done

- [ ] Acceptance criteria met and tested
- [ ] Checks green
- [ ] `README.md` / `DONKA.md` updated when behaviour or configuration changed
- [ ] Squash-merged into `develop`, branch deleted
