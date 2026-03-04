use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{NaiveDateTime, Timelike};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::models::{
    BranchInfo, SlotDetail, WebAppointmentTimeStat, WebBranchCatalogEntry, WebBranchReleaseStat,
    WebHistoryData, WebHistoryDaySummary, WebHistoryEvent, WebPagination, WebReleaseBucketStat,
    WebReleaseWindowStat,
};

const DB_SCHEMA_VERSION: i64 = 3;

#[derive(Clone, Default)]
pub struct RuntimeState {
    pub last_release_at: String,
}

fn db_path() -> PathBuf {
    crate::config::data_file_dir().join("bochk_check.db")
}

fn recreate_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    conn.execute_batch(
        r#"
        DROP TABLE IF EXISTS slot_events;
        DROP TABLE IF EXISTS current_slots;
        DROP TABLE IF EXISTS branches;
        DROP TABLE IF EXISTS app_state;

        CREATE TABLE app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE branches (
            branch_code TEXT PRIMARY KEY,
            branch_name TEXT NOT NULL,
            address_cn TEXT NOT NULL,
            tel_no TEXT NOT NULL,
            google_maps_url TEXT NOT NULL,
            is_enabled INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE current_slots (
            branch_code TEXT NOT NULL,
            appointment_date TEXT NOT NULL,
            appointment_time TEXT NOT NULL,
            first_seen_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            PRIMARY KEY (branch_code, appointment_date, appointment_time)
        );
        CREATE TABLE slot_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_at TEXT NOT NULL,
            event_type TEXT NOT NULL,
            branch_code TEXT NOT NULL,
            appointment_date TEXT NOT NULL,
            appointment_time TEXT NOT NULL
        );
        CREATE INDEX idx_slot_events_time ON slot_events(event_at);
        CREATE INDEX idx_slot_events_branch ON slot_events(branch_code, event_at);
        "#,
    )?;
    conn.pragma_update(None, "user_version", DB_SCHEMA_VERSION)?;
    Ok(())
}

fn ensure_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != DB_SCHEMA_VERSION {
        recreate_schema(conn)?;
    }
    Ok(())
}

fn open_conn() -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn upsert_branch(
    tx: &Transaction<'_>,
    branch: &BranchInfo,
    event_at: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let google_maps_url = crate::notifier::build_map_link(&branch.name, &branch.address_cn);
    tx.execute(
        r#"
        INSERT INTO branches (
            branch_code, branch_name, address_cn, tel_no, google_maps_url, is_enabled, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
        ON CONFLICT(branch_code) DO UPDATE SET
            branch_name=excluded.branch_name,
            address_cn=excluded.address_cn,
            tel_no=excluded.tel_no,
            google_maps_url=excluded.google_maps_url,
            updated_at=excluded.updated_at
        "#,
        params![
            branch.code,
            branch.name,
            branch.address_cn,
            branch.tel_no,
            google_maps_url,
            event_at
        ],
    )?;
    Ok(())
}

fn flatten(details: &[SlotDetail]) -> BTreeMap<(String, String, String), BranchInfo> {
    let mut map = BTreeMap::new();
    for slot in details {
        for branch in &slot.branches {
            map.insert(
                (branch.code.clone(), slot.date.clone(), slot.time.clone()),
                branch.clone(),
            );
        }
    }
    map
}

pub fn load_runtime_state() -> Result<RuntimeState, Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    conn.execute(
        "INSERT OR IGNORE INTO app_state(key, value) VALUES ('last_release_at', '')",
        [],
    )?;
    let last_release_at = conn
        .query_row(
            "SELECT value FROM app_state WHERE key='last_release_at'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_default();

    Ok(RuntimeState { last_release_at })
}

pub fn save_runtime_state(
    state: &RuntimeState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    conn.execute(
        "INSERT INTO app_state(key, value) VALUES ('last_release_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![state.last_release_at],
    )?;
    Ok(())
}

pub fn load_current_slots() -> Result<Vec<SlotDetail>, Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            c.appointment_date,
            c.appointment_time,
            b.branch_code,
            b.branch_name,
            b.address_cn,
            b.tel_no
        FROM current_slots c
        JOIN branches b ON b.branch_code = c.branch_code
        WHERE b.is_enabled = 1
        ORDER BY c.appointment_date, c.appointment_time, b.branch_name
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            BranchInfo {
                code: row.get::<_, String>(2)?,
                name: row.get::<_, String>(3)?,
                status: "A".to_string(),
                address_cn: row.get::<_, String>(4)?,
                tel_no: row.get::<_, String>(5)?,
            },
        ))
    })?;

    let mut grouped: BTreeMap<(String, String), Vec<BranchInfo>> = BTreeMap::new();
    for row in rows {
        let (date, time, branch) = row?;
        grouped.entry((date, time)).or_default().push(branch);
    }

    Ok(grouped
        .into_iter()
        .map(|((date, time), branches)| SlotDetail {
            date,
            time,
            time_slot_id: String::new(),
            branches,
        })
        .collect())
}

