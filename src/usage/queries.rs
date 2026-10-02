use crate::preferences::TrackingDays;
use crate::usage::{
    Bucket, Counts, Metric, Query, Ranking, RoleFilter, SkillBucket, Snapshot, UsageResult,
};
use chrono::{DateTime, Days, Local};
use rusqlite::{params_from_iter, types::Value, Connection, Row};
use std::path::PathBuf;

const JOINS: &str = " FROM activation_events e JOIN sessions s ON s.id=e.session_id LEFT JOIN projects p ON p.id=s.project_id";
const IS_CHILD: &str = "(s.role='subagent' OR e.worker_id<>'')";
const AGGREGATES: &str = "SUM(e.outcome='succeeded'), COUNT(DISTINCT CASE WHEN e.outcome='succeeded' THEN e.session_id END), COUNT(DISTINCT CASE WHEN e.outcome='succeeded' THEN COALESCE(s.root_id,s.id) END), SUM(e.outcome='succeeded' AND (s.role='subagent' OR e.worker_id<>'')), SUM(e.outcome='succeeded' AND s.role='main' AND e.worker_id=''), SUM(e.outcome='unknown'), MAX(CASE WHEN e.outcome='succeeded' THEN e.occurred_at END), MAX(CASE WHEN e.outcome='succeeded' THEN s.provisional ELSE 0 END)";

pub(super) fn snapshot(
    connection: &Connection,
    query: &Query,
    cutoff: i64,
    now: i64,
) -> UsageResult<Snapshot> {
    let (filter, values) = build_filter(query, cutoff, now, true);
    let (all_skills_filter, all_skills_values) = build_filter(query, cutoff, now, false);
    let counts = connection
        .query_row(
            &format!("SELECT {AGGREGATES}{JOINS}{filter}"),
            params_from_iter(&values),
            |row| read_counts(row, 0),
        )
        .map_err(|error| error.to_string())?;
    let rankings = rankings(
        connection,
        query.metric,
        &all_skills_filter,
        &all_skills_values,
    )?;
    let daily = buckets(connection, query.metric, &filter, &values)?;
    let daily_by_skill = if query.skill_ids.is_empty() {
        Vec::new()
    } else {
        skill_buckets(connection, query.metric, &filter, &values)?
    };
    let unresolved: u64 = connection
        .query_row(
            &format!(
                "SELECT COUNT(*){JOINS}{all_skills_filter} AND e.skill_id IS NULL AND e.outcome<>'failed'"
            ),
            params_from_iter(&all_skills_values),
            |row| unsigned(row, 0),
        )
        .map_err(|error| error.to_string())?;
    let unknown_time: u64 = connection
        .query_row(
            "SELECT COALESCE(SUM(unknown_time),0) FROM sources WHERE (?1 IS NULL OR adapter=?1)",
            [query.agent.as_deref()],
            |row| unsigned(row, 0),
        )
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT DISTINCT agent FROM sessions ORDER BY agent")
        .map_err(|error| error.to_string())?;
    let agents = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT path FROM projects ORDER BY path")
        .map_err(|error| error.to_string())?;
    let projects = statement
        .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT identity,skill_id FROM skill_identities")
        .map_err(|error| error.to_string())?;
    let skill_ids = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<std::collections::HashMap<_, _>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(Snapshot {
        skill_ids,
        counts,
        rankings,
        daily,
        daily_by_skill,
        agents,
        projects,
        unresolved,
        unknown_time,
    })
}

/// `include_skills` applies `Query::skill_ids`; rankings leave it out.
fn build_filter(
    query: &Query,
    cutoff: i64,
    now: i64,
    include_skills: bool,
) -> (String, Vec<Value>) {
    let mut filter = " WHERE e.occurred_at>=?1 AND e.occurred_at<=?2 AND e.inherited=0".to_string();
    let mut values = vec![
        Value::Integer(
            query
                .range_days
                .map(|days| calendar_cutoff(days, now))
                .or(query.since)
                .unwrap_or(cutoff)
                .max(cutoff),
        ),
        Value::Integer(now),
    ];
    if let Some(agent) = &query.agent {
        values.push(Value::Text(agent.clone()));
        filter.push_str(&format!(" AND s.agent=?{}", values.len()));
    }
    if let Some(project) = &query.project {
        values.push(Value::Text(
            super::resolve::normalize_path(project)
                .to_string_lossy()
                .into_owned(),
        ));
        filter.push_str(&format!(" AND p.path=?{}", values.len()));
    }
    if include_skills && !query.skill_ids.is_empty() {
        let placeholders = query
            .skill_ids
            .iter()
            .map(|skill_id| {
                values.push(Value::Integer(*skill_id));
                format!("?{}", values.len())
            })
            .collect::<Vec<_>>()
            .join(",");
        filter.push_str(&format!(" AND e.skill_id IN ({placeholders})"));
    }
    match query.role {
        RoleFilter::All => {}
        RoleFilter::Main => filter.push_str(" AND s.role='main' AND e.worker_id=''"),
        RoleFilter::Subagents => filter.push_str(&format!(" AND {IS_CHILD}")),
    }
    (filter, values)
}

