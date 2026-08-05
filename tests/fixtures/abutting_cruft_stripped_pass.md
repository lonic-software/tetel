# Abutting-literal check — backtick/quote cruft stripped pass fixture

The gateway answers at `31s` [CS-1].

```tetel
id: CS-1
claim: The gateway's health endpoint responds within its SLA.
domain: src/gateway.rs#health
extent: src/gateway.rs#health
pin: abc123
kind: OBSERVED
run: curl -o /dev/null -s -w '%{time_total}' https://example.invalid/health
value: 31s
status: VERIFIED
```
