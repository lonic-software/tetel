# Abutting-literal check — pass fixture

Under load, the gateway answers at 31s [A-1], comfortably inside budget.

```tetel
id: A-1
claim: The gateway's health endpoint responds within its SLA.
domain: src/gateway.rs#health
extent: src/gateway.rs#health
pin: abc123
kind: OBSERVED
run: curl -o /dev/null -s -w '%{time_total}' https://example.invalid/health
value: 31s
status: VERIFIED
```
