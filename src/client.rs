use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Local;
use tracing::{debug, error, info, warn};
use reqwest::Proxy;

pub const BASE_URL: &str = "https://transaction.bochk.com/whk/form/openAccount/";
pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 MicroMessenger/8.0.69(0x1800452d) NetType/4G Language/zh_CN";
pub const REFERER: &str = "https://transaction.bochk.com/whk/form/openAccount/continueInput.action";
pub const ORIGIN: &str = "https://transaction.bochk.com";
pub const ACCEPT_LANGUAGE: &str = "zh-SG,zh-CN;q=0.9,zh-Hans;q=0.8";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn build_client(proxy_url: &str) -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .cookie_store(true)
        .connect_timeout(Duration::from_secs(5));

    if !proxy_url.is_empty() {
        let proxy = Proxy::all(proxy_url)?;
        builder = builder.proxy(proxy);
        debug!("使用代理: {}", proxy_url);
    } else {
        debug!("未配置代理，直连请求");
    }

    Ok(builder.build()?)
}

pub async fn initialize_session(
    client: &reqwest::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let req_id = next_request_id();
    let url = format!("{}continueInput.action", BASE_URL);
    let start = std::time::Instant::now();
    info!(
        target: "bochk_check::request_log",
        "REQ#{req_id} START init_session GET continueInput.action"
    );
    let resp = client
        .get(&url)
        .header("Referer", REFERER)
        .header("Origin", ORIGIN)
        .header("Accept-Language", ACCEPT_LANGUAGE)
        .send()
        .await?;
    let status = resp.status();

    if !status.is_success() {
        warn!(
            target: "bochk_check::request_log",
            "REQ#{req_id} FAIL init_session HTTP {} ({}ms)",
            status,
            start.elapsed().as_millis()
        );
        return Err(std::io::Error::other(format!(
            "会话初始化失败: HTTP {}",
            status
        ))
        .into());
    }

    info!(
        target: "bochk_check::request_log",
        "REQ#{req_id} OK init_session ({}ms)",
        start.elapsed().as_millis()
    );
    debug!("会话初始化成功 ({}ms)", start.elapsed().as_millis());
    Ok(())
}

fn ensure_business_success(
    action: &str,
    body: &str,
    json: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match json.get("eaiCode").and_then(|v| v.as_str()) {
        Some("SUCCESS") | None => Ok(()),
        Some(code) => {
            let msg = json
                .get("eaiMsg")
                .and_then(|v| v.as_str())
                .unwrap_or("无详细信息");
            let err = std::io::Error::other(format!(
                "业务错误 {}: {} | body: {} | eaiMsg: {}",
                code, action, body, msg
            ));
            Err(err.into())
        }
    }
}

/// 通用 POST 请求，带重试（最多 3 次，间隔 300ms）
pub async fn api_post(
    client: &reqwest::Client,
    action: &str,
    body: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let req_id = next_request_id();
    let url = format!("{}{}", BASE_URL, action);
    let max_retries: u32 = 3;
    let mut last_err: Box<dyn std::error::Error + Send + Sync> = "unknown".into();

    info!(
        target: "bochk_check::request_log",
        "REQ#{req_id} START POST {} | body: {}",
        action,
        body
    );

    for attempt in 1..=max_retries {
        let start = std::time::Instant::now();
        debug!("→ POST {} | body: {} | attempt {}/{}", action, body, attempt, max_retries);

        let result = client
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Referer", REFERER)
            .header("Origin", ORIGIN)
            .header("Accept-Language", ACCEPT_LANGUAGE)
            .body(body.to_string())
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                let elapsed = start.elapsed().as_millis();

                if !status.is_success() {
                    last_err = format!("HTTP {}: {}", status, action).into();
                    warn!(
                        target: "bochk_check::request_log",
                        "REQ#{req_id} HTTP_ERR {} | status={} | attempt={}/{} | {}ms",
                        action,
                        status,
                        attempt,
                        max_retries,
                        elapsed
                    );
                    warn!("← {} | HTTP {} | {}ms | 重试 {}/{}", action, status, elapsed, attempt, max_retries);
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    continue;
                }

                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Err(e) = ensure_business_success(action, body, &json) {
                            warn!(
                                target: "bochk_check::request_log",
                                "REQ#{req_id} BIZ_ERR {} | attempt={}/{} | {}ms | {}",
                                action,
                                attempt,
                                max_retries,
                                elapsed,
                                e
                            );
                            warn!("← {} | 业务错误 | {}ms | {}", action, elapsed, e);
                            return Err(e);
                        }
                        info!(
                            target: "bochk_check::request_log",
                            "REQ#{req_id} OK {} | attempt={}/{} | {}ms",
                            action,
                            attempt,
                            max_retries,
                            elapsed
                        );
                        debug!("← {} | 200 | {}ms | {}", action, elapsed, json);
                        return Ok(json);
                    }
                    Err(e) => {
                        last_err = e.into();
                        warn!(
                            target: "bochk_check::request_log",
                            "REQ#{req_id} JSON_ERR {} | attempt={}/{} | {}ms",
                            action,
                            attempt,
                            max_retries,
                            elapsed
                        );
                        warn!("← {} | JSON 解析失败 | {}ms | 重试 {}/{}", action, elapsed, attempt, max_retries);
                        if attempt < max_retries {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
            }
            Err(e) => {
                let elapsed = start.elapsed().as_millis();
                last_err = e.into();
                warn!(
                    target: "bochk_check::request_log",
                    "REQ#{req_id} REQ_ERR {} | attempt={}/{} | {}ms | {}",
                    action,
                    attempt,
                    max_retries,
                    elapsed,
                    last_err
                );
                warn!("← {} | 请求失败 | {}ms | 重试 {}/{}: {}", action, elapsed, attempt, max_retries, last_err);
                if attempt < max_retries {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        }
    }

    error!(
        target: "bochk_check::request_log",
        "REQ#{req_id} GIVE_UP {} | retries={} | {}",
        action,
        max_retries,
        last_err
    );
    error!("← {} | 全部 {} 次重试失败", action, max_retries);
    Err(last_err)
}

pub fn append_api_log(action: &str, body: &str, response: &serde_json::Value) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let today = Local::now().format("%Y%m%d").to_string();
    let log_path = crate::config::base_dir().join(format!("api_log_{}.jsonl", today));

    let entry = serde_json::json!({
        "ts": Local::now().format("%H:%M:%S").to_string(),
        "action": action,
        "body": body,
        "response": response,
    });

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{}", serde_json::to_string(&entry).unwrap_or_default());
    }
}