pub fn load_current_slot_first_seen_map(
) -> Result<BTreeMap<(String, String, String), String>, Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            c.branch_code,
            c.appointment_date,
            c.appointment_time,
            c.first_seen_at
        FROM current_slots c
        JOIN branches b ON b.branch_code = c.branch_code
        WHERE b.is_enabled = 1
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let mut map = BTreeMap::new();
    for row in rows {
        let (code, date, time, first_seen_at) = row?;
        map.insert((code, date, time), first_seen_at);
    }
    Ok(map)
}

pub fn persist_snapshot_diff(
    old_details: &[SlotDetail],
    new_details: &[SlotDetail],
    event_at: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = open_conn()?;
    let tx = conn.transaction()?;

    let old_map = flatten(old_details);
    let new_map = flatten(new_details);

    for branch in new_map.values() {
        upsert_branch(&tx, branch, event_at)?;
    }

    for (code, date, time) in new_map.keys() {
        if !old_map.contains_key(&(code.clone(), date.clone(), time.clone())) {
            tx.execute(
                r#"
                INSERT INTO slot_events (
                    event_at, event_type, branch_code, appointment_date, appointment_time
                ) VALUES (?1, 'appeared', ?2, ?3, ?4)
                "#,
                params![event_at, code, date, time],
            )?;
        }
    }

    for (code, date, time) in old_map.keys() {
        if !new_map.contains_key(&(code.clone(), date.clone(), time.clone())) {
            tx.execute(
                r#"
                INSERT INTO slot_events (
                    event_at, event_type, branch_code, appointment_date, appointment_time
                ) VALUES (?1, 'disappeared', ?2, ?3, ?4)
                "#,
                params![event_at, code, date, time],
            )?;
        }
    }

    tx.execute("DELETE FROM current_slots", [])?;
    for ((code, date, time), _) in &new_map {
        let first_seen_at = tx
            .query_row(
                r#"
                SELECT MIN(event_at) FROM slot_events
                WHERE event_type='appeared'
                  AND branch_code=?1
                  AND appointment_date=?2
                  AND appointment_time=?3
                "#,
                params![code, date, time],
                |row| row.get::<_, Option<String>>(0),
            )?
            .unwrap_or_else(|| event_at.to_string());

        tx.execute(
            r#"
            INSERT INTO current_slots (
                branch_code, appointment_date, appointment_time, first_seen_at, last_seen_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![code, date, time, first_seen_at, event_at],
        )?;
    }

    tx.commit()?;
    Ok(())
}

pub fn upsert_branch_catalog(
    branches: &[BranchInfo],
    updated_at: &str,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    if branches.is_empty() {
        return Ok(0);
    }

    let mut conn = open_conn()?;
    let tx = conn.transaction()?;
    let mut seen_codes = BTreeSet::new();
    let mut updated = 0usize;

    for branch in branches {
        if !seen_codes.insert(branch.code.clone()) {
            continue;
        }
        upsert_branch(&tx, branch, updated_at)?;
        updated += 1;
    }

    tx.commit()?;
    Ok(updated)
}

pub fn load_branch_contacts(
    branch_codes: &[String],
) -> Result<BTreeMap<String, (String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    if branch_codes.is_empty() {
        return Ok(BTreeMap::new());
    }

    let conn = open_conn()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            branch_code,
            address_cn,
            tel_no
        FROM branches
        WHERE branch_code = ?1
        "#,
    )?;

    let mut result = BTreeMap::new();
    for code in branch_codes {
        if let Some(found) = stmt
            .query_row(params![code], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?
        {
            result.insert(found.0, (found.1, found.2));
        }
    }

    Ok(result)
}

pub fn load_branch_catalog(
) -> Result<Vec<WebBranchCatalogEntry>, Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            branch_code,
            branch_name,
            address_cn,
            tel_no,
            is_enabled,
            updated_at
        FROM branches
        ORDER BY is_enabled DESC, branch_name ASC, branch_code ASC
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(WebBranchCatalogEntry {
            code: row.get::<_, String>(0)?,
            name: row.get::<_, String>(1)?,
            address_cn: row.get::<_, String>(2)?,
            tel_no: row.get::<_, String>(3)?,
            is_enabled: row.get::<_, i64>(4)? != 0,
            updated_at: row.get::<_, String>(5)?,
        })
    })?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}


