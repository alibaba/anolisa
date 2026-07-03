# Package Management

The `cosh-cli pkg` subsystem provides cross-distribution package management operations. It automatically detects the current distribution and routes to the corresponding package manager backend (dnf / apt-get / zypper / brew).

## Command List

| Command | Description |
|---------|-------------|
| `cosh-cli pkg install <name>` | Install package |
| `cosh-cli pkg remove <name>` | Remove package |
| `cosh-cli pkg search <query>` | Search packages |
| `cosh-cli pkg list --installed` | List installed packages |

## install

Install a specified package.

```bash
cosh-cli pkg install nginx
cosh-cli pkg install nginx --dry-run
```

Success output:

```json
{
  "ok": true,
  "data": {
    "package": "nginx",
    "version": "1.24.0",
    "already_installed": false,
    "dependencies_installed": []
  },
  "meta": { "subsystem": "pkg", "duration_ms": 5200, "distro": "alinux", "dry_run": false }
}
```

If the package is already installed, `already_installed` is `true` and the command still returns success.

## remove

Remove a specified package.

```bash
cosh-cli pkg remove nginx
cosh-cli pkg remove nginx --dry-run
```

Success output:

```json
{
  "ok": true,
  "data": {
    "package": "nginx",
    "version_removed": "1.24.0",
    "dependencies_removed": []
  },
  "meta": { "subsystem": "pkg", "duration_ms": 2100, "distro": "ubuntu", "dry_run": false }
}
```

## search

Search available packages. Results include installation status.

```bash
cosh-cli pkg search "web server"
```

Output:

```json
{
  "ok": true,
  "data": {
    "packages": [
      { "name": "nginx", "version": "1.24.0", "description": "...", "installed": true },
      { "name": "apache2", "version": "2.4.58", "description": "...", "installed": false }
    ],
    "total": 2
  },
  "meta": { "subsystem": "pkg", "duration_ms": 800, "distro": "ubuntu", "dry_run": false }
}
```

## list

List installed packages.

```bash
cosh-cli pkg list --installed
```

Output:

```json
{
  "ok": true,
  "data": {
    "packages": [
      { "name": "nginx", "version": "1.24.0" },
      { "name": "curl", "version": "8.5.0" }
    ],
    "total": 2
  },
  "meta": { "subsystem": "pkg", "duration_ms": 300, "distro": "centos", "dry_run": false }
}
```

## Backend Mapping

| Operation | dnf | apt-get | zypper | brew |
|-----------|-----|---------|--------|------|
| install | `dnf install -y` | `apt-get install -y` | `zypper install -y` | `brew install` |
| remove | `dnf remove -y` | `apt-get remove -y` | `zypper remove -y` | `brew uninstall` |
| search | `dnf search -q` | `apt-cache search` | `zypper search` | `brew search` |
| list | `dnf list installed -q` | `dpkg-query -W` | `zypper se --installed-only` | `brew list --versions` |

## Error Handling

- Package not found returns `PkgNotFound`, `hint` suggests searching
- Package manager execution failure returns `PkgBackendError`, `recoverable: true`
- Unsupported distribution returns `UnsupportedDistro`
