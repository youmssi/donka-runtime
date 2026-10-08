# Rules OpenAPI

`GET /api/rules/{project}` returns an OpenAPI 3 document describing the
deployed release's evaluable rules — one `POST /evaluate/{path}` operation per
graph and policy, tagged by kind.

Request and response schemas come from the rule itself when it declares them on
its input/output nodes. Anything undeclared is derived by analysing the release
as a single workspace: property references become a nested JSON Schema, and the
return types of function nodes are resolved by type-checking their TypeScript.

The document is built on the first request for a project and reused until the
release changes, so only that first request pays for the analysis.

## Type resolution

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

## Build caching

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

## Building offline

`build.rs` fetches the tsgo wasm module from the `tsgo-wasm` crate's GitHub
release and verifies its checksum. For air-gapped builds, point it at a local
copy instead:

```bash
TSGO_WASM_FILE=/path/to/tsgo.wasm.zst cargo build --release
```
