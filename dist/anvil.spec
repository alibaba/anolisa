Name:           anvil
Version:        0.2.0
Release:        1%{?dist}
Summary:        ANOLISA Anvil - Per-Host Sandbox Daemon
License:        Apache-2.0
URL:            https://github.com/alibaba/anolisa

# Rust binary, pre-built
Source0:        anvil-%{version}.tar.gz

BuildRequires:  rust >= 1.88
BuildRequires:  cargo

Requires:       /dev/kvm

%description
ANOLISA Anvil is a per-host sandbox daemon that manages sandbox instances
via HTTP/gRPC API. It supports multiple backends (Firecracker, linux-sandbox,
gVisor) and provides policy-driven workload routing and warm pool management.

%prep
%setup -q

%build
cd src/anvil
cargo build --release

%install
install -D -m 0755 src/anvil/target/release/anvil %{buildroot}/usr/libexec/anolisa/anvil
install -D -m 0644 dist/anvil.service %{buildroot}%{_unitdir}/anvil.service
install -D -m 0644 dist/tmpfiles-anvil.conf %{buildroot}%{_tmpfilesdir}/anvil.conf
install -D -m 0644 src/anvil/examples/config.toml %{buildroot}/etc/anolisa/anvil/config.toml
install -d %{buildroot}/etc/anolisa/anvil/policies
install -m 0644 src/anvil/examples/policies/*.toml %{buildroot}/etc/anolisa/anvil/policies/
install -d %{buildroot}/var/lib/anvil/{instances,templates,images}

%post
%systemd_post anvil.service
systemd-tmpfiles --create anvil.conf 2>/dev/null || :

%preun
%systemd_preun anvil.service

%postun
%systemd_postun_with_restart anvil.service

%files
/usr/libexec/anolisa/anvil
%{_unitdir}/anvil.service
%{_tmpfilesdir}/anvil.conf
%config(noreplace) /etc/anolisa/anvil/config.toml
%config(noreplace) /etc/anolisa/anvil/policies/*.toml
%dir /var/lib/anvil
%dir /var/lib/anvil/instances
%dir /var/lib/anvil/templates
%dir /var/lib/anvil/images
%dir /etc/anolisa/anvil
%dir /etc/anolisa/anvil/policies

%changelog
* Mon Jun 30 2026 ANOLISA Team <anolisa@alibaba-inc.com> - 0.2.0-1
- Initial package: daemon + Firecracker/LinuxSandbox/Mock backends
- HTTP API (UDS + TCP), policy engine, warm pool
