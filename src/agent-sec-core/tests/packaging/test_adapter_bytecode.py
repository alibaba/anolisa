"""Adapter resource roots must stay free of CPython bytecode.

ANOLISA records a digest of an adapter's resource root at ``adapter enable``
time and re-derives it on ``adapter status``. Bytecode written by a Hook while
it runs used to change that digest, so a healthy adapter turned ``degraded``
just from being used (alibaba/anolisa#2252).

The fix has two halves and both are asserted here:

* every Python Hook launches with ``python3 -B``, so no new cache is written;
* a bounded sweep removes caches that already exist, because ``-B`` stops
  CPython from *writing* bytecode but not from *importing* bytecode it finds.

Excluding caches from the digest instead is not an option: CPython imports a
header-valid cache without reading its source, so bytecode kept out of the
digest would be executable content that nothing verifies.
"""

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SWEEPER = REPO_ROOT / "tools" / "clean-adapter-bytecode.sh"
POST_INSTALL = REPO_ROOT / "packaging" / "raw" / "hooks" / "post-install.sh"
CONTRACT = REPO_ROOT / ".anolisa" / "component.toml"

# (framework, resource root, manifest path relative to that root, placeholder)
FRAMEWORKS = [
    ("qwencode", "qwen-code-extension", "qwen-extension.json", "${extensionPath}"),
    (
        "codex",
        "codex-plugin/hooks-plugin",
        "hooks/hooks.json",
        "${PLUGIN_ROOT}",
    ),
    ("qoder", "qoder-plugin", "hooks/hooks.json", "${QODER_PLUGIN_ROOT}"),
    ("cosh", "cosh-extension", "cosh-extension.json", "${extensionPath}"),
]


def _hook_argvs(root: Path, manifest_rel: str, placeholder: str) -> list[list[str]]:
    """Expand every Python hook command in a manifest into a runnable argv."""
    manifest = json.loads((root / manifest_rel).read_text(encoding="utf-8"))
    argvs: list[list[str]] = []
    for hook_groups in manifest["hooks"].values():
        for group in hook_groups:
            for hook in group.get("hooks", []):
                command = hook.get("command")
                if not isinstance(command, str):
                    continue
                # Qwen and Codex put the script in `command`; Qoder splits it
                # into `command` + `args`. Both shapes normalize to one argv.
                argv = shlex.split(command) + list(hook.get("args", []))
                if not argv or "python" not in argv[0]:
                    continue
                argvs.append(
                    [
                        part.replace(placeholder, str(root)).replace("${/}", os.sep)
                        for part in argv
                    ]
                )
    assert argvs, f"no Python hooks found in {root / manifest_rel}"
    return argvs


def _tree_digest(root: Path) -> str:
    """Mirror of ANOLISA's ``adapter::util::digest_tree`` encoding.

    Files are hashed in sorted relative-path order as ``path\\0len\\0bytes``.
    Kept in step with the Rust side deliberately: this is the value that
    decides whether ``adapter status`` reports healthy or degraded.
    """
    digest = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        payload = path.read_bytes()
        digest.update(str(path.relative_to(root)).encode())
        digest.update(b"\0")
        digest.update(len(payload).to_bytes(8, "little"))
        digest.update(b"\0")
        digest.update(payload)
    return digest.hexdigest()


def _caches(root: Path) -> list[Path]:
    return sorted(p for p in root.rglob("__pycache__") if p.is_dir())


def _run_hooks(argvs: list[list[str]], *, write_bytecode: bool) -> None:
    """Run every hook once. Exit status is irrelevant — imports are the point.

    A hook that fails without agent-sec-cli on PATH has still imported its
    sibling modules, which is exactly when CPython would write a cache.
    """
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    # An ambient PYTHONDONTWRITEBYTECODE would mask a missing `-B` and make the
    # control run vacuous, so it is always cleared here.
    env.pop("PYTHONDONTWRITEBYTECODE", None)
    for argv in argvs:
        if write_bytecode:
            argv = [part for part in argv if part != "-B"]
        subprocess.run(
            [sys.executable if part == "python3" else part for part in argv],
            input="{}\n",
            capture_output=True,
            check=False,
            env=env,
            text=True,
            timeout=30,
        )


