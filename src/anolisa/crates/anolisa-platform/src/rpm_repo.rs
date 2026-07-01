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
}
