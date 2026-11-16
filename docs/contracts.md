# Input contracts

A decision can declare what its request must contain: a JSON Schema on its input node, edited in
Studio as the decision's **Input fields** (DNK-37). The engine checks every request against it
before any rule runs, so the Runtime enforces the same definition a form is built from.

## A request that breaks the contract

The Runtime answers `400` and names the field:

```json
{
  "message": "/applicant/age: 12 is less than the minimum of 18",
  "contract": {
    "field": "applicant.age",
    "message": "12 is less than the minimum of 18"
  }
}
```

| Field | Meaning |
| --- | --- |
| `contract.field` | The field at fault, as a dotted path. A missing field is named itself (`applicant.age`), not its parent. Empty when the request as a whole is wrong. |
| `contract.message` | What is wrong, as the validator says it. |
| `message` | The engine's own text, as for any other failure. |

`contract` appears only when the request broke the contract; other failures (an expression that
cannot be evaluated, a connector that failed) answer `400` without it. The decision log records
the refused request like any failed evaluation.

Only `POST /api/projects/{project}/evaluate/{key}` adds `contract`; the rules API keeps its own
error shape.

## Empty values and dates

- `null` in an optional field counts as the field left out: the request passes. A required
  field must hold a value; `null` there is refused, naming the field.
- A field declared as a date (`"type": "string"` with `"format": "date"` or `"date-time"`)
  reaches the rules as a date, so date functions and comparisons work on it. It still reads and
  answers as the same text.

Both come from zen-engine 2.1 (DNK-62); before it, `null` in any typed field was refused.

## In the artifact

Studio writes each contract twice: inside the graph's input node, where the engine reads it, and
as `.config/contracts/<key>/input.schema.json` for forms and pipelines. The Runtime does not read
the second copy: everything under `.config/` is skipped when loading decisions. See
[artifact format](https://github.com/youmssi/donka/blob/develop/docs/artifact-format.md#input-contracts).
