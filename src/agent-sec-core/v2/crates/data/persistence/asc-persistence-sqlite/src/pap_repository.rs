use asc_foundation_types::{ResourceId, Revision};
use asc_pap::{
    Page, PapError, PapRepository, PolicyRevisionState, PreparedPolicy, PreparedScope,
    ScopeRevisionState,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::sql::{
    BoundedListError, append_audit, count, decode, list_json_bounded, serialize, to_i64,
};
use crate::store::SqlitePolicyStore;

impl PapRepository for SqlitePolicyStore {
    fn put_policy(&self, policy: &PreparedPolicy) -> Result<PreparedPolicy, PapError> {
        let mut connection = self.connection().map_err(|()| PapError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| PapError::Persistence)?;
        if let Some(existing) = select_policy(&transaction, &policy.policy_id, policy.revision)? {
            if existing == *policy {
                return Ok(existing);
            }
            return Err(PapError::Conflict);
        }
        let revision = to_i64(policy.revision).map_err(|()| PapError::Persistence)?;
        let head: Option<i64> = transaction
            .query_row(
                "SELECT last_allocated_revision FROM policy_revision_heads WHERE policy_id = ?1",
                [policy.policy_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PapError::Persistence)?;
        match head {
            None if policy.revision.get() == 1 => {
                transaction
                    .execute(
                        "INSERT INTO policy_revision_heads(policy_id, last_allocated_revision)
                         VALUES (?1, ?2)",
                        params![policy.policy_id.as_str(), revision],
                    )
                    .map_err(|_| PapError::Conflict)?;
            }
            Some(current) if current.checked_add(1) == Some(revision) => {
                let updated = transaction
                    .execute(
                        "UPDATE policy_revision_heads SET last_allocated_revision = ?2
                         WHERE policy_id = ?1 AND last_allocated_revision = ?3",
                        params![policy.policy_id.as_str(), revision, current],
                    )
                    .map_err(|_| PapError::Persistence)?;
                if updated != 1 {
                    return Err(PapError::Conflict);
                }
            }
            _ => return Err(PapError::Conflict),
        }
        let json = serialize(policy).map_err(|()| PapError::Persistence)?;
        transaction
            .execute(
                "INSERT INTO policy_revisions
                 (policy_id, revision, template_digest, record_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    policy.policy_id.as_str(),
                    revision,
                    policy.template_digest,
                    json
                ],
            )
            .map_err(|_| PapError::Persistence)?;
        append_audit(
            &transaction,
            "policy.templates.put",
            policy.policy_id.as_str(),
            None,
            "stored",
        )
        .map_err(|()| PapError::Persistence)?;
        transaction.commit().map_err(|_| PapError::Persistence)?;
        Ok(policy.clone())
    }

    fn get_policy_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<PolicyRevisionState>, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        let last_allocated: Option<i64> = connection
            .query_row(
                "SELECT last_allocated_revision FROM policy_revision_heads WHERE policy_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PapError::Persistence)?;
        let Some(last_allocated) = last_allocated else {
            return Ok(None);
        };
        let last_allocated_revision =
            Revision::new(u64::try_from(last_allocated).map_err(|_| PapError::Persistence)?)
                .map_err(|_| PapError::Persistence)?;
        Ok(Some(PolicyRevisionState {
            last_allocated_revision,
            latest: select_latest_policy(&connection, id)?,
        }))
    }

    fn get_policy(&self, id: &ResourceId, revision: Revision) -> Result<PreparedPolicy, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        select_policy(&connection, id, revision)?.ok_or(PapError::NotFound)
    }

    fn list_policies(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedPolicy>, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        let total = count(&connection, "policy_revisions").map_err(|()| PapError::Persistence)?;
        let items = list_json_bounded(
            &connection,
            "SELECT length(CAST(record_json AS BLOB)), record_json
             FROM policy_revisions
             ORDER BY policy_id, revision LIMIT ?1 OFFSET ?2",
            limit,
            offset,
            max_item_bytes,
        )
        .map_err(pap_list_error)?;
        Ok(Page { items, total })
    }

    fn delete_policy_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedPolicy, PapError> {
        let mut connection = self.connection().map_err(|()| PapError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| PapError::Persistence)?;
        let policy = select_policy(&transaction, id, revision)?.ok_or(PapError::NotFound)?;
        transaction
            .execute(
                "DELETE FROM policy_revisions WHERE policy_id = ?1 AND revision = ?2",
                params![
                    id.as_str(),
                    to_i64(revision).map_err(|()| PapError::Persistence)?
                ],
            )
            .map_err(|_| PapError::Persistence)?;
        append_audit(
            &transaction,
            "policy.templates.delete",
            id.as_str(),
            None,
            "deleted",
        )
        .map_err(|()| PapError::Persistence)?;
        transaction.commit().map_err(|_| PapError::Persistence)?;
        Ok(policy)
    }

    fn put_scope(&self, scope: &PreparedScope) -> Result<PreparedScope, PapError> {
        let mut connection = self.connection().map_err(|()| PapError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| PapError::Persistence)?;
        if let Some(existing) = select_scope(&transaction, &scope.scope_id, scope.revision)? {
            if existing == *scope {
                return Ok(existing);
            }
            return Err(PapError::Conflict);
        }
        let revision = to_i64(scope.revision).map_err(|()| PapError::Persistence)?;
        let head: Option<i64> = transaction
            .query_row(
                "SELECT last_allocated_revision FROM scope_revision_heads WHERE scope_id = ?1",
                [scope.scope_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PapError::Persistence)?;
        match head {
            None if scope.revision.get() == 1 => {
                transaction
                    .execute(
                        "INSERT INTO scope_revision_heads(scope_id, last_allocated_revision)
                         VALUES (?1, ?2)",
                        params![scope.scope_id.as_str(), revision],
                    )
                    .map_err(|_| PapError::Conflict)?;
            }
            Some(current) if current.checked_add(1) == Some(revision) => {
                let updated = transaction
                    .execute(
                        "UPDATE scope_revision_heads SET last_allocated_revision = ?2
                         WHERE scope_id = ?1 AND last_allocated_revision = ?3",
                        params![scope.scope_id.as_str(), revision, current],
                    )
                    .map_err(|_| PapError::Persistence)?;
                if updated != 1 {
                    return Err(PapError::Conflict);
                }
            }
            _ => return Err(PapError::Conflict),
        }
        transaction
            .execute(
                "INSERT INTO scope_revisions
                 (scope_id, revision, template_digest, record_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    scope.scope_id.as_str(),
                    revision,
                    scope.template_digest,
                    serialize(scope).map_err(|()| PapError::Persistence)?
                ],
            )
            .map_err(|_| PapError::Persistence)?;
        append_audit(
            &transaction,
            "policy.scopes.put",
            scope.scope_id.as_str(),
            None,
            "stored",
        )
        .map_err(|()| PapError::Persistence)?;
        transaction.commit().map_err(|_| PapError::Persistence)?;
        Ok(scope.clone())
    }

    fn get_scope_revision_state(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ScopeRevisionState>, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        let last_allocated: Option<i64> = connection
            .query_row(
                "SELECT last_allocated_revision FROM scope_revision_heads WHERE scope_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PapError::Persistence)?;
        let Some(last_allocated) = last_allocated else {
            return Ok(None);
        };
        let last_allocated_revision =
            Revision::new(u64::try_from(last_allocated).map_err(|_| PapError::Persistence)?)
                .map_err(|_| PapError::Persistence)?;
        Ok(Some(ScopeRevisionState {
            last_allocated_revision,
            latest: select_latest_scope(&connection, id)?,
        }))
    }

    fn get_scope(&self, id: &ResourceId, revision: Revision) -> Result<PreparedScope, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        select_scope(&connection, id, revision)?.ok_or(PapError::NotFound)
    }

    fn list_scopes(
        &self,
        limit: u32,
        offset: u64,
        max_item_bytes: usize,
    ) -> Result<Page<PreparedScope>, PapError> {
        let connection = self.connection().map_err(|()| PapError::Persistence)?;
        let total = count(&connection, "scope_revisions").map_err(|()| PapError::Persistence)?;
        let items = list_json_bounded(
            &connection,
            "SELECT length(CAST(record_json AS BLOB)), record_json
             FROM scope_revisions
             ORDER BY scope_id, revision LIMIT ?1 OFFSET ?2",
            limit,
            offset,
            max_item_bytes,
        )
        .map_err(pap_list_error)?;
        Ok(Page { items, total })
    }

    fn delete_scope_revision(
        &self,
        id: &ResourceId,
        revision: Revision,
    ) -> Result<PreparedScope, PapError> {
        let mut connection = self.connection().map_err(|()| PapError::Persistence)?;
        let transaction = connection
            .transaction()
            .map_err(|_| PapError::Persistence)?;
        let scope = select_scope(&transaction, id, revision)?.ok_or(PapError::NotFound)?;
        transaction
            .execute(
                "DELETE FROM scope_revisions WHERE scope_id = ?1 AND revision = ?2",
                params![
                    id.as_str(),
                    to_i64(revision).map_err(|()| PapError::Persistence)?
                ],
            )
            .map_err(|_| PapError::Persistence)?;
        append_audit(
            &transaction,
            "policy.scopes.delete",
            id.as_str(),
            None,
            "deleted",
        )
        .map_err(|()| PapError::Persistence)?;
        transaction.commit().map_err(|_| PapError::Persistence)?;
        Ok(scope)
    }
}

const fn pap_list_error(error: BoundedListError) -> PapError {
    match error {
        BoundedListError::Database => PapError::Persistence,
        BoundedListError::ItemTooLarge => PapError::ResponseTooLarge,
    }
}

fn select_policy(
    connection: &Connection,
    id: &ResourceId,
    revision: Revision,
) -> Result<Option<PreparedPolicy>, PapError> {
    connection
        .query_row(
            "SELECT record_json FROM policy_revisions
             WHERE policy_id = ?1 AND revision = ?2",
            params![
                id.as_str(),
                to_i64(revision).map_err(|()| PapError::Persistence)?
            ],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()
        .map_err(|_| PapError::Persistence)
}

fn select_latest_policy(
    connection: &Connection,
    id: &ResourceId,
) -> Result<Option<PreparedPolicy>, PapError> {
    connection
        .query_row(
            "SELECT record_json FROM policy_revisions
             WHERE policy_id = ?1 ORDER BY revision DESC LIMIT 1",
            params![id.as_str()],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()
        .map_err(|_| PapError::Persistence)
}

fn select_scope(
    connection: &Connection,
    id: &ResourceId,
    revision: Revision,
) -> Result<Option<PreparedScope>, PapError> {
    connection
        .query_row(
            "SELECT record_json FROM scope_revisions
             WHERE scope_id = ?1 AND revision = ?2",
            params![
                id.as_str(),
                to_i64(revision).map_err(|()| PapError::Persistence)?
            ],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()
        .map_err(|_| PapError::Persistence)
}

fn select_latest_scope(
    connection: &Connection,
    id: &ResourceId,
) -> Result<Option<PreparedScope>, PapError> {
    connection
        .query_row(
            "SELECT record_json FROM scope_revisions
             WHERE scope_id = ?1 ORDER BY revision DESC LIMIT 1",
            params![id.as_str()],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()
        .map_err(|_| PapError::Persistence)
}
