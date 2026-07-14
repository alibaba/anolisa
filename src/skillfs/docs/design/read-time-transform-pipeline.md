# Read-Time Transform Pipeline

Design notes for the `SKILL.md` read-time transform pipeline: the ordering
contract, the optional stage model, immutability guarantees, and the external
rule-provider contract. This document describes component-internal design; the
user-facing configuration reference lives in the SkillFS README and
`docs/user-guide/{en,zh}/runtime/skillfs.md`.

## 1. Activation before transformation

A `SKILL.md` read is served in a strict order:

1. Parse the Agent-visible path.
2. Resolve activation to `Current`, `Snapshot`, or `Hidden`.
3. `Hidden` returns `ENOENT` immediately — it never enters the pipeline.
4. Read the bytes of the *selected* target only: the live source for `Current`,
   the trusted snapshot for `Snapshot`.
5. Run the configured transform stages over those bytes.
6. Serve the transformed bytes and report their exact length from `getattr`.

Transformation is therefore always downstream of the security decision. A
`Snapshot` read transforms the snapshot bytes and never falls back to the live
source, even if a stage errors; a `Hidden` skill is never read or transformed.

### Pinned open handles

An open read handle captures the skill's `ActiveTarget` at open time and stores
it on the handle. Subsequent reads resolve against that pinned target rather
than re-consulting the live resolver, so a resolver change after open cannot
re-point an in-flight read (a `Snapshot` handle keeps reading the snapshot; a
`Current` handle stays readable if the skill is later hidden). Flat and Hermes
nested `SKILL.md` share this contract via the same pinned-resolution helper.

`getattr` operates on the inode and is not handle-pinned: after an activation
change, a fresh `stat` reflects the new target's transformed size. Open handles
continue to serve their pinned target's bytes; only the kernel's cached size may
change. Within a stable activation state, `getattr` size, offset/partial reads,
and full reads all agree on the transformed bytes.

## 2. Optional stages, fixed order

The pipeline holds each stage in a dedicated typed slot:

```text
directive: Option<DirectiveStage>
os_adapter: Option<OsAdapterStage>
```

`run` applies `directive` (if present) then `os_adapter` (if present) and
returns the input unchanged when both are absent. This makes three properties
structural rather than enforced by convention:

- **Fixed order** — `directive` always precedes `os_adapter`.
- **No duplicates** — a slot holds at most one stage; a stage cannot be added
  twice.
- **Optional** — either or both slots may be empty (directive-only, adapter-
  only, or a fully empty raw-passthrough pipeline).

The set of stages is decided once, at mount startup. `run` performs only
in-memory work; it never parses YAML, reads `/etc/os-release`, spawns a process,
touches the network, or calls an LLM.

### Why directive stays enabled by default

The directive/compiler stage predates this pipeline and is the historical
compile-on-read behavior. Keeping it enabled by default (when
`[transforms.directive]` is absent) preserves byte-for-byte output for existing
mounts. Making it *optional* — rather than a hardcoded first stage — lets it be
disabled for adapter-only or raw operation and leaves room to remove or replace
it later without reworking the pipeline shape.

## 3. Source and snapshot immutability

Transform stages are pure functions over the read bytes. They never write to
the source tree, snapshots, activation metadata, or the rule artifact. The only
observable effect is the bytes an Agent reads; the physical `SKILL.md` and any
trusted snapshot on disk are unchanged by a read.

## 4. Excluded paths

Only `SKILL.md` reads flow through the pipeline. The following never enter it:

- `.skill-meta/**` and lifecycle-reserved roots;
- activation JSON and xattrs;
- control-socket payloads;
- `skill-discover` virtual content;
- every other file type (other Markdown, shell, Python, YAML, JSON, TOML).

## 5. Transformed size and read semantics

The same transformed byte string backs `getattr` size, complete reads, and
offset/partial reads for a given resolved target, so a tool that stats then
reads (or reads in chunks) sees a consistent view. Content that grows or shrinks
under the OS adapter is reflected in the reported size.

## 6. OS-adapter rule catalog and compatibility

SkillFS ships a built-in Ubuntu/Alinux rule catalog and embeds it in the binary
from the repository asset `crates/skillfs-core/assets/ubuntu-alinux.yaml` via
`include_bytes!`, so the default adapter works in source builds, RPMs, and
containers without a separate on-disk file. An operator may override the default
with an external read-only artifact by setting a non-empty `rules_path`. Either
way the artifact is a top-level YAML sequence, loaded and validated once at
mount startup; `OsAdapterStage::load_default` compiles the built-in bytes and
`OsAdapterStage::load` compiles an external file. There is no second in-code
mapping table — the catalog lives only in the asset.

