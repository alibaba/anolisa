//! Policy enforcement adapter layer (design doc §8): `EnforcementAdapter`
//! trait, the obligation dispatcher, the five built-in adapters
//! (DaemonInline/Harness/Sandbox/Fuse/Approval, Table 9) and the HMAC
//! `TokenAuthority` implementation for the STEP_UP/DEFER closed loop.
//!
//! Enforcement is deny-biased: failure to fulfil a prerequisite obligation
//! (StepUp/KernelRule/Quarantine) is treated as Deny; advisory obligation
//! failures only spool and alert. Receipts always flow back to form the
//! decision → enforcement evidence chain.

pub mod adapter;
pub mod dispatcher;
pub mod token_authority;
