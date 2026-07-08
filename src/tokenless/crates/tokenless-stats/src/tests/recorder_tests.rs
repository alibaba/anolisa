use crate::record::OperationType;

fn new_recorder() -> (StatsRecorder, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let rec = StatsRecorder::new(&db).unwrap();
    (rec, dir)
}

fn sample(op: OperationType, mode: CompressionMode, session: &str) -> StatsRecord {
    StatsRecord::new(op, "cli".to_string(), 1000, 400, 500, 200)
        .with_session_id(session)
        .with_mode(mode)
}

#[test]
fn records_and_reads_mode() {
    let (rec, _dir) = new_recorder();
    let id = rec
        .record(&sample(
            OperationType::CompressSchema,
            CompressionMode::DryRun,
            "s1",
        ))
        .unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.mode, CompressionMode::DryRun);
    assert_eq!(got.session_id.as_deref(), Some("s1"));
}

#[test]
fn records_and_reads_stash_fields() {
    let (rec, _dir) = new_recorder();
    let rec_in = sample(
        OperationType::CompressResponse,
        CompressionMode::Active,
        "stash-ses",
    )
    .with_stash(Some(3), Some(0), Some(42));
    let id = rec.record(&rec_in).unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.stash_writes, Some(3));
    assert_eq!(got.stash_errors, Some(0));
    assert_eq!(got.stash_size, Some(42));
}

#[test]
fn stash_fields_default_none_when_unstashed() {
    let (rec, _dir) = new_recorder();
    let id = rec
        .record(&sample(
            OperationType::CompressResponse,
            CompressionMode::Active,
            "no-stash",
        ))
        .unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.stash_writes, None);
    assert_eq!(got.stash_errors, None);
    assert_eq!(got.stash_size, None);
}

#[test]
fn default_mode_is_active() {
    let (rec, _dir) = new_recorder();
    let id = rec
        .record(&sample(
            OperationType::CompressSchema,
            CompressionMode::Active,
            "s1",
        ))
        .unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.mode, CompressionMode::Active);
}

#[test]
fn records_by_session_filters() {
    let (rec, _dir) = new_recorder();
    rec.record(&sample(
        OperationType::CompressResponse,
        CompressionMode::Active,
        "baseline",
    ))
    .unwrap();
    rec.record(&sample(
        OperationType::CompressResponse,
        CompressionMode::DryRun,
        "tokenless",
    ))
    .unwrap();
    rec.record(&sample(
        OperationType::CompressResponse,
        CompressionMode::Active,
        "baseline",
    ))
    .unwrap();

    let baseline = rec.records_by_session("baseline", None).unwrap();
    let tokenless = rec.records_by_session("tokenless", None).unwrap();
    assert_eq!(baseline.len(), 2);
    assert_eq!(tokenless.len(), 1);
    assert_eq!(tokenless[0].mode, CompressionMode::DryRun);
}

#[test]
fn count_returns_total_records() {
    let (rec, _dir) = new_recorder();
    assert_eq!(rec.count().unwrap(), 0);
    rec.record(&sample(
        OperationType::CompressSchema,
        CompressionMode::Active,
        "s1",
    ))
    .unwrap();
    rec.record(&sample(
        OperationType::CompressResponse,
        CompressionMode::Active,
        "s1",
    ))
    .unwrap();
    assert_eq!(rec.count().unwrap(), 2);
}

#[test]
fn clear_removes_all_records() {
    let (rec, _dir) = new_recorder();
    rec.record(&sample(
        OperationType::CompressSchema,
        CompressionMode::Active,
        "s1",
    ))
    .unwrap();
    rec.record(&sample(
        OperationType::CompressResponse,
        CompressionMode::Active,
        "s1",
    ))
    .unwrap();
    assert_eq!(rec.count().unwrap(), 2);
    rec.clear().unwrap();
    assert_eq!(rec.count().unwrap(), 0);
}

#[test]
fn all_records_with_limit() {
    let (rec, _dir) = new_recorder();
    for _ in 0..5 {
        rec.record(&sample(
            OperationType::CompressSchema,
            CompressionMode::Active,
            "s1",
        ))
        .unwrap();
    }
    let all = rec.all_records(None).unwrap();
    assert_eq!(all.len(), 5);
    let limited = rec.all_records(Some(3)).unwrap();
    assert_eq!(limited.len(), 3);
}

#[test]
fn record_by_id_missing_returns_none() {
    let (rec, _dir) = new_recorder();
    assert!(rec.record_by_id(9999).unwrap().is_none());
}

