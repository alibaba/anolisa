
%define anolis_release 1
%global debug_package %{nil}

# Preserve original shebang (#!/usr/bin/env bash) for cross-platform compatibility
%undefine __brp_mangle_shebangs

Name:           agent-sec-core
Version:        0.3.0
Release:        %{anolis_release}%{?dist}
Summary:        Agent Security Core Package (metapackage)

License:        Apache-2.0
URL:            https://github.com/alibaba/anolisa
Source0:        %{name}-%{version}.tar.gz

# Build dependencies
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  rust >= 1.70
BuildRequires:  cargo
BuildRequires:  python3-devel
BuildRequires:  python3-pip
BuildRequires:  nodejs
BuildRequires:  npm

# Metapackage: pull all subpackages
Requires:       agent-sec-cli = %{version}-%{release}
Requires:       agent-sec-cosh-hook = %{version}-%{release}
Requires:       agent-sec-openclaw-hook = %{version}-%{release}
Requires:       agent-sec-skills = %{version}-%{release}

%description
Agent-Sec-Core is an OS-level security baseline and hardening framework for AI Agents.
This metapackage installs all agent-sec-core components including CLI, hooks, and skills.

# =============================================================================
# Subpackage 1: agent-sec-cli
# =============================================================================
%package -n agent-sec-cli
Summary:        Agent Security CLI tool (Rust + Python native extension)
Requires:       python3 >= 3.11
Requires:       python3 < 3.12

%description -n agent-sec-cli
Agent-sec-cli provides security scanning and hardening CLI commands.
Built with maturin as a Rust native Python extension.

%files -n agent-sec-cli
%defattr(0644,root,root,0755)
%attr(0755,root,root) %{_bindir}/agent-sec-cli
%{python3_sitearch}/agent_sec_cli/
%{python3_sitearch}/agent_sec_cli-*.dist-info/

# =============================================================================
# Subpackage 2: agent-sec-cosh-hook
# =============================================================================
%package -n agent-sec-cosh-hook
Summary:        CoPilot Shell security hooks with linux-sandbox
Requires:       agent-sec-cli = %{version}-%{release}
Requires:       bubblewrap
Requires:       python3 >= 3.11
Requires:       python3 < 3.12

%description -n agent-sec-cosh-hook
Provides code_scanner_hook and linux-sandbox for copilot-shell integration.
The linux-sandbox binary provides secure sandboxed execution environments.

%files -n agent-sec-cosh-hook
%defattr(0644,root,root,0755)
%attr(0755,root,root) /usr/local/bin/linux-sandbox
/usr/local/lib/anolisa/cosh_hooks/

# =============================================================================
# Subpackage 3: agent-sec-openclaw-hook
# =============================================================================
%package -n agent-sec-openclaw-hook
Summary:        OpenClaw plugin for agent security
Requires:       agent-sec-cli = %{version}-%{release}
Requires:       nodejs >= 18

%description -n agent-sec-openclaw-hook
OpenClaw IDE plugin providing agent security integration.
Hooks into OpenClaw to perform code scanning before tool execution.

%files -n agent-sec-openclaw-hook
%defattr(0644,root,root,0755)
%attr(0755,root,root) /opt/agent-sec/openclaw-plugin/scripts/deploy.sh
/opt/agent-sec/openclaw-plugin/

# =============================================================================
# Subpackage 4: agent-sec-skills
# =============================================================================
%package -n agent-sec-skills
Summary:        Agent security skill definitions for copilot-shell
Requires:       agent-sec-cli = %{version}-%{release}

%description -n agent-sec-skills
Skill reference files and documentation for copilot-shell integration.
Includes agent-sec-core and code-scanner skill definitions.

%files -n agent-sec-skills
%defattr(0644,root,root,0755)
%{_datadir}/anolisa/skills/

# =============================================================================
# Main package has no files (metapackage)
# =============================================================================
%files

# =============================================================================
# Build & Install
# =============================================================================
%prep
%setup -q

%build
make build-all

%install
rm -rf $RPM_BUILD_ROOT
make install-all DESTDIR=$RPM_BUILD_ROOT

%changelog
* Tue Apr 14 2026 Xingdong Li <XingDong.Li@linux.alibaba.com> - 0.3.0-1
- Switch agent-sec-cli build to maturin for Rust native extension support
- Add python3-devel and python3-pip BuildRequires for maturin wheel building
- Install agent-sec-cli as proper Python wheel with native .so extension
- Remove legacy script copy to skill directory (now handled by pip install)

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
