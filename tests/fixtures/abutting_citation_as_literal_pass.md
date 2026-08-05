# Abutting-literal check — citation-as-literal pass fixture

See the summary [CL-1] [CL-2].

```tetel
id: CL-1
claim: An unrelated row cited immediately before CL-2.
domain: a.rs#f
extent: a.rs#f
pin: abc123
kind: READING
status: VERIFIED
```

```tetel
id: CL-2
claim: The gateway's health endpoint responds within its SLA.
domain: src/gateway.rs#health
extent: src/gateway.rs#health
pin: abc123
kind: OBSERVED
run: curl -o /dev/null -s -w '%{time_total}' https://example.invalid/health
value: 31s
status: VERIFIED
```
