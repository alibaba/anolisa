use std::sync::Arc;

use agentsight_audit::{AuditService, AuditStore};

#[test]
fn service_accepts_an_independent_store() {
    let store = Arc::new(AuditStore::open_in_memory().expect("in-memory audit store should open"));
    let service = AuditService::new(Arc::clone(&store));

    assert!(Arc::ptr_eq(service.store(), &store));
}

#[test]
fn service_exposes_retention_cleanup_for_server_scheduling() {
    let store = Arc::new(AuditStore::open_in_memory().expect("in-memory audit store should open"));
    let service = AuditService::new(store);

    assert_eq!(service.purge_before(10).expect("purge should run"), 0);
}
