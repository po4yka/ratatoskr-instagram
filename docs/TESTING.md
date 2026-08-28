# Instagram connector testing strategy

Required tests:

- OAuth/account binding, encrypted credentials, refresh/revoke, scopes, and capability drift.
- Own-media capability no-op, current-generation binding, bounded request grammar, strict synthetic
  provider fixtures, durable cursor resume, completion-only watermark, atomic retained/refreshed/new
  authority, downgrade-at-finalization refusal, raw BlobRef truth, and outbox idempotence.
- URL classification/canonicalization and malicious URL cases.
- Explicit Share/Browser capture idempotency and provenance.
- Public post/reel resolution, deleted/private/unsupported/expired responses, unknown fields.
- Reference-only media defaults; rights/URL/type/size/owner-budget admission; redirect, MIME,
  length, digest, and shared-reference BlobStore deletion behavior.
- Safe Data Export import: ZIP slip/absolute/backslash/duplicate/symlink/encrypted/unsupported/truncated
  inputs, exact count/size/ratio limits, 256-case property runs, deterministic parser output,
  unknown evidence, replay-safe reconciliation, exact completeness math, owner isolation, and
  terminal restart behavior. A local 60-second libFuzzer harness is available under `fuzz/`.
- Missing-data versus deletion semantics.
- Closed deletion completeness inventory; cross-owner refusal; preview/apply fidelity; duplicate and
  final capture behavior; connection credential/exclusive cleanup; shared evidence retention;
  idempotent outbox/audit/blob convergence and late Knowledge completion non-resurrection.
- Re-resolution eligibility and deletion-race checks; zero-I/O item/request/byte/deadline/concurrency/
  provider budget guards; unchanged versus updated publication accounting.
- Parser reprocessing receipt/parser refusal; dry-run zero mutation and apply report fidelity;
  bounded checkpoint resume/replay; omission retention; CLI JSON/stdout/stderr/exit contract.
- SQL schema initialization, outbox/inbox redelivery, and no-secret/content logging.
- Planned workspace mobile/extension capture -> Instagram -> Knowledge flow.

Fixtures are synthetic or user-authorized and scrubbed; no personal account is required in CI.
Normal tests download no provider media and contact no live Instagram account.
The admitted Data Export fixture is synthetic/redacted and proves parser behavior, not current
compatibility with a real Instagram owner export. Adding such evidence requires a separately
authorized, scrubbed fixture review; personal exports and media never enter the repository.
No test proves live provider behavior, real-owner export compatibility, downstream Knowledge
consumption, or fleet legacy cutover; those remain separate integration evidence boundaries.

## Test-first

A change is planned before it is built, and the plan is a task list in which behaviour arrives in
pairs: one task adds a failing test, the next makes it pass. `openspec/config.yaml` carries that
rule, which is what puts it into every planning and implementation request rather than only into this
document.

The loop:

1. Write the test the scenario names. Run it. Confirm it fails, and read the failure — a test that
   fails because it does not compile has proved nothing about the behaviour.
2. Write the smallest change that makes it pass. Run it again.
3. Refactor only once it is green, adding no test and changing no behaviour.

Two checks stand behind this, and neither of them can see the order:

- `openspec validate --archived`, in `.github/workflows/openspec.yml`, fails when a change was
  archived with a task left unticked.
- A step in `.github/workflows/fleet.yml` fails when this repository holds a manifest and a `ci.yml`
  that never runs a test.

`ratatoskr-workspace/docs/QUALITY_GATES.md` records why the order itself is not checkable, rather
than leaving the gap to be discovered.