@pytest.mark.parametrize(
    ("framework", "root_rel", "manifest_rel", "placeholder"), FRAMEWORKS
)
def test_every_python_hook_launches_with_dash_b(
    framework: str, root_rel: str, manifest_rel: str, placeholder: str
) -> None:
    for argv in _hook_argvs(REPO_ROOT / root_rel, manifest_rel, placeholder):
        assert "-B" in argv, f"{framework} hook must launch with -B: {argv}"
        # `-B` only suppresses writes when the interpreter sees it as an
        # option, i.e. before the script path.
        assert argv.index("-B") < len(argv) - 1, f"-B must precede the script: {argv}"


@pytest.mark.parametrize(
    ("framework", "root_rel", "manifest_rel", "placeholder"), FRAMEWORKS
)
def test_hook_execution_writes_no_bytecode_and_keeps_digest(
    framework: str,
    root_rel: str,
    manifest_rel: str,
    placeholder: str,
    tmp_path: Path,
) -> None:
    control = tmp_path / "control"
    guarded = tmp_path / "guarded"
    # The source tree is not necessarily clean: the hook unit tests import
    # these modules in place, so a developer checkout usually carries
    # __pycache__ already. Sweep both copies first, otherwise an inherited
    # cache would be mistaken for one this test's hook run produced.
    for copy in (control, guarded):
        shutil.copytree(REPO_ROOT / root_rel, copy)
        subprocess.run([str(SWEEPER), str(copy)], check=True, capture_output=True)
        assert _caches(copy) == [], "fixture must start without bytecode"

    # Control run: strip `-B` back out and confirm this framework's hooks do
    # write caches. Without this the -B assertion below could pass simply
    # because a hook crashed before importing anything.
    _run_hooks(
        _hook_argvs(control, manifest_rel, placeholder),
        write_bytecode=True,
    )
    if not _caches(control):
        pytest.skip(f"{framework} hooks import no local modules on this host")

    before = _tree_digest(guarded)
    _run_hooks(
        _hook_argvs(guarded, manifest_rel, placeholder),
        write_bytecode=False,
    )

    assert _caches(guarded) == [], (
        f"{framework} hooks wrote bytecode into the resource root: "
        f"{[str(p) for p in _caches(guarded)]}"
    )
    # Same statement from ANOLISA's point of view: the enable-time digest still
    # matches, so `adapter status` stays healthy and re-enable is unnecessary.
    assert _tree_digest(guarded) == before


def _staged_root(root: Path) -> Path:
    """A resource root carrying pre-`-B` bytecode, as an upgraded host would."""
    (root / "hooks" / "__pycache__").mkdir(parents=True)
    (root / "hooks" / "hook_config.py").write_text("CONFIG = 1\n")
    (root / "hooks" / "__pycache__" / "hook_config.cpython-311.pyc").write_bytes(
        b"\xcb\r\r\nstale"
    )
    (root / "hooks" / "__pycache__" / "hook_config.cpython-311.opt-1.pyo").write_bytes(
        b"\xcb\r\r\nstale"
    )
    return root


def test_sweep_removes_historical_caches(tmp_path: Path) -> None:
    root = _staged_root(tmp_path / "qwencode")
    proc = subprocess.run(
        [str(SWEEPER), str(root)], capture_output=True, text=True, check=False
    )

    assert proc.returncode == 0, proc.stderr
    assert _caches(root) == []
    assert (root / "hooks" / "hook_config.py").exists(), "sources must survive"