pub fn filter_enabled_details(
    details: &[SlotDetail],
) -> Result<Vec<SlotDetail>, Box<dyn std::error::Error + Send + Sync>> {
    if details.is_empty() {
        return Ok(Vec::new());
    }

    let conn = open_conn()?;
    let mut stmt = conn.prepare("SELECT branch_code FROM branches WHERE is_enabled = 0")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut disabled_codes = BTreeSet::new();
    for row in rows {
        disabled_codes.insert(row?);
    }

    if disabled_codes.is_empty() {
        return Ok(details.to_vec());
    }

    let mut filtered = Vec::new();
    for slot in details {
        let branches: Vec<BranchInfo> = slot
            .branches
            .iter()
            .filter(|branch| !disabled_codes.contains(&branch.code))
            .cloned()
            .collect();
        if !branches.is_empty() {
            filtered.push(SlotDetail {
                date: slot.date.clone(),
                time: slot.time.clone(),
                time_slot_id: slot.time_slot_id.clone(),
                branches,
            });
        }
    }

    Ok(filtered)
}

pub fn load_web_history(
    day_limit: usize,
    event_page: usize,
    event_page_size: usize,
    top_branch_limit: usize,
    top_time_limit: usize,
) -> Result<WebHistoryData, Box<dyn std::error::Error + Send + Sync>> {
    let conn = open_conn()?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let today = conn.query_row(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN event_type='appeared' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN event_type='disappeared' THEN 1 ELSE 0 END), 0)
        FROM slot_events
        WHERE substr(event_at, 1, 10) = ?1
        "#,
        params![today],
        |row| {
            Ok(WebHistoryDaySummary {
                date: chrono::Local::now().format("%Y-%m-%d").to_string(),
                appeared_count: row.get::<_, i64>(0)? as u32,
                disappeared_count: row.get::<_, i64>(1)? as u32,
            })
        },
    )?;

    let mut recent_days_stmt = conn.prepare(
        r#"
        SELECT
            substr(event_at, 1, 10) AS stat_date,
            COALESCE(SUM(CASE WHEN event_type='appeared' THEN 1 ELSE 0 END), 0) AS appeared_count,
            COALESCE(SUM(CASE WHEN event_type='disappeared' THEN 1 ELSE 0 END), 0) AS disappeared_count
        FROM slot_events
        GROUP BY stat_date
        ORDER BY stat_date DESC
        LIMIT ?1
        "#,
    )?;
    let recent_days_rows = recent_days_stmt.query_map(params![day_limit as i64], |row| {
        Ok(WebHistoryDaySummary {
            date: row.get::<_, String>(0)?,
            appeared_count: row.get::<_, i64>(1)? as u32,
            disappeared_count: row.get::<_, i64>(2)? as u32,
        })
    })?;
    let mut recent_days = Vec::new();
    for row in recent_days_rows {
        recent_days.push(row?);
    }
    recent_days.reverse();

    let since_date = (chrono::Local::now() - chrono::Duration::days(6))
        .format("%Y-%m-%d")
        .to_string();
    let mut top_branch_stmt = conn.prepare(
        r#"
        SELECT
            COALESCE(MAX(b.branch_name), e.branch_code) AS branch_name,
            COUNT(*) AS release_count
        FROM slot_events e
        LEFT JOIN branches b ON b.branch_code = e.branch_code
        WHERE e.event_type='appeared'
          AND substr(e.event_at, 1, 10) >= ?1
        GROUP BY e.branch_code
        ORDER BY release_count DESC, branch_name ASC
        LIMIT ?2
        "#,
    )?;
    let top_branch_rows = top_branch_stmt.query_map(
        params![since_date, top_branch_limit as i64],
        |row| {
            Ok(WebBranchReleaseStat {
                branch_name: row.get::<_, String>(0)?,
                release_count: row.get::<_, i64>(1)? as u32,
            })
        },
    )?;
    let mut top_release_branches = Vec::new();
    for row in top_branch_rows {
        top_release_branches.push(row?);
    }

    let top_appointment_times =
        load_top_appointment_times(&conn, &since_date, top_time_limit)?;
    let release_points = load_release_points(&conn, &since_date)?;
    let all_release_windows = build_release_buckets(&release_points);
    let mut top_release_windows = build_release_windows(&release_points);
    top_release_windows.sort_by(|a, b| {
        b.sample_count
            .cmp(&a.sample_count)
            .then_with(|| a.center_time.cmp(&b.center_time))
    });
    top_release_windows.truncate(top_time_limit);

    let total_items = conn.query_row("SELECT COUNT(*) FROM slot_events", [], |row| {
        row.get::<_, i64>(0)
    })? as usize;
    let total_pages = if total_items == 0 {
        0
    } else {
        (total_items + event_page_size.saturating_sub(1)) / event_page_size.max(1)
    };
    let page = if total_pages == 0 {
        1
    } else {
        event_page.max(1).min(total_pages)
    };
    let page_size = event_page_size.max(1);
    let offset = page.saturating_sub(1) * page_size;

    let mut recent_event_stmt = conn.prepare(
        r#"
        SELECT
            e.id,
            e.event_at,
            e.event_type,
            e.appointment_date,
            e.appointment_time,
            e.branch_code,
            COALESCE(b.branch_name, e.branch_code),
            COALESCE(b.address_cn, ''),
            COALESCE(b.tel_no, ''),
            COALESCE(b.google_maps_url, '')
        FROM slot_events e
        LEFT JOIN branches b ON b.branch_code = e.branch_code
        ORDER BY e.id DESC
        LIMIT ?1 OFFSET ?2
        "#,
    )?;
    let recent_event_rows =
        recent_event_stmt.query_map(params![page_size as i64, offset as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;

    let mut duration_stmt = conn.prepare(
        r#"
        SELECT event_at
        FROM slot_events
        WHERE event_type='appeared'
          AND branch_code=?1
          AND appointment_date=?2
          AND appointment_time=?3
          AND id <= ?4
        ORDER BY id DESC
        LIMIT 1
        "#,
    )?;
    let mut recent_events = Vec::new();
    for row in recent_event_rows {
        let (
            event_id,
            event_at,
            event_type,
            appointment_date,
            appointment_time,
            branch_code,
            branch_name,
            address_cn,
            tel_no,
            google_maps_url,
        ) = row?;

        let appeared_at = duration_stmt
            .query_row(
                params![branch_code, appointment_date, appointment_time, event_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;

        let duration_secs = appeared_at
            .as_deref()
            .and_then(|start| duration_seconds_between(start, &event_at))
            .unwrap_or(0);

        recent_events.push(WebHistoryEvent {
            event_at,
            event_type,
            appointment_date: crate::parser::format_date(&appointment_date),
            appointment_time,
            branch_name,
            duration_secs,
            address_cn,
            tel_no,
            google_maps_url,
        });
    }

    Ok(WebHistoryData {
        today,
        recent_days,
        recent_events,
        recent_events_pagination: WebPagination {
            page,
            page_size,
            total_items,
            total_pages,
        },
        top_release_branches,
        top_appointment_times,
        top_release_windows,
        all_release_windows,
    })
}

fn load_top_appointment_times(
    conn: &Connection,
    since_date: &str,
    limit: usize,
) -> Result<Vec<WebAppointmentTimeStat>, Box<dyn std::error::Error + Send + Sync>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            appointment_time,
            COUNT(*) AS release_count
        FROM slot_events
        WHERE event_type='appeared'
          AND substr(event_at, 1, 10) >= ?1
        GROUP BY appointment_time
        ORDER BY release_count DESC, appointment_time ASC
        LIMIT ?2
        "#,
    )?;

    let rows = stmt.query_map(params![since_date, limit as i64], |row| {
        Ok(WebAppointmentTimeStat {
            appointment_time: row.get::<_, String>(0)?,
            release_count: row.get::<_, i64>(1)? as u32,
        })
    })?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

fn load_release_points(
    conn: &Connection,
    since_date: &str,
) -> Result<Vec<u32>, Box<dyn std::error::Error + Send + Sync>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT event_at
        FROM slot_events
        WHERE event_type='appeared'
          AND substr(event_at, 1, 10) >= ?1
        ORDER BY event_at ASC
        "#,
    )?;

    let rows = stmt.query_map(params![since_date], |row| row.get::<_, String>(0))?;
    let mut points = Vec::new();
    for row in rows {
        let event_at = row?;
        if let Some(seconds) = hhmm_seconds_from_timestamp(&event_at) {
            points.push(seconds);
        }
    }

    Ok(points)
}

