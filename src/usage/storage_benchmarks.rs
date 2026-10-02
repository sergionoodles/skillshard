use super::tests::*;
use super::*;

#[test]
#[ignore = "one million synthetic events benchmark; run explicitly in development"]
fn million_event_indexed_query_benchmark() {
    use std::time::Instant;
    const EVENTS: i64 = 1_000_000;
    const EVENT_INTERVAL: i64 = (DAY * 30) / EVENTS;
    let mut database = database();
    let started = Instant::now();
    let transaction = database.connection.transaction().unwrap();
    transaction.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100) INSERT INTO skills(id,identity,name) SELECT x,'identity-'||x,'skill-'||x FROM n;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO sessions(id,agent,native_id,role,root_id) SELECT x,'codex','session-'||x,'main',x FROM n;").unwrap();
    transaction.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO activation_events(session_id,worker_id,native_id,occurred_at,evidence,reference,skill_id,outcome,deduplication_key) SELECT 1+(x%1000),'','call-'||x,?2-((x*?4)%?3),'SkillTool','skill-'||(1+(x%100)),1+(x%100),'succeeded','key-'||x FROM n", params![EVENTS,NOW,DAY*30,EVENT_INTERVAL]).unwrap();
    transaction.commit().unwrap();
    let fixture_time = started.elapsed();
    let (aggregate_time, ranking_time) =
        super::queries::benchmark_components(&database.connection, NOW - DAY * 30, NOW).unwrap();
    let started = Instant::now();
    let snapshot = database
        .snapshot(&Query::default(), NOW - DAY * 30, NOW)
        .unwrap();
    let snapshot_time = started.elapsed();
    assert_eq!(snapshot.counts.activations, EVENTS as u64);
    let started = Instant::now();
    let single = database
        .snapshot(
            &Query {
                skill_ids: vec![1],
                ..Query::default()
            },
            NOW - DAY * 30,
            NOW,
        )
        .unwrap();
    let single_time = started.elapsed();
    assert_eq!(single.counts.activations, 10_000);
    let started = Instant::now();
    let counter: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM activation_events WHERE skill_id=1 AND occurred_at>=?1",
            [NOW - DAY * 30],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(counter, 10_000);
    eprintln!("million events: fixture={fixture_time:?}; all aggregate={aggregate_time:?}; rankings={ranking_time:?}; full snapshot={snapshot_time:?}; skill snapshot={single_time:?}; indexed skill counter={:?}",started.elapsed());
}
