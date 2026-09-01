use asc_foundation_types::Revision;
use rusqlite::{Connection, Transaction, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedListError {
    Database,
    ItemTooLarge,
}

pub(crate) fn count(connection: &Connection, table: &str) -> Result<u64, ()> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let value: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|_| ())?;
    u64::try_from(value).map_err(|_| ())
}

pub(crate) fn append_audit(
    transaction: &Transaction<'_>,
    method: &str,
    resource_id: &str,
    operation_id: Option<&str>,
    outcome: &str,
) -> Result<(), ()> {
    transaction
        .execute(
            "INSERT INTO policy_admin_audit
             (principal, method, resource_id, operation_id, outcome)
             VALUES ('policy-admin-token', ?1, ?2, ?3, ?4)",
            params![method, resource_id, operation_id, outcome],
        )
        .map_err(|_| ())?;
    Ok(())
}

pub(crate) fn serialize<T: serde::Serialize>(value: &T) -> Result<String, ()> {
    serde_json::to_string(value).map_err(|_| ())
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(json: &str) -> rusqlite::Result<T> {
    serde_json::from_str(json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            json.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

pub(crate) fn list_json_bounded<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    limit: u32,
    offset: u64,
    max_item_bytes: usize,
) -> Result<Vec<T>, BoundedListError> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| BoundedListError::Database)?;
    let mut rows = statement
        .query(params![i64::from(limit), offset])
        .map_err(|_| BoundedListError::Database)?;
    let mut items = Vec::new();
    let mut used_bytes = 0_usize;
    while let Some(row) = rows.next().map_err(|_| BoundedListError::Database)? {
        let json_bytes = row
            .get::<_, i64>(0)
            .map_err(|_| BoundedListError::Database)
            .and_then(|value| usize::try_from(value).map_err(|_| BoundedListError::Database))?;
        let separator_bytes = usize::from(!items.is_empty());
        let item_bytes = json_bytes
            .checked_add(separator_bytes)
            .ok_or(BoundedListError::ItemTooLarge)?;
        if item_bytes > max_item_bytes.saturating_sub(used_bytes) {
            if items.is_empty() {
                return Err(BoundedListError::ItemTooLarge);
            }
            break;
        }
        let json: String = row.get(1).map_err(|_| BoundedListError::Database)?;
        items.push(decode(&json).map_err(|_| BoundedListError::Database)?);
        used_bytes += item_bytes;
    }
    Ok(items)
}

pub(crate) fn to_i64(revision: Revision) -> Result<i64, ()> {
    i64::try_from(revision.get()).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_SQL: &str = "SELECT length(CAST(record_json AS BLOB)), record_json
         FROM records ORDER BY rowid LIMIT ?1 OFFSET ?2";

    fn records(values: &[&str]) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute("CREATE TABLE records(record_json TEXT NOT NULL)", [])
            .unwrap();
        for value in values {
            connection
                .execute("INSERT INTO records(record_json) VALUES (?1)", [value])
                .unwrap();
        }
        connection
    }

    #[test]
    fn bounded_list_returns_a_partial_page_before_decoding_the_next_record() {
        let first = r#"{"id":1}"#;
        let connection = records(&[first, r#"{"id":2}"#, r#"{"id":3}"#]);

        let page =
            list_json_bounded::<serde_json::Value>(&connection, LIST_SQL, 100, 0, first.len())
                .unwrap();

        assert_eq!(page, vec![serde_json::json!({"id": 1})]);
    }

    #[test]
    fn bounded_list_rejects_an_oversized_first_record_without_decoding_it() {
        let connection = records(&["this intentionally is not JSON"]);

        let error =
            list_json_bounded::<serde_json::Value>(&connection, LIST_SQL, 100, 0, 1).unwrap_err();

        assert_eq!(error, BoundedListError::ItemTooLarge);
    }
}
