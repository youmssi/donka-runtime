# Donka Runtime

The decision-serving service of [Donka](https://github.com/youmssi/donka). It loads published
release artifacts from object storage, hot-reloads them, and evaluates decisions over REST.

This repository is a fork of [gorules/agent-public](https://github.com/gorules/agent-public)
(MIT). The upstream history is preserved; the `upstream` remote points at it and is merged
regularly. The original license and copyright notice in `LICENSE` stay as they are.

## Planned Donka changes (see donka/docs/ROADMAP.md, epic D)

| Item | Change |
|---|---|
| D1 | Rebrand; upgrade zen-engine to 2.0.1 (done, DNK-12; same pin as Studio, enforced by Studio's drift check) |
| D2 | Hashed, per-environment access tokens in `.config/project.json` (done, DNK-13: `src/data/access.rs`) |
| D3 | Connector handler (custom-node adapter) with secrets from env or files, timeouts, retries, circuit breaker (done, DNK-17: `crates/connectors`, wired in `src/connectors.rs`) |
| D4 | Decision-log emitter: ships input, output, release and trace to Studio asynchronously (done, DNK-18: `src/decision_log.rs`; graceful stop in `src/main.rs`) |
| D5 | Per-token rate limits on the evaluate and rules routes, `429` with `Retry-After` (done, DNK-22: `src/rate_limit.rs`) |
| D6 | Input contracts: a request breaking the decision's input schema gets `400` naming the field (done, DNK-37: `src/contract.rs`, `docs/contracts.md`) |
| D7 | zen-engine brought to 2.1.6, Runtime first, accepted on replay and scenario evidence (done, DNK-62: `tests/self_reference.rs`, null inputs in `tests/contract.rs`) |
| D8 | Connectors authenticate with OAuth2 client credentials and mutual TLS (done, DNK-63: `crates/connectors/src/live.rs`) |
| D9 | Every secret setting also accepted as a file (`_FILE`) (done, DNK-65: `src/secret_files.rs`) |
| D10 | Published signed images (done, DNK-64: `.github/workflows/images.yml`); the `Zip` provider with `PROVIDER__WATCH` serves and hot-reloads the disk volume Studio publishes to (done, DNK-64: `src/provider/zip.rs`) |
| D11 | Load test with published latency and throughput targets, checked in CI (planned, DNK-67) |
| D12 | Image scanning, SLSA provenance, OpenSSF Scorecard, threat model and security contact (planned, DNK-49, DNK-69, DNK-73) |
| D13 | Batch evaluation of many inputs against one release (Phase 2 candidate, DNK-70) |
| D14 | Releases that take effect at a scheduled time, from the artifact already held (Phase 2 candidate, DNK-78) |

Changes are kept behind small extension points so upstream merges stay easy. The Runtime never
reads Studio's licence: released rules keep being served whatever Studio's state (DNK-77). Where
each capability stands across Donka: Studio's `docs/capability-map.md`.

## Names we keep on purpose

- The Rust library is still called `agent` (only the package and binary are `donka-runtime`),
  so every source file and test stays identical to upstream.
- `x-gorules` in the generated OpenAPI documents and `application/vnd.gorules.decision` in
  decision files are contracts read by API clients and written by the editor. Renaming them
  would break compatibility; a Donka alias can be added later as an additive change.
