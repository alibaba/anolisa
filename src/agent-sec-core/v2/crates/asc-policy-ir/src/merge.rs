//! Effect merge law (§3.5, Table 6): total order deny > step_up > defer >
//! modify > audit > allow, commutative and associative. Obligation union is
//! handled separately so it cannot break deny absorption: an unconditional
//! deny strips StepUp/Defer obligations contributed by other rules (runtime
//! assembly in asc-pdp). These algebraic properties are proptest targets.

use crate::model::EffectIr;

/// Merges hit effects under the Table 6 total order; commutative and
/// associative by construction (max over a total order). With no hits the
/// domain default applies: capability domains default to deny, detection
/// domains to pass, authorization to permit-with-forbid-override.
pub fn merge_effects<I>(domain_default: EffectIr, hits: I) -> EffectIr
where
    I: IntoIterator<Item = EffectIr>,
{
    hits.into_iter().max().unwrap_or(domain_default)
}
