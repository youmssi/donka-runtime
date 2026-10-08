# Donka Runtime — Instructions for coding agents

This file is the single entry point for any coding agent working in this repository. Human
contributors read it too. `CLAUDE.md` imports it and adds what is specific to that tool.

## 1. Read before you write

1. Read this file and `CONTRIBUTING.md`. The full engineering guides live in the Studio repo
   (`youmssi/donka`, `docs/engineering/`); the rules there apply here **except where this file
   says otherwise** (see §3).
2. Read the story (`DNK-<n>`, in `youmssi/donka` → `docs/backlog/`) in full.
3. Check the story's dependencies are merged. If one is not, **stop and say so**.
4. Search the code for what already exists before you create anything.
5. For a crate version you are not sure about, read its source in `~/.cargo/registry/src/…`.

## 2. Project overview

- **Product:** Donka is a decision management platform for credit and risk teams. This
  repository is **Donka Runtime**, the data plane: it loads release artifacts published by Donka
  Studio from object storage (S3/MinIO, Azure, GCS, filesystem, zip), hot-reloads them, and
  evaluates decisions over REST for customer systems.
- **Origin:** fork of [gorules/agent-public](https://github.com/gorules/agent-public) (MIT). The
  `upstream` remote points at it. See `DONKA.md` for what Donka changes.
- **API:** upstream paths are kept (`/api/projects/{project}/evaluate/{key}`,
  `/api/rules/{project}/evaluate/{key}`, `/api/rules/{project}` OpenAPI, `/api/health`).
- **Contract with Studio:** the release artifact (`.config/project.json` + decision files) and
  the decision-log feed. Both follow additive-only changes; this repo ships **before** Studio
  when either changes.
- **Stack:** Rust (edition 2024), Axum 0.8, zen-engine (pinned exactly; must equal Studio's pin,
  checked by Studio's `scripts/check-engine-version.sh`), utoipa, wasmtime via `tsgo-wasm`.

## 3. Hard rules

1. **No AI authorship trace, anywhere** (commits, PRs, comments, file headers, docs), and no
   `Co-authored-by` line for a tool.
2. **Conventional Commits** with `Refs: DNK-<n>`. Types: `feat`, `fix`, `refactor`, `test`,
   `docs`, `chore`, `perf`, `build`.
3. **One story, one branch, merged before the next.** `dnk-<n>-<slug>` from `develop`,
   squash-merged into `develop`. Never commit to `main` or `develop`.
4. **Keep the fork thin (ADR-006 in Studio).** Keep upstream's structure, API paths and naming
   inside the code. Put Donka behaviour behind small extension points (a new module, a trait
   implementation, a config flag) rather than editing upstream logic in place. Do not reformat or
   refactor upstream code outside what the story needs.
5. **No hardcoded configuration.** New settings use the existing `config` crate layout
   (`SECTION__KEY` environment variables) and are documented in `docs/` (`configuration.md`,
   or the feature's page).
6. **No secret in artifacts, logs, traces or errors.** Access tokens are compared as hashes
   (from DNK-13).
7. **Additive changes only** to the artifact format and the HTTP API.
8. **No dead code**, no commented-out code, no TODO without a ticket.
9. **Tests match the acceptance criteria**, including token scope, reload during traffic and
   provider failures.
10. **Stop at an `[INTERACTIVE STEP]`** or any product decision: present options, wait.

## 4. How to work

Small verified steps; prove it works, then say so; report honestly; root-cause failures; build
what the story asks and nothing more; ask about product or irreversible decisions; decide
conventional technical ones and say what you chose.

## 5. Checks before every push

```bash
cargo fmt --all --check
cargo clippy --all-targets        # no NEW warnings in code you changed (upstream code has
                                  # existing lints that we do not rewrite, see §3.4)
cargo test                        # tests/it needs Docker (MinIO, Azurite containers)
cargo clippy -p donka-connectors --all-targets -- -D warnings   # mock-only build, as Studio uses it
cargo test -p donka-connectors
```

Offline or proxied builds: `TSGO_WASM_FILE=/path/tsgo.wasm.zst` and
`SWAGGER_UI_DOWNLOAD_URL=file:///path/swagger-ui.zip` avoid the build-time downloads.

## 6. Pull requests

Title = squash commit title. Body follows `.github/pull_request_template.md`. Draft while in
progress; ready when checks are green. Answer every review comment.

## 7. Releases

`develop` → `main` through a promotion PR merged with a merge commit; release-please's release PR
on `main` then sets the version and `CHANGELOG.md`, tags it, and opens a back-merge PR into
`develop` (merge commit). `feat` bumps the minor version, `fix`/`perf` the patch, `!` a major.
The Runtime releases before
Studio whenever the artifact format or the Runtime API changes. After deploy: `GET /api/health`
and one evaluate against a known release.

## 8. Upstream sync

`git fetch upstream` and merge `upstream/main` into a `dnk-<n>-upstream-sync` branch, resolve,
run the checks, open a PR into `develop`. Never rebase shared branches.

## 9. Repository map

```
src/app.rs, src/main.rs     router and startup
src/routes/                 HTTP endpoints (evaluate, rules + OpenAPI, project info, health)
src/provider/               artifact sources: s3, azure_storage, gcs, filesystem, zip
src/data/                   artifact model (.config/project.json)
src/immutable_loader.rs     in-memory release loader
src/tsgo.rs, spec_derive.rs  TypeScript-based type derivation for OpenAPI
crates/connectors/          connector nodes (MIT crate shared with Studio); src/connectors.rs wires it
tests/it/                   integration tests (containers)
docs/                       configuration, connectors, decision log, rules OpenAPI
.github/assets/             README banner
DONKA.md                    what this fork changes and why
```
