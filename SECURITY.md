# Security policy

## Reporting a vulnerability

Please report security problems in Donka Runtime privately, through GitHub's
[private vulnerability reporting](https://github.com/youmssi/donka-runtime/security/advisories/new)
("Report a vulnerability" on the repository's **Security** tab). Do not open a public issue,
pull request or discussion for a vulnerability.

Include what you can of:
- the version or commit affected;
- the steps to reproduce, or a proof of concept;
- what an attacker gains;
- any suggested fix.

## What happens next

| Step | Target |
| --- | --- |
| We acknowledge your report | 3 business days |
| We confirm whether it is a vulnerability, and its severity | 10 business days |
| A fix is released, depending on severity | Critical 7 days, high 30 days, medium 90 days |

We keep you informed until the fix is released. We credit you in the advisory unless you prefer
otherwise. We publish a GitHub security advisory once customers have had the fix.

## Supported versions

Security fixes go to the latest release. Customers running an older version upgrade with
`donka-admin upgrade`.

## Safe harbour

Research done in good faith is welcome. Within this policy, we will not take legal action
against you. Good faith means:
- testing only installations you own or are allowed to test;
- not accessing, keeping or changing other people's data beyond what you need to show the
  problem;
- giving us reasonable time to fix it before you disclose.

Donka's threat model is in the Studio repository:
[docs/security/threat-model.md](https://github.com/youmssi/donka/blob/develop/docs/security/threat-model.md).