#[test]
fn summary_from_empty_records() {
    let summary = StatsSummary::from_records(&[]);
    assert_eq!(summary.total_records, 0);
    assert_eq!(summary.chars_saved(), 0);
    assert_eq!(summary.tokens_saved(), 0);
    assert_eq!(summary.chars_percent(), 0.0);
    assert_eq!(summary.tokens_percent(), 0.0);
}

#[test]
fn summary_from_records_aggregates() {
    let records = vec![
        StatsRecord::new(
            OperationType::CompressSchema,
            "a".into(),
            1000,
            400,
            500,
            200,
        ),
        StatsRecord::new(
            OperationType::CompressResponse,
            "b".into(),
            2000,
            800,
            1000,
            400,
        ),
    ];
    let summary = StatsSummary::from_records(&records);
    assert_eq!(summary.total_records, 2);
    assert_eq!(summary.total_before_chars, 3000);
    assert_eq!(summary.total_after_chars, 1500);
    assert_eq!(summary.total_before_tokens, 1200);
    assert_eq!(summary.total_after_tokens, 600);
    assert_eq!(summary.chars_saved(), 1500);
    assert_eq!(summary.tokens_saved(), 600);
    assert!((summary.chars_percent() - 50.0).abs() < 0.1);
    assert!((summary.tokens_percent() - 50.0).abs() < 0.1);
}

#[test]
fn summary_zero_before_returns_zero_percent() {
    let summary = StatsSummary {
        total_records: 1,
        total_before_chars: 0,
        total_after_chars: 0,
        total_before_tokens: 0,
        total_after_tokens: 0,
    };
    assert_eq!(summary.chars_percent(), 0.0);
    assert_eq!(summary.tokens_percent(), 0.0);
}

#[test]
fn actual_savings_percent_zero_session_total() {
    let summary = StatsSummary {
        total_records: 1,
        total_before_chars: 1000,
        total_after_chars: 500,
        total_before_tokens: 400,
        total_after_tokens: 200,
    };
    assert_eq!(summary.actual_savings_percent(0), 0.0);
    let pct = summary.actual_savings_percent(2000);
    assert!((pct - 10.0).abs() < 0.1);
}

#[test]
fn schema_migration_adds_missing_columns() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migrate.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                operation TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                source_pid INTEGER,
                session_id TEXT,
                tool_use_id TEXT,
                before_chars INTEGER NOT NULL,
                before_tokens INTEGER NOT NULL,
                after_chars INTEGER NOT NULL,
                after_tokens INTEGER NOT NULL,
                before_text TEXT,
                after_text TEXT
            )",
        )
        .unwrap();
    }
    let rec = StatsRecorder::new(&db_path).unwrap();
    let mut record =
        StatsRecord::new(OperationType::CompressSchema, "cli".into(), 100, 25, 50, 12)
            .with_mode(CompressionMode::Active)
            .with_stash(Some(1), Some(0), Some(5));
    let id = rec.record(&record).unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.mode, CompressionMode::Active);
    assert_eq!(got.stash_writes, Some(1));
}

#[test]
fn all_records_handles_corrupt_row() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("corrupt.db");
    let rec = StatsRecorder::new(&db_path).unwrap();
    rec.record(&sample(
        OperationType::CompressSchema,
        CompressionMode::Active,
        "s1",
    ))
    .unwrap();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO stats (timestamp, operation, agent_id, before_chars, before_tokens, after_chars, after_tokens)
             VALUES ('not-a-date', 'compress_schema', 'cli', 100, 25, 50, 12)",
            [],
        )
        .unwrap();
    }
    let records = rec.all_records(None).unwrap();
    assert!(records.len() >= 1);
}

#[test]
fn record_with_before_after_output() {
    let (rec, _dir) = new_recorder();
    let record =
        StatsRecord::new(OperationType::CompressSchema, "cli".into(), 100, 25, 50, 12)
            .with_before_text("before-text".to_string())
            .with_after_text("after-text".to_string())
            .with_output("before-output".to_string(), "after-output".to_string());
    let id = rec.record(&record).unwrap();
    let got = rec.record_by_id(id).unwrap().unwrap();
    assert_eq!(got.before_text.as_deref(), Some("before-text"));
    assert_eq!(got.after_text.as_deref(), Some("after-text"));
    assert_eq!(got.before_output.as_deref(), Some("before-output"));
    assert_eq!(got.after_output.as_deref(), Some("after-output"));
}
