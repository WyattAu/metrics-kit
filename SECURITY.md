# Security Policy — metrics-kit

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅        |

## Reporting a vulnerability

Report privately via [GitHub security advisories] for this repository, or
email **wyatt_au@protonmail.com**. Do **not** open a public issue for
security reports.

You will receive an acknowledgement within **72 hours**. Coordinated
disclosure: we ask for up to 90 days before public disclosure while a
patch ships.

## Scope notes

`metrics-kit` renders metrics derived from in-process counters. Security
considerations for integrators:

- **Label values are escaped** (`\`, `"`, `\n`) before exposition, but
  label values containing request-controlled data (paths, hosts, emails)
  should still be avoided — they are a cardinality and data-leak vector,
  and the registry's series budget exists to bound the blast radius.
- The `axum` metrics route is **unauthenticated by design**; bind it on an
  internal interface or wrap it with your auth middleware.
- No allocations from request-controlled strings occur on the hot path.
- `#![forbid(unsafe_code)]` — no unsafe blocks exist in this crate.

[GitHub security advisories]:
    https://github.com/WyattAu/metrics-kit/security/advisories/new
