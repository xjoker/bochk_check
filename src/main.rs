mod client;
mod config;
mod models;
mod monitor;
mod notifier;
mod parser;
mod web;

use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use tokio::sync::RwLock;
use tracing::{error, info};

use config::load_config;
use client::{build_client, fetch_date_quota, initialize_session};
use models::{build_web_data, ChangeEntry, SharedWebData, SlotDetail, WebData};
use monitor::drill_down;
use notifier::send_bark_notifications;
use parser::{
    append_change_log, diff_json, extract_available_dates, format_date,
    format_date_quota_changes, format_details_message,
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

    let mut last_response: Option<serde_json::Value> = None;
    let mut last_details: Vec<SlotDetail> = Vec::new();
    let mut fail_count: u32 = 0;
    let mut fail_notified = false;
    let started_at = std::time::Instant::now();
    let mut total_checks: u64 = 0;

    loop {
        let config = match load_config() {
            Ok(c) => c,
            Err(e) => {
                error!("重新加载配置失败: {}", e);
                init_config.clone()
            }
        };

        let interval = Duration::from_secs(config.monitor.interval_secs);
        let cycle_start = std::time::Instant::now();

        let cycle_result = async {
            let cycle_client = build_client(&config.proxy.url)?;
            initialize_session(&cycle_client).await?;
            let current = fetch_date_quota(&cycle_client).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((cycle_client, current))
        }
        .await;

        match cycle_result {
            Ok((cycle_client, current)) => {
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

                let details = if has_available {
                    info!(
                        "[{}] 发现 {} 个可预约日期，开始深度查询... (fetch: {}ms)",
                        now,
                        available.len(),
                        fetch_elapsed
                    );
                    let d = drill_down(&cycle_client, &available).await;
                    Some(d)
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

                        let quota_msgs = format_date_quota_changes(&diffs);
                        let has_new_available =
                            quota_msgs.iter().any(|m| m.contains("出现可预约"));

                        if !quota_msgs.is_empty() {
                            let mut body = quota_msgs.join("\n");

                            if has_new_available {
                                match details {
                                    Some(ref d) if !d.is_empty() => {
                                        body.push_str("\n\n");
                                        body.push_str(&format_details_message(d));
                                    }
                                    Some(_) => {
                                        body.push_str(
                                            "\n\n(已确认存在可预约日期，但暂未获取到分行明细)",
                                        );
                                    }
                                    None => {
                                        body.push_str(
                                            "\n\n(深度查询未执行，正在后续轮次探测)",
                                        );
                                    }
                                }
                            }

                            info!("推送通知:\n{}", body);
                            send_bark_notifications(
                                &bark_client,
                                &config.bark.urls,
                                "BOCHK 预约变化",
                                &body,
                            )
                            .await;
                        }

                        Some(diffs)
                    }
                } else {
                    info!("[{}] 首次获取数据 ({}ms)", now, fetch_elapsed);
                    if let Some(quota) = current.get("dateQuota") {
                        info!("当前 dateQuota: {}", quota);
                    }

                    if has_available {
                        let mut body = format!(
                            "启动即发现可预约日期: {}",
                            available
                                .iter()
                                .map(|d| format_date(d))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        if let Some(ref d) = details {
                            if !d.is_empty() {
                                body.push_str("\n\n");
                                body.push_str(&format_details_message(d));
                            } else {
                                body.push_str("\n\n(已确认存在可预约日期，但暂未获取到分行明细)");
                            }
                        }
                        send_bark_notifications(
                            &bark_client,
                            &config.bark.urls,
                            "BOCHK 有号!",
                            &body,
                        )
                        .await;
                    }

                    None
                };

                // 更新 last_details 缓存（在 details 被 move 之前）
                if let Some(ref d) = details {
                    last_details = d.clone();
                }
                if !has_available {
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
                    let wd = build_web_data(&last_details, &dq, total_checks);
                    let mut lock = web_data.write().await;
                    *lock = wd;
                }

                // 仅首次或有变化时写日志
                let is_first = last_response.is_none();
                if is_first || diff.is_some() {
                    let entry = ChangeEntry {
                        timestamp: now,
                        raw_response: current.clone(),
                        diff,
                        available_dates: available.clone(),
                        details,
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

                // 连续失败 3 次立即告警，之后每 10 次重复告警
                let should_notify = fail_count >= 3
                    && (!fail_notified || fail_count % 10 == 0);

                if should_notify {
                    let uptime_mins = started_at.elapsed().as_secs() / 60;
                    let body = format!(
                        "⚠️ 监控连续失败 {} 次\n\
                         最后错误: {}\n\
                         耗时: {}ms\n\
                         代理: {}\n\
                         已运行: {}分钟\n\
                         已检查: {}次",
                        fail_count,
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
