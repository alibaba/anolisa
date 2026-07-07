use crate::InMemoryStore;

#[test]
fn is_empty_default_trait_method() {
    let store = InMemoryStore::new();
    assert!(store.is_empty());
    store.stash("payload").unwrap();
    assert!(!store.is_empty());
}

#[test]
fn stash_error_display() {
    let e = StashError::Backend("test error".to_string());
    assert!(format!("{}", e).contains("test error"));
}
