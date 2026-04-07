
%define anolis_release 1
%global debug_package %{nil}

# Preserve original shebang (#!/usr/bin/env bash) for cross-platform compatibility
%undefine __brp_mangle_shebangs

Name:           agent-sec-core
Version:        0.0.8
Release:        %{anolis_release}%{?dist}
Summary:        Agent Security Core Package

License:        Apache-2.0
URL:            https://github.com/alibaba/anolisa
Source0:        %{name}-%{version}.tar.gz

# Build dependencies
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  rust >= 1.70
BuildRequires:  cargo

# Runtime dependencies
# asset-verify
Requires:       python3 >= 3.6
Requires:       gnupg2 >= 2.0
Requires:       jq
Recommends:     python3-pgpy >= 0.5

# sandbox
Requires:       bubblewrap

# seharden
# Requires:       loongshield >= 1.1.1


%description
Agent-Sec-Core is an OS-level security baseline and hardening framework for AI Agents.
It provides system hardening, sandbox isolation, and asset integrity verification,
suitable for AI Agent platforms such as Agent OS and OpenClaw.

%prep
%setup -q

%build

# Build linux-sandbox
# Note: rust-toolchain.toml version compatibility is handled by rpm-build.sh
make build-sandbox

%install
rm -rf $RPM_BUILD_ROOT
install -d -m 0755 %{buildroot}/usr/local/bin
install -d -m 0755 $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/scripts
install -d -m 0755 $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/references

# Install linux-sandbox binary
install -p -m 0755 linux-sandbox/target/release/linux-sandbox %{buildroot}/usr/local/bin/

# Install sign-skill.sh tool
install -p -m 0755 tools/sign-skill.sh %{buildroot}/usr/local/bin/

# Install scripts
cp -rp agent-sec-cli/* $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/scripts/

# Install references files
cp -rp skill/references/* $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/references/

# Install documentation
cp skill/SKILL.md $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core/

# Set permissions for executable scripts
find $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core -type f -name '*.sh' -exec chmod 0755 {} +
find $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core -type f -name '*.py' -exec chmod 0755 {} +

# Set permissions for regular files
find $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core -type f \
    ! -name '*.sh' ! -name '*.py' -exec chmod 0644 {} +

# Set permissions for directories
find $RPM_BUILD_ROOT%{_datadir}/anolisa/skills/agent-sec-core -type d -exec chmod 0755 {} +

%files
%defattr(0644,root,root,0755)
%attr(0755,root,root) /usr/local/bin/linux-sandbox
%attr(0755,root,root) /usr/local/bin/sign-skill.sh
%attr(0755,root,root) %{_datadir}/anolisa/skills/agent-sec-core/scripts/*/*.py
%{_datadir}/anolisa/skills/agent-sec-core/

%changelog
* Mon Mar 23 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.8-1
- Disable brp-mangle-shebangs to preserve #!/usr/bin/env bash for cross-platform compatibility

* Fri Mar 20 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.7-1
- Add defattr and attr in files section for permission protection
- Fix install section: add explicit permission settings for directories and files
- Use install -d -m 0755 instead of mkdir -p for deterministic permissions
- Set executable permissions (0755) for .sh and .py scripts
- Set read-only permissions (0644) for other files
- Add build-time skill signing using sign-skill.sh

* Fri Mar 20 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.6-1
- Change skill install path to /usr/share/anolisa/skills/agent-sec-core

* Thu Mar 19 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.5-1
- Add linux-sandbox module

* Thu Mar 19 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.4-1
- Refactor: move test files from scripts/asset-verify/test/ to /tests/
- scripts/ directory now contains only production files
- Add asset-verify dependencies version (python3 >= 3.6, gnupg2 >= 2.0, python3-pgpy >= 0.5)

* Tue Mar 17 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.3-1
- Fix spec install section: use cp -r for recursive directory copy
- Add asset-verify dependencies (python3, gnupg2, python3-pgpy)

* Mon Mar 16 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.2-1
- Add loongshield security hardening capability

* Fri Mar 13 2026 YiZheng Yang <YiZheng.Yang@linux.alibaba.com> - 0.0.1-1
- Initial package
