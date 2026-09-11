"""DPROC-014: render and validate the actual V2 packaging target."""

import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def test_v2_stages_system_unit_and_matching_account(tmp_path):
    subprocess.run(
        [
            "make",
            "install-systemd-system",
            f"DESTDIR={tmp_path}",
            "SYSTEMD_SERVICE_BINDIR=/usr/bin",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    unit = tmp_path / "usr/lib/systemd/system/agent-sec-core.service"
    content = unit.read_text()
    assert 'ExecStart="/usr/bin/agent-sec-daemon" serve' in content
    assert "{bindir}" not in content
    assert not (tmp_path / "usr/lib/systemd/user").exists()
    for setting in (
        "User=agent-sec",
        "Group=agent-sec",
        "RuntimeDirectory=agent-sec-core",
        "RuntimeDirectoryMode=0755",
        "RuntimeDirectoryPreserve=yes",
        "Type=simple",
        "Restart=on-failure",
        "TimeoutStopSec=45",
        "KillMode=control-group",
        "StartLimitBurst=5",
        "StandardError=journal",
        "CapabilityBoundingSet=",
        "SystemCallFilter=@system-service",
        "SystemCallArchitectures=native",
        "MemoryDenyWriteExecute=true",
    ):
        assert setting in content.splitlines()
    sysusers = (tmp_path / "usr/lib/sysusers.d/agent-sec-core.conf").read_text()
    assert sysusers.strip() in (ROOT / "agent-sec-core.spec.v2.in").read_text()
    spec = (ROOT / "agent-sec-core.spec.v2.in").read_text()
    assert "%systemd_postun_with_restart agent-sec-core.service" in spec
    assert "systemd_user_" not in spec and "_userunitdir" not in spec
    assert "__strip" not in spec
    assert "__requires_exclude_from" not in spec
    assert "__provides_exclude_from" not in spec
    cli_package = spec.split("%package -n agent-sec-cli\n", 1)[1].split(
        "%description -n agent-sec-cli", 1
    )[0]
    assert "Requires(pre):  /usr/bin/systemd-sysusers" in cli_package
    assert all(name not in cli_package for name in ("python3", "gnupg2", "loongshield"))
    makefile = (ROOT / "Makefile").read_text()
    targets = [
        line
        for line in makefile.splitlines()
        if line.startswith("install-all-for-rpmbuild-v2:")
    ]
    assert any("install-systemd-system" in line for line in targets)
    assert all("install-systemd-user" not in line for line in targets)


def test_v2_manifest_staging_preserves_v1_scope(tmp_path):
    for version, source, scope in (
        ("v1", ".anolisa/component.toml", "user"),
        ("v2", ".anolisa/component-v2.toml", "system"),
    ):
        destination = tmp_path / version
        subprocess.run(
            [
                "make",
                "stage-component-manifest",
                "install-component-manifest",
                f"COMPONENT_MANIFEST={source}",
                f"BUILD_DIR={destination / 'build'}",
                f"DESTDIR={destination / 'install'}",
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        installed = (
            destination / "install/usr/share/anolisa/components/sec-core/component.toml"
        )
        staged = destination / "build/share/anolisa/components/sec-core/component.toml"
        assert (
            installed.read_bytes()
            == staged.read_bytes()
            == (ROOT / source).read_bytes()
        )
        component = tomllib.loads(installed.read_text())["component"]
        assert component["services"][0]["scope"] == scope
        if version == "v2":
            targets = {entry["target"] for entry in component["layout"]["files"]}
            assert "{unitdir}/agent-sec-core.service" in targets
            assert "/usr/lib/sysusers.d/agent-sec-core.conf" in targets
            assert not any(
                "userunitdir" in target or "python3.11" in target for target in targets
            )
    makefile = (ROOT / "Makefile").read_text()
    for target in ("build-all-v2", "install-all-for-rpmbuild-v2"):
        assert f"{target}: COMPONENT_MANIFEST := .anolisa/component-v2.toml" in makefile
