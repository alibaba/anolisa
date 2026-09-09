# V2 dependency boundaries

`Cargo.lock` pins the resolved dependency graph; it does not establish that the
sources are available offline or that the dependencies have been security-audited.
The workspace currently has registry and local path dependencies, with no ActPlane
Git dependency. The Adapter emits the supported DSL subset; compiler acceptance
belongs to the actual AgentSight/ActPlane deployment.

## HTTP/TLS and unsafe inventory

| Dependency path | Purpose and review boundary |
|---|---|
| `asc-daemon / asc-cli -> asc-observability[runtime] -> opentelemetry_sdk / tracing-opentelemetry` | Local context and stderr diagnostics only. No OTLP exporter, HTTP client or TLS provider in this path. |
| `asc-agentsight-client -> ureq -> rustls / rustls-webpki -> ring` | AgentSight API HTTP/TLS. ureq 2.12.1 explicitly selects the ring provider for its default TLS config. This Client is not yet wired into the daemon. |
| `rustls -> ring` | The AgentSight client crypto boundary contains unsafe/native code. Workspace-local unsafe prohibition does not establish a zero-unsafe dependency tree. |
| `ureq / clients -> url -> idna / ICU` | URL and domain-name handling. Keep input bounds and evaluate advisories for the resolved graph. |
| `tokio -> libc / mio / socket2` | Existing OS/socket boundary, also outside workspace-local `unsafe_code = "forbid"`. |
| `Client -> uuid (v5) -> sha1_smol` | Deterministic target identity, not an authentication or signature algorithm. The v5 feature is requested only by the Client. Workspace builds can still unify features. |

## Provider choice and build scope

The production OTel runtime is local-only. Removing its exporter also removes
`opentelemetry-otlp`, reqwest, hyper and AWS-LC from the workspace lockfile.
The current Linux normal/build graph for `asc-daemon` and `asc-cli` contains no
HTTP/TLS stack. The AgentSight Client still uses ureq with rustls/ring, and is not
yet wired into the daemon. A workspace build includes that separate client.
The daemon service itself remains UDS-only; TLS belongs to outbound clients.

Cargo unifies `asc-observability/runtime` across selected workspace members. The
feature exposes process initialization helpers but no longer enables exporter
or HTTP dependencies. Only product main installs the process-singleton runtime;
depending on the crate does not automatically initialize OTel globals.

Verify both scopes from `v2/` after dependency or composition changes:

```sh
cargo tree -p asc-daemon -p asc-cli --edges normal,build,features --locked --offline
cargo tree --workspace --edges normal,build,features --locked --offline
```

The removed `actplane-ifc-compiler -> serde_yaml -> unsafe-libyaml` chain remains
absent from this workspace lockfile.

## Native build and packaging

Local tracing adds no C/CMake crypto toolchain. Building the whole workspace
still includes ring's native compilation through the AgentSight Client; review
its target-specific prerequisites when that client enters product packaging.
The current [RPM spec](../agent-sec-core.spec.in) builds V1 and other packaged
components, not the V2 workspace. Do not infer V2 release packaging coverage
from a successful developer workspace build.

## Release checks

- Keep `Cargo.lock` reviewed; inspect new dependencies, enabled features, licenses,
  source origins and build scripts. Retain the existing ban on local unsafe code.
- Recheck the product and workspace feature graphs above, including crypto
  provider selection and native build requirements. A dependency upgrade must
  not silently invalidate this inventory or the release environment's prerequisites.
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
by the tracing or reconciliation tests. These remain explicit release-engineering work.

References: [RustSec tooling](https://rustsec.org/),
[cargo-deny](https://embarkstudios.github.io/cargo-deny/),
[cargo-vet](https://mozilla.github.io/cargo-vet/),
[Cargo vendor](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html).
