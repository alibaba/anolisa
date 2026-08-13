//! Decision cache (§6.4): caches only Tier B pure sub-results and PIP
//! attributes, never final verdicts. Keys embed policy revision epoch and
//! adjudicator state versions so revision switches invalidate wholesale.