/// Local midnight starting a range of `days` calendar days that ends today,
/// so a reporting range matches the days a daily chart shows.
fn calendar_cutoff(days: TrackingDays, now: i64) -> i64 {
    let previous_days = u64::from(days.days()).saturating_sub(1);
    DateTime::from_timestamp_millis(now)
        .map(|time| time.with_timezone(&Local).date_naive())
        .and_then(|today| today.checked_sub_days(Days::new(previous_days)))
        .and_then(|first| first.and_hms_opt(0, 0, 0))
        .and_then(|midnight| midnight.and_local_timezone(Local).earliest())
        .map_or_else(|| days.cutoff(now), |midnight| midnight.timestamp_millis())
}

fn read_counts(row: &Row<'_>, offset: usize) -> rusqlite::Result<Counts> {
    Ok(Counts {
        activations: unsigned(row, offset)?,
        sessions: unsigned(row, offset + 1)?,
        conversations: unsigned(row, offset + 2)?,
        subagent_activations: unsigned(row, offset + 3)?,
        main_activations: unsigned(row, offset + 4)?,
        unconfirmed: unsigned(row, offset + 5)?,
        last_used: row.get(offset + 6)?,
        provisional_conversations: row.get::<_, Option<bool>>(offset + 7)?.unwrap_or_default(),
    })
}

fn rankings(
    connection: &Connection,
    metric: Metric,
    filter: &str,
    values: &[Value],
) -> UsageResult<Vec<Ranking>> {
    let order = match metric {
        Metric::Activations => 4,
        Metric::Sessions => 5,
        Metric::Conversations => 6,
    };
    let sql = format!("SELECT k.id,k.name,k.identity,{AGGREGATES}{JOINS} JOIN skills k ON k.id=e.skill_id{filter} GROUP BY k.id ORDER BY {order} DESC,k.name,k.id");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params_from_iter(values), |row| {
            Ok(Ranking {
                skill_id: row.get(0)?,
                name: row.get(1)?,
                identity: row.get(2)?,
                counts: read_counts(row, 3)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

const LOCAL_DAY: &str = "date(e.occurred_at/1000.0,'unixepoch','localtime')";

fn metric_counter(metric: Metric) -> &'static str {
    match metric {
        Metric::Activations => "COUNT(*)",
        Metric::Sessions => "COUNT(DISTINCT e.session_id)",
        Metric::Conversations => "COUNT(DISTINCT COALESCE(s.root_id,s.id))",
    }
}

fn buckets(
    connection: &Connection,
    metric: Metric,
    filter: &str,
    values: &[Value],
) -> UsageResult<Vec<Bucket>> {
    let counter = metric_counter(metric);
    let sql = format!("SELECT {LOCAL_DAY} AS label,{counter}{JOINS}{filter} AND e.outcome='succeeded' GROUP BY label ORDER BY label");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params_from_iter(values), |row| {
            Ok(Bucket {
                label: row.get(0)?,
                value: unsigned(row, 1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

fn skill_buckets(
    connection: &Connection,
    metric: Metric,
    filter: &str,
    values: &[Value],
) -> UsageResult<Vec<SkillBucket>> {
    let counter = metric_counter(metric);
    let sql = format!("SELECT {LOCAL_DAY} AS label,e.skill_id,{counter}{JOINS}{filter} AND e.outcome='succeeded' AND e.skill_id IS NOT NULL GROUP BY label,e.skill_id ORDER BY label,e.skill_id");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map(params_from_iter(values), |row| {
            Ok(SkillBucket {
                label: row.get(0)?,
                skill_id: row.get(1)?,
                value: unsigned(row, 2)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}

pub(super) fn unsigned(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, Option<i64>>(index)?.unwrap_or_default();
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

#[cfg(test)]
pub(super) fn benchmark_components(
    connection: &Connection,
    cutoff: i64,
    now: i64,
) -> UsageResult<(std::time::Duration, std::time::Duration)> {
    let (filter, values) = build_filter(&Query::default(), cutoff, now, true);
    let started = std::time::Instant::now();
    connection
        .query_row(
            &format!("SELECT {AGGREGATES}{JOINS}{filter}"),
            params_from_iter(&values),
            |row| read_counts(row, 0),
        )
        .map_err(|error| error.to_string())?;
    let aggregate_time = started.elapsed();
    let started = std::time::Instant::now();
    rankings(connection, Metric::Activations, &filter, &values)?;
    Ok((aggregate_time, started.elapsed()))
}
