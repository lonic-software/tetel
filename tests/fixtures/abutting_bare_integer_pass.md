# Abutting-literal check — bare integer cross-reference pass fixture

The gateway's SLA is documented in appendix 2 [BI-1].

```tetel
id: BI-1
claim: The gateway's health endpoint responds within its SLA.
domain: src/gateway.rs#health
extent: src/gateway.rs#health
pin: abc123
kind: OBSERVED
run: curl -o /dev/null -s -w '%{time_total}' https://example.invalid/health
value: 31s
status: VERIFIED
```
