//! asc-policyd: sidecar daemon hosting the Policy Engine Framework (design
//! doc §11.3). Listens on `policy.sock` (0700, peer-credential checked) next
//! to the Python daemon socket, speaking NDJSON frames. This is the only
//! crate that owns a tokio runtime; every framework trait object (PDP, PIP
//! providers, PEP adapters, PolicyStore) is assembled here from config.

fn main() -> anyhow::Result<()> {
    // P0 bootstrap: UDS server, event bus, PolicyStore recovery and trait
    // object assembly land with the first framework milestone (Table 16).
    Ok(())
}
