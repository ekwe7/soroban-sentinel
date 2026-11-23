# Soroban Sentinel GitHub Action

Use the composite action to run a read-only TTL scan in CI:

```yaml
name: Scheduled Sentinel TTL Scan

on:
  schedule:
    - cron: "0 */6 * * *"
  workflow_dispatch:

permissions:
  contents: read

jobs:
  sentinel:
    runs-on: ubuntu-latest
    steps:
      - name: Scan testnet contract
        uses: ekwe7/soroban-sentinel@v0
        with:
          contract-id: CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM
          network: testnet
          keys: |
            COUNTER
            ADMIN
          fail-on: never
```

The action downloads a matching binary for the runner OS and architecture, verifies its SHA-256 entry from `SHA256SUMS`, and fails closed on mismatch or RPC failure. It writes a Markdown report to `$GITHUB_STEP_SUMMARY` and exposes `status` and `exit-code` outputs.

## Inputs

- `contract-id` (required)
- `network` (default `testnet`)
- `rpc-url` (optional)
- `keys` (newline-separated persistent key symbols)
- `threshold` (default `120960`)
- `critical-threshold` (optional)
- `version` (default `latest`)
- `fail-on`: `warn`, `critical`, or `never`

## Failure behavior

- `warn`: WARN, CRITICAL, or ARCHIVED results fail.
- `critical`: WARN passes; CRITICAL or ARCHIVED fails.
- `never`: risk results pass, but RPC and malformed-input failures still fail.

The scheduled example intentionally uses testnet and `fail-on: never`; it is a connectivity/example scan and does not authorize production operations.
