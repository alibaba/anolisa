# ANOLISA Component Contract

This document defines where a packaged component exposes its ANOLISA
component contract and how ANOLISA should consume that contract across RPM and
raw backends.

## Package Location

RPM packages that expose ANOLISA component metadata MUST install the component
contract at:

```text
/usr/share/anolisa/components/<component>/component.toml
```

In RPM spec files, use the datadir macro rather than hard-coding
`/usr/share`:

```spec
%global anolisa_component sec-core

install -d -m 0755 %{buildroot}%{_datadir}/anolisa/components/%{anolisa_component}
install -m 0644 .anolisa/component.toml \
  %{buildroot}%{_datadir}/anolisa/components/%{anolisa_component}/component.toml

%files
%dir %{_datadir}/anolisa/components
%dir %{_datadir}/anolisa/components/%{anolisa_component}
%{_datadir}/anolisa/components/%{anolisa_component}/component.toml
```

Examples:

```text
/usr/share/anolisa/components/sec-core/component.toml
/usr/share/anolisa/components/tokenless/component.toml
/usr/share/anolisa/components/os-skills/component.toml
```

## Rationale

`component.toml` is static, package-owned, architecture-independent metadata.
Under the Filesystem Hierarchy Standard, that makes `/usr/share` the right
system location.

Do not install the component contract under:

- `/etc`: reserved for administrator-editable configuration.
- `/var/lib`: reserved for runtime state.
- `/usr/libexec`: reserved for helper executables.
- `/opt`: reserved for private package trees, not ANOLISA discovery contracts.
- `/usr/share/anolisa/adapters/<component>`: reserved for adapter payloads.

The adapter payload tree remains separate:

```text
/usr/share/anolisa/adapters/<component>/<framework>/...
```

The component contract is component-level metadata. It may describe adapters,
services, health checks, files, backend compatibility, and future lifecycle
behavior, so it should not live inside the adapter namespace.

## User And Raw Installs

For user-mode or raw installs, the same logical datadir layout applies.
ANOLISA follows the user roots described by `file-hierarchy(7)`: the default
data root is `~/.local/share`, and `XDG_DATA_HOME` may override that data root.

The default location is:

```text
~/.local/share/anolisa/components/<component>/component.toml
```

When `XDG_DATA_HOME` is set, use the overridden data root:

```text
$XDG_DATA_HOME/anolisa/components/<component>/component.toml
```

Raw archives may also carry the source contract at:

```text
.anolisa/component.toml
```

ANOLISA should normalize that source contract into the installed datadir layout
or directly into the installed-state snapshot described below.

## Installed-State Snapshot

ANOLISA should keep package-owned contract files separate from its runtime
state. After install or adopt, ANOLISA may copy the resolved contract into its
state directory:

```text
{state_dir}/component-manifests/<component>/component.toml
```

Typical paths:

```text
/var/lib/anolisa/component-manifests/<component>/component.toml
~/.local/state/anolisa/component-manifests/<component>/component.toml
```

The package-owned contract is the source provided by RPM or raw artifacts. The
state snapshot is ANOLISA's runtime record and may be used by commands such as
`anolisa adapter enable <component> <framework>` after the component has been
installed or adopted.

## Discovery Order

For an installed component, ANOLISA should resolve the component contract in
this order:

1. Existing installed-state snapshot:
   `{state_dir}/component-manifests/<component>/component.toml`.
2. Package datadir contract:
   `{datadir}/components/<component>/component.toml`.
3. Raw archive source contract during install:
   `.anolisa/component.toml`.

If an RPM-installed component has no package datadir contract, commands should
treat adapter declarations as unavailable and report that the RPM does not
publish an ANOLISA component contract.

## Adapter Operation Notices

An adapter contract may declare static, display-only operator notices that
ANOLISA shows after `adapter enable` or `adapter disable` succeeds. Notices
are declared with `[[adapters.notices]]` on a generic adapter entry, or with
`[[adapters.openclaw.notices]]` / `[[adapters.hermes.notices]]` in a
framework-specific section (which takes precedence over the generic entry):

```toml
[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "tokenless"

[[adapters.notices]]
when = "post_enable"
level = "info"
text = "Restart the framework to load the plugin."
command = "openclaw restart"

[[adapters.notices]]
when = "post_disable"
level = "warning"
text = "Cached tokens remain until the framework restarts."
```

Each notice has:

- `when` (required): `post_enable` or `post_disable`.
- `level` (optional): `info` (default) or `warning`.
- `text` (required): the notice body.
- `command` (optional): a display-only command hint.

Notices are inert text. `text` and `command` are never shell-expanded,
template-substituted, or executed. Human-readable output escapes control
characters to protect terminal state, while structured JSON preserves the
declared values. Required framework configuration is not a notice — it stays
a structured `[[adapters.config]]` entry.

Display behavior:

- `adapter enable` shows `post_enable` notices after a successful enable;
  `adapter disable` shows `post_disable` notices after a successful disable.
  `post_disable` notices are taken from the enable-time receipt, so they are
  shown even if the component manifest is no longer present.
- Human-readable output prints the notices; `--quiet` suppresses them.
- `--json` returns the notices in a stable `data.notices` array.
- `--dry-run` previews the notices that a real operation would show, labeled
  as a preview; nothing is executed.

## Contract Template

Use a shipped component manifest such as
[`manifests/components/cosh/component.toml`](../manifests/components/cosh/component.toml)
as the example schema for new component contracts.
