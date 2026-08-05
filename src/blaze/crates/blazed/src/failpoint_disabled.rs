// SPDX-License-Identifier: Apache-2.0
//! No-op hooks used when daemon verification support is disabled.

#![allow(dead_code)] // Call sites land with their owning lifecycle commits.

/// Keep daemon startup independent from verification-only configuration.
pub(crate) fn announce() {}

/// Leave backend operations unchanged in production builds.
pub(crate) fn backend(_name: &str) -> blaze_core::Result<()> {
    Ok(())
}

/// Leave storage operations unchanged in production builds.
pub(crate) fn storage(_name: &str) -> blaze_core::Result<()> {
    Ok(())
}

/// Leave state commits unchanged in production builds.
pub(crate) fn state(_name: &str) -> crate::error::Result<()> {
    Ok(())
}

/// Never pause production requests.
pub(crate) async fn pause(_name: &str) {}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn production_hooks_are_inert() {
        super::announce();
        super::backend("any").expect("backend hook");
        super::storage("any").expect("storage hook");
        super::state("any").expect("state hook");
        super::pause("any").await;
    }
}
