# anolisa CLI

The `anolisa` CLI is the unified lifecycle entry point for ANOLISA components.
It resolves component sources, keeps scoped installation records, delegates RPM
transactions to the native package manager, and diagnoses or repairs drift.

---

## Installation

### Option A: Install script (recommended)

```bash
curl -fsSL https://agentic-os.sh | sh
```

### Option B: YUM (Alinux)

```bash
sudo yum install anolisa
```

Verify installation:

```bash
anolisa --version
```

---

## Scope And Visibility

`--install-mode user` writes under the current user's roots, while
`--install-mode system` writes system state and normally requires root. When
the option is omitted, root defaults to system mode and a regular user defaults
to user mode.

Read-only commands use a user-plus-system view. A regular user can therefore
see and diagnose a system installation, and adapter discovery can use its
published contract. Mutating commands still write only the explicitly selected
scope. In particular, a user installation may coexist with a system
installation of the same component.

---

## Commands

### install

Install one component through the configured raw or RPM backend, or plan every
component in the index:

```bash
anolisa --install-mode user install <component>
sudo anolisa --install-mode system install <component>
anolisa install --all
```

An installation in the other scope does not make the selected scope
"already installed." Reinstalling or changing an existing record is handled
by lifecycle planning rather than silently overwriting it.

### uninstall

Remove one installation from the selected scope:

```bash
anolisa uninstall <component>
anolisa uninstall <component> --purge
sudo anolisa --install-mode system uninstall <component> --remove-system-package
```

ANOLISA-owned files and managed RPM packages are removed by their owning
backend. Adopted or observed system RPMs are left installed by default; use
`--remove-system-package` only when native package removal is intended.

### update

Update one component, every recorded component, the CLI binary, or run the
read-only RPM update report:

```bash
anolisa update <component>
anolisa update all
anolisa update self
anolisa update --check
```

`update all` does not update the CLI binary. Delegated members are merged into
one native transaction where possible; each component keeps its own recovery
journal and record.

### list and status

Inspect the effective user-plus-system view:

```bash
anolisa list
anolisa list --installed
anolisa status
anolisa status <component>
```

In a user view with records in both scopes, the user record is active and the
system record remains visible as shadowed state. A system-mode view reads only
the system root; it does not enumerate other users' state.

### doctor

Run read-only health, dependency, service, state, and recovery-journal checks:

```bash
anolisa doctor
anolisa doctor <component>
anolisa --dry-run doctor <component>
```

`doctor` scans every root in the current visibility view: user mode includes
the user root and a readable system root, while system mode includes only the
system root. It qualifies system repair suggestions with
`sudo anolisa --install-mode system` when the current invocation cannot mutate
that root. `--fix` is reserved in this release; follow the reported `fix_plan`
explicitly.

### restart

Restart services recorded for an installation in the selected scope:

```bash
anolisa --install-mode user restart <component>
sudo anolisa --install-mode system restart <component>
```

### upgrade

Plan or apply the system/RPM image upgrade. Raw-managed components are reported
as skipped rather than migrated to another backend:

```bash
anolisa --install-mode system --dry-run upgrade
sudo anolisa --install-mode system upgrade
sudo anolisa --install-mode system upgrade --target <profile>
```

### adopt, repair, and forget

Manage state without confusing package ownership:

```bash
sudo anolisa --install-mode system adopt <component>
sudo anolisa --install-mode system repair <component>
anolisa --install-mode user forget <component>
sudo anolisa --install-mode system forget <component>
```

`adopt` records an existing system RPM as delegated-adopted without claiming
native removal authority. `repair` reconciles a scoped record with rpmdb or an
interrupted journal. `forget` removes only the record in the selected scope and
never performs package or owned-file removal; a user-scoped forget cannot
delete a visible system record.

### adapter

Manage component adapters:

```bash
anolisa adapter scan
anolisa adapter enable <component> [framework]
anolisa adapter disable <component> [framework]
anolisa adapter status [component]
```

### logs and bug reports

Inspect component logs or generate a diagnostic bundle:

```bash
anolisa logs <component>
anolisa logs <component> --limit 50
anolisa logs <component> --severity warn
anolisa bug
```

---

## Recovery Behavior

Install, uninstall, update, adopt, and repair write recovery intent in the
selected state root before their lifecycle side effects. Native package
operations are forward-only: if dnf may have committed but the ANOLISA record
did not, the journal remains pending and `anolisa repair <component>`
re-observes rpmdb. Owned-file operations keep verified backups and compensate
in reverse order on failure. `forget` is an atomic record-only state update; it
does not perform package/file side effects or create a recovery journal.

`upgrade` remains a compatibility orchestrator rather than a planner/journal
consumer. It refuses existing pending recovery and re-observes rpmdb after a
transaction failure, but it does not create a per-component recovery journal.
After an interrupted `upgrade`, run `anolisa doctor` and reconcile any reported
component drift before starting another lifecycle mutation.

Do not delete a pending journal merely to unblock a command. Run `doctor` to
identify its scope and subject, then run the qualified `repair` command. A
malformed or ambiguous journal is intentionally left pending for manual
inspection.

---

## Global Options

| Option | Description |
|--------|-------------|
| `--install-mode user\|system` | Select the mutation scope |
| `--prefix <PATH>` | Override the selected scope's install prefix |
| `--dry-run` | Print the plan without executing it |
| `--json` | Emit machine-readable JSON |
| `-v, --verbose` | Increase verbosity |
| `-q, --quiet` | Suppress non-error output |
| `--no-color` | Disable colored output |
| `--version` | Show the CLI version |
| `--help` | Show command help |

---

## Example Workflow

```bash
curl -fsSL https://agentic-os.sh | sh
anolisa env
anolisa install cosh
anolisa install tokenless
anolisa adapter enable tokenless cosh
anolisa doctor
anolisa status
```

---

## Configuration

Registry settings are read from `/etc/anolisa/config.toml` in system mode or
`~/.config/anolisa/config.toml` in user mode. Only the `[registry]` table is
used for registry resolution:

```toml
[registry]
url = "https://registry.example.com/index.toml"
cache_ttl_secs = 3600
offline_fallback = true
```

Backend selection and endpoints live in the corresponding `repo.toml`
(`/etc/anolisa/repo.toml` or `~/.config/anolisa/repo.toml`). CLI flags override
the operation being run; there is no `[install] mode` setting.

---

## See Also

- [Installation Guide](../installation.md)
- [Troubleshooting](../troubleshooting.md)
- [Install Lifecycle Design](../../../../src/anolisa/docs/design/install-lifecycle.md)
