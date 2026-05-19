# Licensing

chronikl is released under the **Business Source License 1.1**. See [LICENSE](https://github.com/nsrosenqvist/chronikl/blob/main/LICENSE) for the full text.

---

## Free use

You may use chronikl freely for:

- **Personal use**
- **Educational use**
- **Open-source projects** (any project distributed under an OSI-approved license)
- **Non-production evaluation** at any organisation

## Commercial use

Commercial production use at a for-profit entity requires a commercial license. Contact `niklas.s.rosenqvist@gmail.com` to discuss terms.

## Change date

Each chronikl version becomes available under the **Apache License 2.0** three years after its release (or four years from first publication of that version under BUSL-1.1, whichever comes first).

## Activating a license

Once you have a license key:

```bash
chronikl license activate <KEY>
```

This verifies the key against the embedded ed25519 public key and stores it at `~/.config/chronikl/license.key`. You can also pass the key via `CHRONIKL_LICENSE_KEY` env var (handy for CI).

## Checking status

```bash
chronikl license status
# customer: Acme Corp (acme-001)
# issued:   2026-05-08
# expires:  2027-05-08
# status:   valid
```

When the key is within 30 days of expiring, status reads `expiring soon (N days)`. Expired keys read `expired` and don't count as active.

## Deactivating

```bash
chronikl license deactivate
```

Removes the on-disk key.

## What does a license unlock?

Currently nothing — chronikl's behaviour is identical with or without a valid license. The license check is informational. If you're using chronikl commercially in production, you need a license to comply with BUSL-1.1, but the binary doesn't gate any features behind it.

The presence/absence of an active license is reported in the [telemetry heartbeat](05-Configuration#telemetry) (just a boolean — no key material is transmitted).

## Related Pages

- [Configuration](05-Configuration) — `[license]` section + `CHRONIKL_LICENSE_KEY`
- [LICENSE](https://github.com/nsrosenqvist/chronikl/blob/main/LICENSE)
