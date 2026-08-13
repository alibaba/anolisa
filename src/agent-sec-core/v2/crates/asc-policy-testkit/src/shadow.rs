//! Shadow replay: re-adjudicates sampled production trajectories against a
//! staged revision and blocks commit when the decision diff exceeds the
//! configured threshold (§10.2).
