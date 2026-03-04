mod client;
mod config;
mod models;
mod monitor;
mod notifier;
mod parser;
mod state;
mod web;

use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::{error, info};

use config::{load_config, set_persist_jsonl_enabled};
use client::{
    build_client, fetch_date_quota, initialize_session,
};
use models::{build_web_data, ChangeEntry, SharedWebData, SlotDetail, WebData};
use monitor::{drill_down, DrillDownResult};
use notifier::send_bark_notifications;
use parser::{
    append_change_log, count_detail_points, details_cover_dates, diff_detail_snapshots, diff_json,
    extract_available_dates, format_date, format_detail_change_message,
};
use state::{
    filter_enabled_details, load_current_slot_first_seen_map, load_current_slots,
    load_runtime_state, persist_snapshot_diff, save_runtime_state, RuntimeState,
};
use web::start_web_server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!("BOCHK 预约监控启动");

    let init_config = load_config()?;
    set_persist_jsonl_enabled(init_config.logging.persist_jsonl);
    // Bark 通知用独立 client（无代理、独立超时）
    let bark_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Web 共享状态
    let web_data: SharedWebData = Arc::new(RwLock::new(WebData::default()));

    // 启动 Web 服务
    if init_config.web.enabled {
        let wd = web_data.clone();
        let port = init_config.web.port;
        tokio::spawn(start_web_server(port, wd));
    }

    let mut runtime_state = match load_runtime_state() {
        Ok(state) => state,
        Err(e) => {
            error!("加载运行时状态失败: {}", e);
            RuntimeState::default()
        }
    };
    let mut last_notified_details = match load_current_slots() {
        Ok(details) => details,
        Err(e) => {
            error!("加载当前可预约快照失败: {}", e);
            Vec::new()
        }
    };

    let mut last_response: Option<serde_json::Value> = None;
    let mut last_details: Vec<SlotDetail> = Vec::new();
    let mut fail_count: u32 = 0;
    let mut fail_notified = false;
    let started_at = std::time::Instant::now();
    let mut total_checks: u64 = 0;
    let mut last_good_config = init_config.clone();
    let mut last_release_at = runtime_state.last_release_at.clone();

    loop {
        let config = match load_config() {
            Ok(c) => {
                last_good_config = c.clone();
                c
            }
            Err(e) => {
                error!("重新加载配置失败: {}", e);
                last_good_config.clone()
            }
        };
        set_persist_jsonl_enabled(config.logging.persist_jsonl);

        let interval = Duration::from_secs(config.monitor.interval_secs);
        let cycle_start = std::time::Instant::now();

        let cycle_result = async {
            let cycle_client = build_client(&config.proxy.url)?;
            let _ = initialize_session(&cycle_client).await?;
            let current = fetch_date_quota(&cycle_client).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(current)
        }
        .await;

        match cycle_result {
            Ok(current) => {
                if fail_count > 0 {
                    info!("请求恢复正常（此前连续失败 {} 次）", fail_count);
                    fail_count = 0;
                    fail_notified = false;
                }
                total_checks += 1;

                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let fetch_elapsed = cycle_start.elapsed().as_millis();
                let available = extract_available_dates(&current);

                let has_available = !available.is_empty();

                let drill_result = if has_available {
                    info!(
                        "[{}] 发现 {} 个可预约日期，开始深度查询... (fetch: {}ms)",
                        now,
                        available.len(),
                        fetch_elapsed
                    );
                    Some(drill_down(&config.proxy.url, &available).await)
                } else {
                    None
                };

                let diff = if let Some(ref prev) = last_response {
                    let diffs = diff_json("", prev, &current);
                    if diffs.is_empty() {
                        info!("[{}] 无变化 ({}ms)", now, cycle_start.elapsed().as_millis());
                        None
                    } else {
                        info!("[{}] 检测到 {} 处变化 (fetch: {}ms)", now, diffs.len(), fetch_elapsed);

                        Some(diffs)
                    }
                } else {
                    info!("[{}] 首次获取数据 ({}ms)", now, fetch_elapsed);
                    if let Some(quota) = current.get("dateQuota") {
                        info!("当前 dateQuota: {}", quota);
                    }

                    None
                };

                let raw_current_details = drill_result
                    .as_ref()
                    .map(|result| result.details.clone())
                    .unwrap_or_default();
                let current_details = match filter_enabled_details(&raw_current_details) {
                    Ok(filtered) => filtered,
                    Err(e) => {
                        error!("过滤已禁用分行失败: {}", e);
                        raw_current_details.clone()
                    }
                };
                let details_complete = if has_available {
                    let covers_dates = details_cover_dates(&available, &raw_current_details);
                    let probe_complete = drill_result
                        .as_ref()
                        .map(DrillDownResult::is_complete)
                        .unwrap_or(false);
                    covers_dates && probe_complete
                } else {
                    true
                };

                let mut bark_title: Option<String> = None;
                let mut bark_sections: Vec<String> = Vec::new();

                if has_available && !details_complete {
                    let skipped_slots = drill_result
                        .as_ref()
                        .map(|result| result.soft_skipped_slots)
                        .unwrap_or(0);
                    info!(
                        "本轮深度查询未覆盖全部可预约日期，跳过 Bark 通知: {}",
                        available
                            .iter()
                            .map(|d| format_date(d))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    info!("skip Bark due to incomplete probe, skipped_slots={}", skipped_slots);
                } else {
                    let (added, removed) =
                        diff_detail_snapshots(&last_notified_details, &current_details);
                    let removed_durations = if removed.is_empty() {
                        std::collections::BTreeMap::new()
                    } else {
                        let first_seen_map = match load_current_slot_first_seen_map() {
                            Ok(map) => map,
                            Err(e) => {
                                error!("璇诲彇杩囨湡鐐逛綅棣栨鎹曡幏鏃堕棿澶辫触: {}", e);
                                std::collections::BTreeMap::new()
                            }
                        };
                        build_removed_duration_map(&removed, &first_seen_map, &now)
                    };
                    let body = format_detail_change_message(&added, &removed, &removed_durations);

                    if !body.is_empty() {
                        if !added.is_empty() {
                            last_release_at = now.clone();
                            runtime_state.last_release_at = last_release_at.clone();
                            if let Err(e) = save_runtime_state(&runtime_state) {
                                error!("写入运行时状态失败: {}", e);
                            }
                        }
                        let current_total = count_detail_points(&current_details);
                        bark_title = Some(format!(
                            "BOCHK 当前可约 {} 个（+{} / -{}）",
                            current_total,
                            added.len(),
                            removed.len()
                        ));
                        info!("推送明细变化通知:\n{}", body);
                        bark_sections.push(body);
                    } else {
                        info!("[{}] 分行时段明细无变化", now);
                    }
                }

                if let Some(title) = bark_title {
                    let body = bark_sections.join("\n\n---\n\n");
                    if !body.is_empty() {
                        send_bark_notifications(
                            &bark_client,
                            &config.bark.urls,
                            &title,
                            &body,
                        )
                        .await;
                    }
                }

                if !has_available || details_complete {
                    if let Err(e) =
                        persist_snapshot_diff(&last_notified_details, &current_details, &now)
                    {
                        error!("写入 SQLite 快照失败: {}", e);
                    } else {
                        last_notified_details = current_details.clone();
                    }
                }

                // 更新 last_details 缓存（在 details 被 move 之前）
                if has_available {
                    last_details = current_details.clone();
                } else {
                    last_details.clear();
                }

                // 更新 Web 共享数据
                {
                    let mut dq = std::collections::BTreeMap::new();
                    if let Some(quota) = current.get("dateQuota").and_then(|v| v.as_object()) {
                        for (k, v) in quota {
                            dq.insert(k.clone(), v.as_str().unwrap_or("F").to_string());
                        }
                    }
                    let first_seen_map = match load_current_slot_first_seen_map() {
                        Ok(map) => map,
                        Err(e) => {
                            error!("读取当前点位首次捕获时间失败: {}", e);
                            std::collections::BTreeMap::new()
                        }
                    };
                    let wd = build_web_data(
                        &last_details,
                        &dq,
                        total_checks,
                        &last_release_at,
                        &first_seen_map,
                    );
                    let mut lock = web_data.write().await;
                    *lock = wd;
                }

                // 仅首次或有变化时写日志
                let is_first = last_response.is_none();
                if is_first || diff.is_some() {
                    let entry = ChangeEntry {
                        timestamp: now.clone(),
                        raw_response: current.clone(),
                        diff,
                        available_dates: available.clone(),
                        details: drill_result.map(|result| result.details),
                    };
                    if let Err(e) = append_change_log(&entry) {
                        error!("写入变化日志失败: {}", e);
                    }
                }

                last_response = Some(current);
            }
            Err(e) => {
                fail_count += 1;
                let elapsed_ms = cycle_start.elapsed().as_millis();
                error!("请求失败 (连续第 {} 次, {}ms): {}", fail_count, elapsed_ms, e);

                let alert_threshold = config.monitor.max_fail_count.max(1);
                // 首次达到阈值立即告警；之后每额外 10 次失败重复告警一次。
                let should_notify = fail_count >= alert_threshold
                    && (!fail_notified
                        || (fail_count - alert_threshold) % 10 == 0);

                if should_notify {
                    let uptime_mins = started_at.elapsed().as_secs() / 60;
                    let body = format!(
                        "⚠️ 监控连续失败 {} 次\n\
                         告警阈值: {}次\n\
                         最后错误: {}\n\
                         耗时: {}ms\n\
                         代理: {}\n\
                         已运行: {}分钟\n\
                         已检查: {}次",
                        fail_count,
                        alert_threshold,
                        e,
                        elapsed_ms,
                        config.proxy.url,
                        uptime_mins,
                        total_checks
                    );
                    send_bark_notifications(
                        &bark_client,
                        &config.bark.urls,
                        "BOCHK 监控异常",
                        &body,
                    )
                    .await;
                    fail_notified = true;
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

fn build_removed_duration_map(
    removed: &[SlotDetail],
    first_seen_map: &std::collections::BTreeMap<(String, String, String), String>,
    expired_at: &str,
) -> std::collections::BTreeMap<(String, String, String, String), u64> {
    let mut durations = std::collections::BTreeMap::new();

    for slot in removed {
        for branch in &slot.branches {
            if let Some(first_seen_at) =
                first_seen_map.get(&(branch.code.clone(), slot.date.clone(), slot.time.clone()))
            {
                if let Some(duration_secs) = duration_seconds_between(first_seen_at, expired_at) {
                    durations.insert(
                        (
                            branch.code.clone(),
                            slot.date.clone(),
                            slot.time.clone(),
                            slot.time_slot_id.clone(),
                        ),
                        duration_secs,
                    );
                }
            }
        }
    }

    durations
}

fn duration_seconds_between(start: &str, end: &str) -> Option<u64> {
    let start_dt = chrono::NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S").ok()?;
    let end_dt = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%d %H:%M:%S").ok()?;
    let seconds = (end_dt - start_dt).num_seconds();
    if seconds < 0 {
        return Some(0);
    }
    Some(seconds as u64)
}
