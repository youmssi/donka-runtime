# Donka Runtime

Part of [Donka](https://github.com/youmssi/donka). Fork of [gorules/agent-public](https://github.com/gorules/agent-public) (MIT); see [DONKA.md](DONKA.md) for what Donka changes.

Donka Runtime is an open-source, standalone microservice that acts as a high-performance Rules Engine over REST, without requiring a UI. It is designed to pull Releases from Object Storage, automatically re-load them at runtime when changes occur, and evaluate decision models efficiently. This ensures that your rules are always up-to-date and accessible with minimal configuration.

## Environment Variables

### AWS

In case your deployment supports IAM, most of the environment variables below are __optional__.

```bash
PROVIDER__TYPE=S3
PROVIDER__BUCKET=bucket
PROVIDER__REGION=us-east-1 # Optional in case of IAM
AWS_ACCESS_KEY_ID=<aws-access-key-id> # Optional in case of IAM
AWS_SECRET_ACCESS_KEY=<aws-secret-access-key> # Optional in case of IAM
```

### Azure

```bash
PROVIDER__TYPE=AzureStorage
PROVIDER__CONNECTION_STRING=<connection-string>
PROVIDER__CONTAINER=<container-name>
```

### Google Cloud

```bash
PROVIDER__TYPE=GCS
PROVIDER__BUCKET=<bucket-name>
PROVIDER__BASE64_CONTENTS=<base64-credential-contents>
```

### FileSystem
```bash
PROVIDER__TYPE=Filesystem
```

### FileSystem Zip
For FileSystem type, all project zips should be in ./data folder.
You can build your own image bundled with rules by doing docker build from our image and adding layer that adds ./data folder
```bash
PROVIDER__TYPE=Zip
```

### MinIO
```bash
PROVIDER__TYPE=S3
PROVIDER__REGION=us-east-1
PROVIDER__BUCKET=bucket
PROVIDER__FORCE_PATH_STYLE=true
PROVIDER__ENDPOINT=http://localhost:9000
PROVIDER__PREFIX=folder/
AWS_ACCESS_KEY_ID=
AWS_SECRET_ACCESS_KEY=
```
## Access tokens

Evaluate and rules requests send the token in the `X-Access-Token` header. A release artifact
lists the tokens it accepts in `.config/project.json`:

- `accessTokenHashes` (format version 2, written by Donka Studio): the lowercase hex SHA-256 of
  each token, with the environment it was issued for. Tokens never appear in the artifact. An
  artifact deployed to an environment (`environment.key`) only accepts that environment's
  tokens, so a staging token is refused by production.
- `accessTokens` (version 1): plain tokens, still accepted so older artifacts keep working.

The artifact format is documented in Studio: `youmssi/donka`, `docs/artifact-format.md`.

## Connectors

A decision can call an outside service (a credit bureau, a KYC or AML provider) through a
connector node: a custom node of kind `donka.connector`, authored in Donka Studio. The Runtime
POSTs the node's JSON body (string values may be templates over the node's input, such as
`{{ applicant.nationalId }}`) and adds the JSON response to the node's output under `outputKey`.
When the call still fails after its retries, the node either fails the evaluation or continues
with the `fallback` the author defined (`onError`).

Secrets never travel in the artifact: a node names its secret (`BUREAU_KEY`) and the Runtime
reads the value from `DONKA_SECRET_<NAME>`. A missing secret fails the call and names the
secret, never a value. Values never appear in responses, traces or logs.

```bash
DONKA_SECRET_BUREAU_KEY=...         # one variable per secret a release uses
CONNECTORS__TIMEOUT=3000            # milliseconds per attempt when the node does not say
CONNECTORS__MAX_TIMEOUT=10000       # the most a node may ask for
CONNECTORS__RETRIES=1               # attempts after the first one when the node does not say
CONNECTORS__MAX_RETRIES=3           # the most a node may ask for
CONNECTORS__BREAKER_FAILURES=5      # consecutive failed calls to one URL that pause calls to it
CONNECTORS__BREAKER_COOLDOWN=30000  # milliseconds before one call is tried again
```

Server errors (5xx, 429, 408), timeouts and unreachable services are retried with a short
backoff (100 ms, doubling, at most 1 s); other refusals and non-JSON answers are not. The node's
trace (`trace: true`) shows the outcome (`ok`, `fallback`, `error`), the attempts, the status and
the error code. Other custom node kinds are not evaluated, as upstream.

The handler is the `donka-connectors` crate (`crates/connectors`, MIT). Studio uses the same
crate without its `live` feature to simulate connector nodes with their mock responses.

## Rules OpenAPI

`GET /api/rules/{project}` returns an OpenAPI 3 document describing the
deployed release's evaluable rules — one `POST /evaluate/{path}` operation per
graph and policy, tagged by kind.

Request and response schemas come from the rule itself when it declares them on
its input/output nodes. Anything undeclared is derived by analysing the release
as a single workspace: property references become a nested JSON Schema, and the
return types of function nodes are resolved by type-checking their TypeScript.

The document is built on the first request for a project and reused until the
release changes, so only that first request pays for the analysis.

### Type resolution

Function-node types are resolved by [tsgo](https://github.com/microsoft/typescript-go),
the TypeScript compiler compiled to WebAssembly and run under wasmtime. It is
precompiled at build time (`build.rs`) and embedded in the binary, so startup
deserializes an artifact rather than compiling one.

Running the compiler in a wasm sandbox is what makes it safe to type-check
untrusted rule content in-process: a guest that panics, traps, runs away or
exhausts memory is contained by wasmtime and surfaces as an unresolved type,
never as a crashed agent. Every failure degrades to publishing the declared
schema instead of the derived one.

```bash
TSGO__ENABLED=true         # set false to skip type resolution entirely
TSGO__MEMORY_BYTES=2147483648
TSGO__TIMEOUT=30000        # milliseconds
TSGO__CACHE_CAPACITY=4096  # resolved function types held per process
```

### Build caching

Precompiling the module costs ~26 CPU-seconds. Cargo caches it locally, so only
a change to `build.rs` itself rebuilds it — but CI and Docker start cold. Point
`TSGO_CWASM_CACHE` at a directory to keep the artifact across builds:

```bash
TSGO_CWASM_CACHE=.tsgo-cache cargo build --release
```

`build.rs` revalidates whatever it finds there, checking both that the artifact
came from the same tsgo revision and target and that it still loads. A stale or
corrupt entry is rebuilt rather than used, so the cache cannot ship a module
that fails at runtime.

CI restores this directory with `actions/cache`, and the Dockerfile keeps
dependencies and the module in a stage that only the manifests and `build.rs`
invalidate.

### Building offline

`build.rs` fetches the tsgo wasm module from the `tsgo-wasm` crate's GitHub
release and verifies its checksum. For air-gapped builds, point it at a local
copy instead:

```bash
TSGO_WASM_FILE=/path/to/tsgo.wasm.zst cargo build --release
```
