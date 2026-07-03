//! DNF command-line options for an explicit RPM repository source.

/// DNF repository supplied by ANOLISA configuration for one command run.
///
/// The repo is injected with `--repofrompath` instead of writing a repo file,
/// keeping `repo.toml` authoritative for ANOLISA-managed RPM operations while
/// leaving the host's persistent package-manager configuration untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnfRepoSource {
    id: String,
    base_url: String,
    gpgcheck: Option<bool>,
}

impl DnfRepoSource {
    /// Builds a temporary DNF repository descriptor.
    pub fn new(id: impl Into<String>, base_url: impl Into<String>, gpgcheck: Option<bool>) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into(),
            gpgcheck,
        }
    }

    /// Repository id used by DNF for this temporary source.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Base URL passed to DNF as the repo path.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Package signature verification setting from `repo.toml`.
    pub fn gpgcheck(&self) -> Option<bool> {
        self.gpgcheck
    }

    /// DNF options for **read-only queries** (e.g. `repoquery`).
    ///
    /// Disables all host repos (`--disablerepo=*`) so availability probes only
    /// report packages from the ANOLISA-configured repo, never silently falling
    /// back to a system repo that happens to carry a same-named package.
    pub(crate) fn append_dnf_options(&self, args: &mut Vec<String>) {
        args.push("--disablerepo=*".to_string());
        args.push(format!("--repofrompath={},{}", self.id, self.base_url));
        args.push(format!("--enablerepo={}", self.id));
        if let Some(gpgcheck) = self.gpgcheck {
            args.push(format!(
                "--setopt={}.gpgcheck={}",
                self.id,
                if gpgcheck { "1" } else { "0" }
            ));
        }
    }

    /// DNF options for **write transactions** (`install`/`update`/`remove`).
    ///
    /// Unlike [`append_dnf_options`](Self::append_dnf_options), this does **not**
    /// emit `--disablerepo=*`. RPM packages declare their own `Requires:` and dnf
    /// resolves the entire dependency graph in one transaction. If all system
    /// repos are disabled, dnf cannot satisfy cross-repo dependencies that live
    /// outside the ANOLISA repo (e.g. `bubblewrap` in EPEL). Keeping system repos
    /// enabled lets dnf pull ANOLISA components from the configured repo while
    /// still resolving system-level `Requires` from the host's enabled repos.
    pub(crate) fn append_dnf_txn_options(&self, args: &mut Vec<String>) {
        args.push(format!("--repofrompath={},{}", self.id, self.base_url));
        args.push(format!("--enablerepo={}", self.id));
        if let Some(gpgcheck) = self.gpgcheck {
            args.push(format!(
                "--setopt={}.gpgcheck={}",
                self.id,
                if gpgcheck { "1" } else { "0" }
            ));
        }
    }
}
