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

A secret can also be a file: `DONKA_SECRET_<NAME>_FILE` names a file whose content is the value
(a mounted Docker or Kubernetes secret, a PEM certificate), trailing newlines removed. Setting both
forms, or a file the Runtime cannot read, stops the Runtime at start
([secrets as files](configuration.md#secrets-as-files)).

## Authentication

A node's `auth` says how the call is authenticated; its optional `tls` adds a client certificate.
Both hold secret **names** only.

| `auth.type` | What the Runtime sends |
|---|---|
| `none` | nothing |
| `bearer` | `Authorization: Bearer <secret>` |
| `header` | `<header>: <secret>`, e.g. `X-Api-Key` |
| `oauth2` | `Authorization: Bearer <token>`, the token from an OAuth2 token endpoint |

### OAuth2 client credentials

```json
"auth": {
  "type": "oauth2",
  "tokenUrl": "https://login.bank.example/oauth2/token",
  "clientId": "donka-runtime",
  "secret": "CBS_CLIENT_SECRET",
  "scopes": ["accounts.read"]
}
```

The Runtime asks the token endpoint for a token with the client credentials grant (RFC 6749
§4.4): client id and secret in HTTP Basic, `scope` the scopes joined by spaces. It keeps the token
until a tenth of its lifetime before it expires (thirty seconds at most) and shares it between
calls; a token without `expires_in` is kept until the service refuses it. When the service answers
`401`, the token is dropped, a new one is asked for and the call is made once more. A token
endpoint that fails (5xx, 429, timeout, unreachable) counts as a failed attempt: it is retried and
counts towards the circuit breaker like the call itself; one that refuses the client (4xx) is not
retried. Tokens never appear in responses, traces or logs.

### Mutual TLS

```json
"tls": { "certificate": "CBS_CLIENT_CERT", "key": "CBS_CLIENT_KEY", "ca": "CBS_CA" }
```

```bash
DONKA_SECRET_CBS_CLIENT_CERT_FILE=/run/secrets/cbs-client.crt   # PEM, its chain may follow
DONKA_SECRET_CBS_CLIENT_KEY_FILE=/run/secrets/cbs-client.key     # PEM private key
DONKA_SECRET_CBS_CA_FILE=/run/secrets/cbs-ca.pem                 # PEM CA bundle (optional)
```

The Runtime presents the client certificate to the service, and to its token endpoint when the
node also uses `oauth2`. The service's certificate must chain to the CA bundle or to a public CA.
A certificate the service refuses, or a service certificate the Runtime does not trust, fails the
call with the reason (`the secure connection failed: received fatal alert: UnknownCA`); secrets
that hold no PEM certificate and key fail with their names. Key material never appears in an
error. The client is built at the first call that needs it: restart the Runtime after replacing a
certificate.

Every failure names the connector as its author named it in the decision:
`connector "Core banking": the token endpoint answered 503`.

## Retries and the circuit breaker

Server errors (5xx, 429, 408), timeouts and unreachable services are retried with a short
backoff (100 ms, doubling, at most 1 s); other refusals and non-JSON answers are not. The node's
trace (`trace: true`) shows the outcome (`ok`, `fallback`, `error`), the attempts, the status and
the error code. Other custom node kinds are not evaluated, as upstream.

The handler is the `donka-connectors` crate (`crates/connectors`, MIT). Studio uses the same
crate without its `live` feature to simulate connector nodes with their mock responses, and to
replay logged decisions with what each service answered at the time (`ConnectorAdapter::replay`).
