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

## 6. OS-adapter rule ownership and compatibility

SkillFS does not own or ship OS mappings; it consumes a read-only rule artifact
produced by a **separate rule provider**. The artifact is a top-level YAML
sequence loaded and validated once at mount startup.

### Explicit provider contract

- Each rule declares `ubuntu`, `alinux`, `direction`, and a **required**
  `auto_apply` (`always` | `never`). Eligibility is governed solely by
  `auto_apply`; only `always` rules are applied, and only in a direction the
  resolved target permits.
- `confidence` and `notes` are accepted but inert — SkillFS attaches no behavior
  to them. This is a deliberate break from the earlier prototype's fuzzy,
  unenforced `confidence` semantics.
- A legacy artifact that omits `auto_apply` is rejected with an indexed error,
  rather than defaulting to applied.
- Duplicate and ambiguous active mappings are rejected. A many-to-one forward
  mapping must resolve reverse ambiguity explicitly: exactly one pair is
  `bidirectional` (the canonical reverse) and the alternates are direction-
  scoped (`ubuntu_to_alinux_only` / `alinux_to_ubuntu_only`). Rule order is
  significant, so more specific patterns precede shorter ones.

### Fail-closed OS detection

`target_os = "auto"` maps the exact `/etc/os-release` `ID`: `ubuntu`/`debian`
→ Ubuntu, `alinux`/`anolis` → Alinux. `ID_LIKE` is intentionally ignored, so
RHEL-family derivatives are not silently treated as Alinux and unrecognized
hosts reject the mount. Operators on other distributions must set `target_os`
explicitly.

### Migration note for the reference provider

The reference rule set that ships with the separate provider repository uses a
`confidence` field and does not carry an explicit `auto_apply`. It is a format
reference only and is **not** directly loadable as-is. Before SkillFS can
consume a provider artifact, the provider must:

1. add an explicit `auto_apply: always|never` to every rule; and
2. express many-to-one forward mappings with a single canonical `bidirectional`
   reverse plus direction-scoped alternates, so no reverse target is ambiguous.

## 7. Out of scope (first package)

Deliberately not implemented here: persistent transformed-content caching,
protocol-level transform audit events, additional text-file types, script
transforms, LLM/network calls in the read path, and rule hot-reload without a
remount. A future persistent cache would sit *after* stage execution and *below*
the Agent-visible read boundary, keyed by the selected target and rule digest;
it is called out so the current no-cache behavior is an explicit choice rather
than an omission.
