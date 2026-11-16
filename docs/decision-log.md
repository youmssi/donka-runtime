# Decision log

When it is configured, the Runtime sends a record of every evaluation to Donka Studio's decision
log: the input, the output (or the error the caller received), the trace, the decision key, the
project, release and environment that answered, the time and the duration. Studio stores it
encrypted, makes it searchable, and can replay it against the same release.

```bash
DECISION_LOG__URL=https://studio.example/api/v1/decision-log/records
DECISION_LOG__TOKEN=dnk_log_...          # issued in Studio for this Runtime's environment
DECISION_LOG__BATCH_SIZE=100             # records sent together, at most
DECISION_LOG__BATCH_BYTES=4194304        # bytes sent together, at most
DECISION_LOG__FLUSH_INTERVAL=1000        # milliseconds a record waits for others
DECISION_LOG__QUEUE_CAPACITY=10000       # records held while Studio cannot be reached
DECISION_LOG__TIMEOUT=10000              # milliseconds allowed per send
DECISION_LOG__SHUTDOWN_TIMEOUT=10000     # milliseconds to send what is queued when stopping
```

Set both `DECISION_LOG__URL` and `DECISION_LOG__TOKEN`, or neither (the log is off); one without
the other stops startup.

- **Never slows an answer.** Records queue in memory and leave in batches from a background task.
  When the queue is full (Studio unreachable for long), new records are dropped and the drops
  are logged as errors.
- **Retried.** Timeouts, unreachable Studio, `408`, `429` and `5xx` are retried with backoff
  (1 s, doubling, at most 30 s). Any other refusal (a wrong token, a batch too large) is logged
  and the batch is dropped. Records Studio rejects one by one (an unknown release, an
  environment the token does not cover) are logged with their id and code.
- **On stop** (SIGTERM, Ctrl-C), requests in flight finish and queued records are sent within
  the shutdown timeout.
- **Reference.** Callers may send `X-Donka-Reference` (1 to 200 visible ASCII characters, e.g. an
  application number) to find the decision in Studio; anything else is refused with `400`.
  Every logged answer, successful or not, carries `X-Decision-Id`, the record's id.
- **Trace.** The trace is always recorded, for replay; the answer includes it only when the
  caller asks (`trace: true`), as before.
- Only artifacts that name their project, release and environment (as Studio writes them) are
  logged. Records hold no access token, and connector traces hold no secret.

The feed format is documented in Studio: `youmssi/donka`, `docs/decision-log-feed.md`.