def test_sweep_skips_roots_that_are_not_installed(tmp_path: Path) -> None:
    proc = subprocess.run(
        [str(SWEEPER), str(tmp_path / "absent")],
        capture_output=True,
        text=True,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr


@pytest.mark.skipif(os.geteuid() == 0, reason="root ignores directory permissions")
def test_sweep_fails_when_a_directory_cannot_be_enumerated(tmp_path: Path) -> None:
    """An unreadable subtree must abort, never report a clean sweep.

    Regression guard: enumerating through a process substitution hides find's
    exit status, so a partial listing used to look like success.
    """
    root = _staged_root(tmp_path / "qwencode")
    locked = root / "locked"
    (locked / "__pycache__").mkdir(parents=True)
    (locked / "__pycache__" / "x.cpython-311.pyc").write_bytes(b"\xcb\r\r\n")
    locked.chmod(0o000)
    try:
        proc = subprocess.run(
            [str(SWEEPER), str(root)], capture_output=True, text=True, check=False
        )
    finally:
        locked.chmod(0o755)

    assert proc.returncode == 1
    assert "failed to enumerate" in proc.stderr


def test_sweep_handles_paths_with_spaces(tmp_path: Path) -> None:
    root = tmp_path / "qwencode"
    cache = root / "hook dir" / "__pycache__"
    cache.mkdir(parents=True)
    (cache / "hook.cpython-311.pyc").write_bytes(b"\xcb\r\r\n")

    proc = subprocess.run(
        [str(SWEEPER), str(root)], capture_output=True, text=True, check=False
    )

    assert proc.returncode == 0, proc.stderr
    assert _caches(root) == []


def test_sweep_is_bounded_to_pycache_bytecode(tmp_path: Path) -> None:
    root = _staged_root(tmp_path / "qwencode")
    # Shipped bytecode outside __pycache__ is a managed resource, not a cache.
    (root / "shipped.pyc").write_bytes(b"\xcb\r\r\nshipped")
    (root / "hooks" / "sibling.pyc").write_bytes(b"\xcb\r\r\nsibling")
    # Anything else inside __pycache__ is the tampering signal the bundle
    # digest exists to surface, so it must be preserved and reported.
    (root / "hooks" / "__pycache__" / "payload.so").write_bytes(b"payload")

    proc = subprocess.run(
        [str(SWEEPER), str(root)], capture_output=True, text=True, check=False
    )

    assert proc.returncode == 1, "an unswept cache must fail loudly"
    assert "non-bytecode content" in proc.stderr
    assert (root / "hooks" / "__pycache__" / "payload.so").exists()
    assert (root / "shipped.pyc").exists()
    assert (root / "hooks" / "sibling.pyc").exists()
    # The bytecode it *could* identify is still gone.
    assert not (root / "hooks" / "__pycache__" / "hook_config.cpython-311.pyc").exists()


def _contract() -> dict:
    return tomllib.loads(CONTRACT.read_text(encoding="utf-8"))


def test_contract_declares_strict_post_install_sweep() -> None:
    """The raw backend's only sweep trigger is this hook, so it must be strict.

    RPM gets the same work from each hook subpackage's %post; raw relies on
    ANOLISA running `post_install` after files land and before an adapter's
    bundle digest is re-derived.
    """
    contract = _contract()
    hooks = contract["component"]["hooks"]
    post_install = [h for h in hooks if h["phase"] == "post_install"]
    assert len(post_install) == 1, hooks
    assert post_install[0]["script"].endswith("/post-install.sh")
    assert post_install[0]["strict"] is True, "a silent sweep failure is the bug"

    targets = {f["target"] for f in contract["component"]["layout"]["files"]}
    assert "{datadir}/hooks/{component}/post-install.sh" in targets
    assert (
        "{datadir}/hooks/{component}/clean-adapter-bytecode.sh" in targets
    ), "the hook is useless if the sweeper it execs is not delivered"


def _install_raw_tree(datadir: Path) -> Path:
    """Lay out an installed raw tree, hooks included, as ANOLISA would."""
    hooks_dir = datadir / "hooks" / "sec-core"
    hooks_dir.mkdir(parents=True)
    for src in (POST_INSTALL, SWEEPER):
        dest = hooks_dir / src.name
        shutil.copy(src, dest)
        dest.chmod(0o755)
    return hooks_dir / POST_INSTALL.name


def _declared_raw_roots(datadir: Path) -> list[Path]:
    """Adapter resource roots the contract places under the raw datadir."""
    roots = []
    for adapter in _contract()["adapters"]:
        dest = adapter["dest"].replace("{component}", "sec-core")
        roots.append(Path(dest.replace("{datadir}", str(datadir))))
    return roots


def test_post_install_hook_sweeps_every_declared_adapter_root(tmp_path: Path) -> None:
    """Old install carrying caches -> new version's hook runs -> caches gone.

    Covers every `[[adapters]]` dest, so adding an adapter without adding its
    root to the hook fails here instead of shipping a half-swept upgrade.
    """
    datadir = tmp_path / "share" / "anolisa"
    hook = _install_raw_tree(datadir)

    roots = _declared_raw_roots(datadir)
    assert roots, "contract must declare adapters"
    for root in roots:
        cache = root / "hooks" / "__pycache__"
        cache.mkdir(parents=True)
        (cache / "hook_config.cpython-311.pyc").write_bytes(b"\xcb\r\r\nstale")
        (root / "hooks" / "hook_config.py").write_text("CONFIG = 1\n")

    proc = subprocess.run([str(hook)], capture_output=True, text=True, check=False)

    assert proc.returncode == 0, proc.stderr
    for root in roots:
        assert _caches(root) == [], f"{root} still holds bytecode: {proc.stdout}"
        assert (root / "hooks" / "hook_config.py").exists(), "sources must survive"


def test_post_install_hook_fails_loudly_without_its_sweeper(tmp_path: Path) -> None:
    datadir = tmp_path / "share" / "anolisa"
    hook = _install_raw_tree(datadir)
    (hook.parent / SWEEPER.name).unlink()

    proc = subprocess.run([str(hook)], capture_output=True, text=True, check=False)

    assert proc.returncode == 1
    assert "missing bytecode sweeper" in proc.stderr


def test_post_install_hook_leaves_shared_directories_alone(tmp_path: Path) -> None:
    """The sweep must not reach outside sec-core's own resource roots.

    `{datadir}/skills` is shared with other components. Sweeping it would
    delete bytecode sec-core does not own, and foreign content in one of its
    caches would fail this strict hook and roll back a sec-core update for an
    unrelated component's files.
    """
    datadir = tmp_path / "share" / "anolisa"
    hook = _install_raw_tree(datadir)
    for root in _declared_raw_roots(datadir):
        root.mkdir(parents=True, exist_ok=True)

    shared = datadir / "skills" / "other-component" / "__pycache__"
    shared.mkdir(parents=True)
    foreign = shared / "helper.cpython-311.pyc"
    foreign.write_bytes(b"\xcb\r\r\nnot ours")
    # Content that would make the sweeper fail if it ever looked here.
    (shared / "payload.so").write_bytes(b"payload")

    proc = subprocess.run([str(hook)], capture_output=True, text=True, check=False)

    assert proc.returncode == 0, proc.stderr
    assert foreign.exists(), "another component's bytecode must survive"
    assert (shared / "payload.so").exists()


def test_post_install_hook_covers_only_contract_declared_roots() -> None:
    """Every path the hook sweeps must be an `[[adapters]]` dest.

    Pins the boundary the other way round from
    `test_post_install_hook_sweeps_every_declared_adapter_root`: that one
    catches a missing root, this one catches an added extra.
    """
    swept = {
        line.strip().strip("\\").strip().strip('"').replace("$DATADIR/", "")
        for line in POST_INSTALL.read_text(encoding="utf-8").splitlines()
        if line.strip().startswith('"$DATADIR/')
    }
    declared = {
        adapter["dest"]
        .replace("{component}", "sec-core")
        .replace("{datadir}/", "")
        .rstrip("/")
        for adapter in _contract()["adapters"]
    }
    assert swept, "hook must pass roots to the sweeper"
    assert swept <= declared, f"swept paths outside the contract: {swept - declared}"
