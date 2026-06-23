//! Token usage statistics and time-series query methods for GenAI SQLite store.

use rusqlite::params;

use super::GenAISqliteStore;

// ─── Query result types ────────────────────────────────────────────────────────

/// One data-point in a token time-series response
#[derive(Debug, serde::Serialize)]
pub struct TimeseriesBucket {
    pub bucket_start_ns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

/// One data-point in a per-model token time-series response
#[derive(Debug, serde::Serialize)]
pub struct ModelTimeseriesBucket {
    pub bucket_start_ns: i64,
    pub model: String,
    pub total_tokens: i64,
}

/// Per-agent token usage summary (all-time aggregation)
#[derive(Debug, serde::Serialize)]
pub struct AgentTokenSummary {
    pub agent_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub request_count: i64,
}

impl GenAISqliteStore {
    /// One bucket in a token time-series query.
    pub fn get_token_timeseries(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
        bucket_count: u32,
    ) -> Result<Vec<TimeseriesBucket>, Box<dyn std::error::Error>> {
        let bucket_count = bucket_count.max(1);
        let range_ns = (end_ns - start_ns).max(1);
        let bucket_ns = range_ns / bucket_count as i64;

        let conn = self.conn.lock().unwrap();

        // Build query with optional agent_name filter
        let sql = if agent_name.is_some() {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(SUM(input_tokens), 0)            AS input_tokens,
                COALESCE(SUM(output_tokens), 0)           AS output_tokens,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
               AND agent_name = ?4
             GROUP BY bucket_idx
             ORDER BY bucket_idx ASC"
        } else {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(SUM(input_tokens), 0)            AS input_tokens,
                COALESCE(SUM(output_tokens), 0)           AS output_tokens,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY bucket_idx
             ORDER BY bucket_idx ASC"
        };

        let rows: Vec<TimeseriesBucket> = if let Some(name) = agent_name {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns, name], |row| {
                Ok(TimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns], |row| {
                Ok(TimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    total_tokens: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    }

    /// Model-level token breakdown time-series.
    pub fn get_model_timeseries(
        &self,
        start_ns: i64,
        end_ns: i64,
        agent_name: Option<&str>,
        bucket_count: u32,
    ) -> Result<Vec<ModelTimeseriesBucket>, Box<dyn std::error::Error>> {
        let bucket_count = bucket_count.max(1);
        let range_ns = (end_ns - start_ns).max(1);
        let bucket_ns = range_ns / bucket_count as i64;

        let conn = self.conn.lock().unwrap();

        let sql = if agent_name.is_some() {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(model, 'unknown')                 AS model,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
               AND agent_name = ?4
             GROUP BY bucket_idx, model
             ORDER BY bucket_idx ASC"
        } else {
            "SELECT
                (start_timestamp_ns - ?1) / ?3            AS bucket_idx,
                ?1 + ((start_timestamp_ns - ?1) / ?3) * ?3 AS bucket_start_ns,
                COALESCE(model, 'unknown')                 AS model,
                COALESCE(SUM(total_tokens), 0)            AS total_tokens
             FROM genai_events
             WHERE event_type = 'llm_call'
               AND start_timestamp_ns BETWEEN ?1 AND ?2
             GROUP BY bucket_idx, model
             ORDER BY bucket_idx ASC"
        };

        let rows: Vec<ModelTimeseriesBucket> = if let Some(name) = agent_name {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns, name], |row| {
                Ok(ModelTimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    model: row.get(2)?,
                    total_tokens: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(sql)?;
            stmt.query_map(params![start_ns, end_ns, bucket_ns], |row| {
                Ok(ModelTimeseriesBucket {
                    bucket_start_ns: row.get(1)?,
                    model: row.get(2)?,
                    total_tokens: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    }

    /// Return per-agent token usage aggregated over all recorded history.
    ///
    /// Groups by `COALESCE(agent_name, process_name, 'unknown')` so that every
    /// LLM call is attributed to some label even when agent_name is NULL.
    pub fn get_agent_token_summary(
        &self,
    ) -> Result<Vec<AgentTokenSummary>, Box<dyn std::error::Error>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(agent_name, process_name, 'unknown') AS agent,
                    COALESCE(SUM(input_tokens),  0) AS input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(total_tokens),  0) AS total_tokens,
                    COUNT(*)                        AS request_count
             FROM genai_events
             WHERE event_type = 'llm_call'
             GROUP BY agent
             ORDER BY total_tokens DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AgentTokenSummary {
                agent_name: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                total_tokens: row.get(3)?,
                request_count: row.get(4)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}