pub async fn fetch_date_quota(
    client: &reqwest::Client,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let resp = api_post(client, "jsonAvailableDateAndTime.action", "bean.appDate=").await?;
    append_api_log("jsonAvailableDateAndTime", "bean.appDate=", &resp);
    Ok(resp)
}

pub async fn fetch_time_slots(
    client: &reqwest::Client,
    date: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let body = format!("bean.appDate={}", date);
    let resp = api_post(client, "jsonAvailableDateAndTime.action", &body).await?;
    append_api_log("jsonAvailableDateAndTime(date)", &body, &resp);
    Ok(resp)
}

pub async fn fetch_branches(
    client: &reqwest::Client,
    date: &str,
    time_slot: &str,
    district: &str,
    precondition: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let body = format!(
        "bean.appDate={}&bean.appTime={}&bean.district={}&bean.precondition={}",
        date, time_slot, district, precondition
    );
    let resp = api_post(client, "jsonAvailableBrsByDT.action", &body).await?;
    append_api_log("jsonAvailableBrsByDT", &body, &resp);
    Ok(resp)
}

pub async fn fetch_branch_detail(
    client: &reqwest::Client,
    branch_code: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let req_id = next_request_id();
    let query = format!("bean.branchCode={}", branch_code);
    let url = format!("{}jsonBranchDetail.action?{}", BASE_URL, query);
    let max_retries: u32 = 3;
    let mut last_err: Box<dyn std::error::Error + Send + Sync> = "unknown".into();

    info!(
        target: "bochk_check::request_log",
        "REQ#{req_id} START GET jsonBranchDetail.action | query: {}",
        query
    );

    for attempt in 1..=max_retries {
        let start = std::time::Instant::now();
        let result = client
            .get(&url)
            .header("Referer", REFERER)
            .header("Origin", ORIGIN)
            .header("Accept-Language", ACCEPT_LANGUAGE)
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                let elapsed = start.elapsed().as_millis();

                if !status.is_success() {
                    last_err = format!("HTTP {}: jsonBranchDetail.action", status).into();
                    warn!(
                        target: "bochk_check::request_log",
                        "REQ#{req_id} HTTP_ERR jsonBranchDetail.action | status={} | attempt={}/{} | {}ms",
                        status,
                        attempt,
                        max_retries,
                        elapsed
                    );
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    continue;
                }

                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Err(e) = ensure_business_success("jsonBranchDetail.action", &query, &json) {
                            warn!(
                                target: "bochk_check::request_log",
                                "REQ#{req_id} BIZ_ERR jsonBranchDetail.action | attempt={}/{} | {}ms | {}",
                                attempt,
                                max_retries,
                                elapsed,
                                e
                            );
                            return Err(e);
                        }
                        info!(
                            target: "bochk_check::request_log",
                            "REQ#{req_id} OK jsonBranchDetail.action | attempt={}/{} | {}ms",
                            attempt,
                            max_retries,
                            elapsed
                        );
                        append_api_log("jsonBranchDetail", &query, &json);
                        return Ok(json);
                    }
                    Err(e) => {
                        last_err = e.into();
                        warn!(
                            target: "bochk_check::request_log",
                            "REQ#{req_id} JSON_ERR jsonBranchDetail.action | attempt={}/{} | {}ms",
                            attempt,
                            max_retries,
                            elapsed
                        );
                        if attempt < max_retries {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
            }
            Err(e) => {
                let elapsed = start.elapsed().as_millis();
                last_err = e.into();
                warn!(
                    target: "bochk_check::request_log",
                    "REQ#{req_id} REQ_ERR jsonBranchDetail.action | attempt={}/{} | {}ms | {}",
                    attempt,
                    max_retries,
                    elapsed,
                    last_err
                );
                if attempt < max_retries {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        }
    }

    error!(
        target: "bochk_check::request_log",
        "REQ#{req_id} GIVE_UP jsonBranchDetail.action | retries={} | {}",
        max_retries,
        last_err
    );
    Err(last_err)
}
