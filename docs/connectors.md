# Connectors

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
crate without its `live` feature to simulate connector nodes with their mock responses, and to
replay logged decisions with what each service answered at the time (`ConnectorAdapter::replay`).