fn build_release_windows(points: &[u32]) -> Vec<WebReleaseWindowStat> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut clusters: Vec<Vec<u32>> = Vec::new();
    let cluster_gap_secs = 20 * 60;
    for point in points {
        if let Some(last_cluster) = clusters.last_mut() {
            let last_point = *last_cluster.last().unwrap_or(point);
            if point.saturating_sub(last_point) <= cluster_gap_secs {
                last_cluster.push(*point);
                continue;
            }
        }
        clusters.push(vec![*point]);
    }

    let mut windows: Vec<WebReleaseWindowStat> = clusters
        .into_iter()
        .map(|cluster| {
            let min = *cluster.first().unwrap_or(&0);
            let max = *cluster.last().unwrap_or(&0);
            let count = cluster.len() as u32;
            let sum: u64 = cluster.iter().map(|v| *v as u64).sum();
            let center = (sum / cluster.len() as u64) as u32;

            WebReleaseWindowStat {
                center_time: format_hhmm(center),
                range_start: format_hhmm(min),
                range_end: format_hhmm(max),
                minus_minutes: ((center.saturating_sub(min)) + 59) / 60,
                plus_minutes: ((max.saturating_sub(center)) + 59) / 60,
                sample_count: count,
            }
        })
        .collect();
    windows.sort_by(|a, b| a.center_time.cmp(&b.center_time));

    windows
}

