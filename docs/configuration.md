# Configuration

The Runtime reads its settings from environment variables. It listens on `0.0.0.0:8080`
(`127.0.0.1:3000` in a debug build).

## Release storage

Where the Runtime finds published releases. Donka Studio publishes each environment's releases
under a prefix (`staging/`, `production/`): point each Runtime at its environment's prefix.

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

## Rate limits

Each access token may make a set number of requests per window; past that, the Runtime answers
`429 Too Many Requests` with `Retry-After` (whole seconds) before evaluating anything. Off unless
`RATE_LIMIT__REQUESTS` is set.

```bash
RATE_LIMIT__REQUESTS=600     # requests per token per window
RATE_LIMIT__WINDOW=60000     # the window, in milliseconds (default one minute)
```

- **What counts**: every call under `/api/projects/{project}/…` and `/api/rules/{project}…` made
  with a token the project accepts. `/api/health`, `/api/version` and the API docs never count.
- **Per token**: each token has its own allowance; a project that accepts calls without a token
  shares one allowance among them. A wrong token gets its usual `401` and uses nothing.
- **Bursts**: a whole window's allowance may arrive at once; after that, requests come back
  steadily (one every `WINDOW / REQUESTS`).
- **Answer**: on `/api/projects` the body is `{ "message": … }`; on `/api/rules` it is
  `{ "code": "rateLimit.exceeded", "retryAfter": <seconds> }`.
- **Per instance**: each Runtime counts on its own, so behind a load balancer with three
  instances a token may make up to three times the limit in total.

## Other features

- Connectors (outside calls from a decision): [connectors.md](connectors.md)
- Decision log (records sent to Studio): [decision-log.md](decision-log.md)
- Rules OpenAPI and type resolution: [rules-openapi.md](rules-openapi.md)
