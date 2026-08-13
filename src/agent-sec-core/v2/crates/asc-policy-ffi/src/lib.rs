//! pyo3 bindings exposing a LocalPolicy-style API to the Python daemon
//! (design doc §11.3, fallback path). The sidecar asc-policyd is the
//! committed V2 integration form; this crate is only activated if the
//! sidecar deployment is vetoed by operations, and is not published in P0.
//! The pyo3 dependency is intentionally deferred until this path is enabled.
