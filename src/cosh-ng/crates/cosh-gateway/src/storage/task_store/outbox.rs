impl SqliteTaskStore {

    /// Claims the oldest ready delivery of one kind in an immediate transaction.
    ///
    /// Expired claims are eligible for takeover. The incremented attempt is a
    /// fencing token, so a stale worker cannot acknowledge a later claim even
    /// when the same worker identity is reused.
    ///
    /// # Errors
    ///
    /// Returns a validation, corruption, or SQLite transaction error.
    pub fn claim_outbox(
        &mut self,
        delivery_kind: &BoundedName,
        lease_owner: &BoundedOpaque,
        now_ms: u64,
        lease_expires_at_ms: u64,
    ) -> Result<Option<OutboxClaim>, StoreError> {
        if lease_expires_at_ms <= now_ms {
            return Err(invalid("Outbox lease deadline must be in the future"));
        }
        let now = sqlite_integer(now_ms, "Outbox claim timestamp")?;
        let lease_deadline = sqlite_integer(lease_expires_at_ms, "Outbox lease deadline")?;
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = transaction
            .query_row(
                "SELECT delivery_id, task_id, event_id, payload_json, attempt
                 FROM outbox
                 WHERE delivery_kind = ?1 AND next_attempt_at_ms <= ?2
                   AND (state = 'pending'
                        OR (state = 'leased' AND lease_expires_at_ms <= ?2))
                 ORDER BY next_attempt_at_ms, created_at_ms, delivery_id
                 LIMIT 1",
                params![delivery_kind.as_str(), now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((delivery_id, task_id, event_id, payload_json, attempt)) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };
        let next_attempt = attempt
            .checked_add(1)
            .ok_or_else(|| corrupt("Outbox attempt overflow"))?;
        let changed = transaction.execute(
            "UPDATE outbox
             SET state='leased', attempt=?2, lease_owner=?3, lease_expires_at_ms=?4
             WHERE delivery_id=?1 AND attempt=?5
               AND (state='pending' OR (state='leased' AND lease_expires_at_ms <= ?6))",
            params![
                delivery_id,
                next_attempt,
                lease_owner.as_str(),
                lease_deadline,
                attempt,
                now,
            ],
        )?;
        if changed != 1 {
            return Err(corrupt(
                "Outbox claim lost its immediate-transaction precondition",
            ));
        }
        let claim = OutboxClaim {
            delivery_id: DeliveryId::parse(&delivery_id)
                .map_err(|error| corrupt(&format!("invalid Outbox delivery identity: {error}")))?,
            task_id: TaskId::parse(&task_id)
                .map_err(|error| corrupt(&format!("invalid Outbox Task identity: {error}")))?,
            event_id: MessageId::parse(&event_id)
                .map_err(|error| corrupt(&format!("invalid Outbox event identity: {error}")))?,
            delivery_kind: delivery_kind.clone(),
            payload: serde_json::from_str(&payload_json)?,
            attempt: u64::try_from(next_attempt).map_err(|_| corrupt("negative Outbox attempt"))?,
            lease_owner: lease_owner.clone(),
            lease_expires_at_ms,
        };
        transaction.commit()?;
        Ok(Some(claim))
    }

    /// Marks an exact, unexpired Outbox claim delivered.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::GenerationFenced`] for a stale claim.
    pub fn complete_outbox(
        &mut self,
        claim: &OutboxClaim,
        completed_at_ms: u64,
    ) -> Result<(), StoreError> {
        let completed_at = sqlite_integer(completed_at_ms, "Outbox completion timestamp")?;
        let attempt = sqlite_integer(claim.attempt, "Outbox attempt")?;
        let changed = self.connection_mut().execute(
            "UPDATE outbox
             SET state='delivered', lease_owner=NULL, lease_expires_at_ms=NULL,
                 delivered_at_ms=?2
             WHERE delivery_id=?1 AND state='leased' AND attempt=?3
               AND lease_owner=?4 AND lease_expires_at_ms > ?2",
            params![
                claim.delivery_id.as_str(),
                completed_at,
                attempt,
                claim.lease_owner.as_str(),
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::GenerationFenced {
                expected: claim.attempt,
                actual: self.outbox_attempt(&claim.delivery_id)?.unwrap_or(0),
            })
        }
    }

    /// Releases an exact claim for a bounded retry after a failed delivery.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::GenerationFenced`] for a stale claim.
    pub fn retry_outbox(
        &mut self,
        claim: &OutboxClaim,
        released_at_ms: u64,
        next_attempt_at_ms: u64,
    ) -> Result<(), StoreError> {
        if next_attempt_at_ms < released_at_ms {
            return Err(invalid("Outbox retry cannot precede its release"));
        }
        let attempt = sqlite_integer(claim.attempt, "Outbox attempt")?;
        let released_at = sqlite_integer(released_at_ms, "Outbox release timestamp")?;
        let next_attempt = sqlite_integer(next_attempt_at_ms, "Outbox retry timestamp")?;
        let changed = self.connection_mut().execute(
            "UPDATE outbox
             SET state='pending', next_attempt_at_ms=?2,
                 lease_owner=NULL, lease_expires_at_ms=NULL
             WHERE delivery_id=?1 AND state='leased' AND attempt=?3 AND lease_owner=?4
               AND lease_expires_at_ms > ?5",
            params![
                claim.delivery_id.as_str(),
                next_attempt,
                attempt,
                claim.lease_owner.as_str(),
                released_at,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::GenerationFenced {
                expected: claim.attempt,
                actual: self.outbox_attempt(&claim.delivery_id)?.unwrap_or(0),
            })
        }
    }

    fn outbox_attempt(&self, delivery_id: &DeliveryId) -> Result<Option<u64>, StoreError> {
        let attempt = self
            .connection()
            .query_row(
                "SELECT attempt FROM outbox WHERE delivery_id=?1",
                params![delivery_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        attempt
            .map(|value| u64::try_from(value).map_err(|_| corrupt("negative Outbox attempt")))
            .transpose()
    }
}
