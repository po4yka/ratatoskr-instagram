## 1. Audit evidence

- [x] 1.1 Retain the full failing fuzz audit before the policy and TLS lock edits. Configuration has no separate behavioral RED; observed exit 5 identifies four Rustls advisories and two license failures.
- [x] 1.2 Align only the reviewed Rustls record, declare the existing package license, add the exact tool exception, and observe locked metadata plus full root and fuzz policy success.

## 2. Enforcement and publication

- [ ] 2.1 Add both fuzz commands to the appropriate CI jobs and developer instructions; run the full repository gates and independent review. CI configuration and documentation have no behavioral RED.
- [ ] 2.2 Publish the reviewed commit, wait for exact-commit CI and record the result in SEC-030 before workspace pin promotion. Publication metadata has no behavioral RED.
