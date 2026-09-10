# V2 dependency boundaries

`Cargo.lock` pins the resolved dependency graph; it does not establish that the
sources are available offline or that the dependencies have been security-audited.
The workspace currently has registry and local path dependencies, with no ActPlane
Git dependency. The Adapter emits the supported DSL subset; compiler acceptance
belongs to the actual AgentSight/ActPlane deployment.

## HTTP/TLS and unsafe inventory

| Dependency path | Purpose and review boundary |
|---|---|
| `asc-agentsight-client -> ureq -> rustls` | Synchronous HTTP/TLS for the AgentSight API, separate from daemon UDS transport. Locked rustls 0.23.43 forbids unsafe in its own crate. |
| `rustls / rustls-webpki -> ring` | Cryptographic primitives; contains unsafe/native code. Review advisories, supported platforms and upstream audit evidence. No claim of a zero-unsafe TLS dependency tree. |
| `ureq / Client -> url -> idna / ICU` | URL and domain-name handling. Keep input bounds and evaluate advisories for the resolved graph. |
| `tokio -> libc / mio / socket2` | Existing OS/socket boundary, also outside workspace-local `unsafe_code = "forbid"`. |
| `Client -> uuid (v5) -> sha1_smol` | Deterministic target identity, not an authentication or signature algorithm. The v5 feature is requested only by the Client. Workspace builds can still unify features. |

The removed `actplane-ifc-compiler -> serde_yaml -> unsafe-libyaml` chain is no
longer in this workspace lockfile. No HTTP/TLS library or crypto-provider switch
is part of this change. In particular, replacing ring with aws-lc-rs would add
an FFI-based crypto implementation, not prove that unsafe exposure decreased.

The daemon's normal/build dependency graph does not currently include the Client,
Adapter, ureq or ring. Verify that boundary separately from workspace tests:

```sh
cargo tree -p asc-daemon --edges normal,build --locked --offline
cargo tree -p asc-daemon --edges normal,build,features --locked --offline
```

## Release checks

- Keep `Cargo.lock` reviewed; inspect new dependencies, enabled features, licenses,
  source origins and build scripts. Retain the existing ban on local unsafe code.
- Use `cargo audit` for known advisories, or `cargo deny check` for advisories plus
  source/license/dependency policies. Record tool version, advisory database
  revision, findings and time-bounded exceptions. A successful build is not an
  advisory scan, and a clean scan does not prove absence of unknown defects.
- For critical dependencies, record trusted audit evidence and review upgrade
  diffs. `cargo vet` can manage these records; it does not perform the audit itself.
- Before offline packaging, obtain all dependency sources (including required
  target-specific packages). `cargo vendor --locked` supports both registry and
  Git sources; install its emitted source replacement configuration, then validate
  the intended build with `--frozen` in an isolated environment. A warm local cache
  passing `--offline` does not establish that an offline source bundle is complete.

This file registers the dependency boundary and follow-up release checks. No new
advisory scan, third-party source audit, vendor bundle or CI audit gate is claimed
by the reconciliation tests. These remain explicit release-engineering work.

References: [RustSec tooling](https://rustsec.org/),
[cargo-deny](https://embarkstudios.github.io/cargo-deny/),
[cargo-vet](https://mozilla.github.io/cargo-vet/),
[Cargo vendor](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html).
