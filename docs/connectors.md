# Connectors

A decision can call an outside service (a credit bureau, a KYC or AML provider) through a
connector node: a custom node of kind `donka.connector`, authored in Donka Studio. The Runtime
POSTs the node's JSON body (string values may be templates over the node's input, such as
`{{ applicant.nationalId }}`) and adds the JSON response to the node's output under `outputKey`.
When the call still fails after its retries, the node either fails the evaluation or continues
with the `fallback` the author defined (`onError`).

Connectors are the only way a decision reaches another system. JavaScript function nodes have no
network access: importing `http` and calling any address fails the evaluation with
`HTTP requests from function nodes are disabled`, before any connection is made (DNK-83).

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
CONNECTORS__ALLOWED_HOSTS=api.bureau.cm,cbs.bank.internal:8443   # hosts any connector may call
CONNECTORS__HOSTS__BUREAU_KEY=api.bureau.cm                       # hosts each secret may go to
```

## Where calls may go

A connector calls only the hosts the installation lists, and sends a secret only to the hosts
listed for that secret. This holds whatever URL a rule author writes, so a release cannot send a
secret, or a request, anywhere else (DNK-84):

- `CONNECTORS__ALLOWED_HOSTS`: every host a connector may call, comma-separated. Empty, the
  default, refuses every call.
- `CONNECTORS__HOSTS__<SECRET>`: the hosts `<SECRET>` may be sent to. A secret without a list
  is never sent. For OAuth2, the token endpoint's host is checked for the client secret, and the
  service's host too: list both. For mutual TLS, the certificate, key and CA secrets each need
  the service's host (and the token endpoint's, when there is one).
- An entry is a host name, which allows any port, or `host:port`; case does not matter.

A refused call is checked before any secret is read or anything is sent. It fails, or falls
back, with `host_not_allowed` or `secret_host_not_allowed`, and the message names the setting
to add. `donka-admin init` writes these lists for the connectors it configures, and
`donka-admin doctor` reports a secret without one.

Connectors never follow a redirect, which could carry a secret header to another host. A
`3xx` answer is the connector's error (`redirect`).

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