### Built-in catalog composition

The bundled catalog carries **311 rules** covering package-manager verbs,
`-dev`/`-devel` package names, service unit names, and filesystem paths. Each
rule's eligibility is normalized to an explicit `auto_apply`: the 257
high-confidence rules are `auto_apply: always` and the 51 medium plus 3 low
rules are `auto_apply: never`. Medium and low rules are therefore documented in
the catalog but never applied — SkillFS performs no verification, Repology
lookup, network call, subprocess, or LLM review to promote them. After
normalization the catalog produces **223** non-identity active substitutions for
target Alinux and **192** for target Ubuntu, with no duplicate or ambiguous
active mapping for either target.

### Explicit rule contract (built-in and external)

- Each rule declares `ubuntu`, `alinux`, `direction`, and a **required**
  `auto_apply` (`always` | `never`). Eligibility is governed solely by
  `auto_apply`; only `always` rules are applied, and only in a direction the
  resolved target permits.
- `confidence` and `notes` are accepted but inert — SkillFS attaches no behavior
  to them.
- An artifact that omits `auto_apply` on any rule is rejected with an indexed
  error, rather than defaulting to applied. This applies to external override
  artifacts too: they must carry explicit `auto_apply`.
- Duplicate and ambiguous active mappings are rejected. A many-to-one forward
  mapping must resolve reverse ambiguity explicitly: exactly one pair is
  `bidirectional` (the canonical reverse) and the alternates are direction-
  scoped (`ubuntu_to_alinux_only` / `alinux_to_ubuntu_only`). The built-in
  catalog applies this to the apt shorthand verbs (`apt update`, `apt upgrade`,
  …), which are `ubuntu_to_alinux_only` so the reverse target uses the canonical
  `apt-get`/`apt-cache` spelling without collision.

### Non-cascading substitution

`apply` runs a single left-to-right pass over the *original* read bytes. At each
position it selects the **longest** matching source pattern (most specific
wins), emits that rule's declared target, and advances past the consumed span.
Neither the replacement text nor already-scanned input is rescanned, so:

- overlapping patterns never chain — `apache2` does not rewrite the inside of
  `apache2-utils`, and `cron` does not re-hit the `crond` a more specific rule
  produced;
- each rule is a 1:1 map to its declared target, independent of file order (two
  distinct sources of equal length cannot both match at one position, so the
  longest match is unambiguous).

A naive per-rule sequential `replace` would corrupt these cases; the single-pass
scan is the correctness fix.

**Protection matches.** Ineligible patterns — `auto_apply: never`, identity
(`from == to`), and direction-disallowed for the resolved target — also take
part in the scan, as *protection* matches: when one is the longest match at a
position it is emitted verbatim and skipped. Without this, dropping ineligible
rules from the compiled table let a shorter eligible rule rewrite inside a span
an ineligible rule claims (e.g. the `never` path `/etc/init.d/apache2` becoming
`/etc/init.d/httpd`, or the identity `postgresql-contrib` becoming
`postgresql-client-contrib` on reverse), silently bypassing eligibility. An
eligible substitution always wins over protection for the same source, so a
direction-disallowed alternate never suppresses the canonical reverse mapping it
shares a target with.

### Fail-closed OS detection

`target_os = "auto"` maps the exact `/etc/os-release` `ID`: `ubuntu`/`debian`
→ Ubuntu, `alinux`/`anolis` → Alinux. `ID_LIKE` is intentionally ignored, so
RHEL-family derivatives are not silently treated as Alinux and unrecognized
hosts reject the mount. Operators on other distributions must set `target_os`
explicitly. A present-but-blank `rules_path` is rejected as a misconfiguration
rather than silently falling back to the built-in catalog.

## 7. Out of scope (first package)

Deliberately not implemented here: persistent transformed-content caching,
protocol-level transform audit events, additional text-file types, script
transforms, LLM/network calls in the read path, and rule hot-reload without a
remount. A future persistent cache would sit *after* stage execution and *below*
the Agent-visible read boundary, keyed by the selected target and rule digest;
it is called out so the current no-cache behavior is an explicit choice rather
than an omission.