fn build_release_buckets(points: &[u32]) -> Vec<WebReleaseBucketStat> {
    use std::collections::BTreeMap;

    if points.is_empty() {
        return Vec::new();
    }

    let mut buckets: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for point in points {
        let bucket_key = *point / 1800;
        buckets.entry(bucket_key).or_default().push(*point);
    }

    buckets
        .into_iter()
        .map(|(bucket_key, values)| {
            let bucket_start = bucket_key * 1800;
            let bucket_end = ((bucket_key + 1) * 1800).min(24 * 3600);
            let observed_start = *values.iter().min().unwrap_or(&bucket_start);
            let observed_end = *values.iter().max().unwrap_or(&bucket_start);

            WebReleaseBucketStat {
                bucket_label: format!(
                    "{}-{}",
                    format_hhmm(bucket_start),
                    format_hhmm_edge(bucket_end)
                ),
                observed_start: format_hhmm(observed_start),
                observed_end: format_hhmm(observed_end),
                sample_count: values.len() as u32,
            }
        })
        .collect()
}

fn hhmm_seconds_from_timestamp(value: &str) -> Option<u32> {
    let dt = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(dt.time().num_seconds_from_midnight())
}

fn format_hhmm(total_seconds: u32) -> String {
    let hours = (total_seconds / 3600) % 24;
    let minutes = (total_seconds % 3600) / 60;
    format!("{:02}:{:02}", hours, minutes)
}

fn format_hhmm_edge(total_seconds: u32) -> String {
    if total_seconds >= 24 * 3600 {
        "24:00".to_string()
    } else {
        format_hhmm(total_seconds)
    }
}

fn duration_seconds_between(start: &str, end: &str) -> Option<u64> {
    let start_dt = NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S").ok()?;
    let end_dt = NaiveDateTime::parse_from_str(end, "%Y-%m-%d %H:%M:%S").ok()?;
    let seconds = (end_dt - start_dt).num_seconds();
    if seconds < 0 {
        return Some(0);
    }
    Some(seconds as u64)
}
