use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use tracing::{info, warn};

use crate::client::{
    build_client, fetch_branch_detail, fetch_branches, fetch_date_quota, fetch_time_slots,
    initialize_session,
};
use crate::models::SlotDetail;
use crate::parser::{
    format_date, parse_available_districts, parse_branch_detail, parse_branches, parse_time_slots,
    to_api_date,
};
use crate::state::{load_branch_contacts, upsert_branch_catalog};

pub struct DrillDownResult {
    pub details: Vec<SlotDetail>,
    pub soft_skipped_slots: usize,
}

impl DrillDownResult {
    pub fn is_complete(&self) -> bool {
        self.soft_skipped_slots == 0
    }
}

pub async fn drill_down(proxy_url: &str, available_dates: &[String]) -> DrillDownResult {
    let start = std::time::Instant::now();
    let mut soft_skipped_slots = 0usize;
    let mut slot_map = BTreeMap::<(String, String), SlotDetail>::new();

    let mut date_tasks: FuturesUnordered<_> = available_dates
        .iter()
        .cloned()
        .map(|date| {
            let proxy_url = proxy_url.to_string();
            async move { drill_down_date(&proxy_url, &date).await }
        })
        .collect();

    while let Some((date, details, skipped)) = date_tasks.next().await {
        soft_skipped_slots += skipped;
        for detail in details {
            let key = (date.clone(), detail.time_slot_id.clone());
            slot_map.insert(key, detail);
        }
    }

    info!("第1层全部完成: {}ms，开始汇总结果", start.elapsed().as_millis());

    let all_details: Vec<SlotDetail> = slot_map
        .into_values()
        .filter(|d| !d.branches.is_empty())
        .collect();

    let elapsed = start.elapsed().as_millis();
    let skipped = soft_skipped_slots;
    if skipped > 0 {
        info!(
            "深度查询完成: {} 个可预约时段, {} 个时段未返回区域明细, 耗时 {}ms",
            all_details.len(),
            skipped,
            elapsed
        );
    } else {
        info!(
            "深度查询完成: {} 个可预约时段, 耗时 {}ms",
            all_details.len(),
            elapsed
        );
    }

    DrillDownResult {
        details: all_details,
        soft_skipped_slots,
    }
}

async fn drill_down_date(proxy_url: &str, date: &str) -> (String, Vec<SlotDetail>, usize) {
    let mut slot_map = BTreeMap::<String, SlotDetail>::new();
    let mut branch_meta_cache = BTreeMap::<String, (String, String)>::new();
    let mut soft_skipped_slots = 0usize;

    let client = match build_client(proxy_url) {
        Ok(client) => client,
        Err(e) => {
            warn!("为 {} 创建独立会话失败: {}", format_date(date), e);
            return (date.to_string(), Vec::new(), 0);
        }
    };

    let session_id = match initialize_session(&client).await {
        Ok(session_id) => session_id,
        Err(e) => {
            warn!("为 {} 初始化独立会话失败: {}", format_date(date), e);
            return (date.to_string(), Vec::new(), 0);
        }
    };
    let session_tag = session_id.unwrap_or_else(|| "(未返回)".to_string());
    info!(
        "日期 {} 使用独立会话 JSESSIONID={}",
        format_date(date),
        session_tag
    );

    if let Err(e) = fetch_date_quota(&client).await {
        warn!(
            "会话 {} 查询 {} 顶层 dateQuota 失败: {}",
            session_tag,
            format_date(date),
            e
        );
        return (date.to_string(), Vec::new(), 0);
    }

    let api_date = to_api_date(date);
    let resp = match fetch_time_slots(&client, &api_date).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!(
                "会话 {} 查询 {} 时间段失败: {}",
                session_tag,
                format_date(date),
                e
            );
            return (date.to_string(), Vec::new(), 0);
        }
    };

    let parsed = parse_time_slots(&resp);
    info!("第1层 {} 完成: {} 个可用时段", format_date(date), parsed.len());

    for (slot_id, time_str, _) in parsed {
        let districts = match fetch_districts_with_retry(&client, &api_date, &slot_id).await {
            Ok(districts) => districts,
            Err(e) => {
                let err_text = e.to_string();
                if err_text.contains("业务错误 WHKEQR888") {
                    soft_skipped_slots += 1;
                    info!(
                        "会话 {} 时段 {}/{} 未返回区域明细（WHKEQR888），已跳过",
                        session_tag,
                        format_date(date),
                        slot_id
                    );
                } else {
                    warn!(
                        "会话 {} 查询 {}/{} 区域失败: {}",
                        session_tag, date, slot_id, err_text
                    );
                }
                continue;
            }
        };

        if districts.is_empty() {
            continue;
        }

        let mut branches = Vec::new();
        for (district_key, _) in districts {
            match fetch_branches(&client, &api_date, &slot_id, &district_key, "D").await {
                Ok(resp) => branches.extend(parse_branches(&resp)),
                Err(e) => warn!(
                    "会话 {} 查询 {}/{} 区域 {} 下分行失败: {}",
                    session_tag, date, slot_id, district_key, e
                ),
            }
        }

        if branches.is_empty() {
            continue;
        }

        let mut known_meta = BTreeMap::new();
        let mut missing_codes = Vec::new();
        let branch_codes: Vec<String> = branches.iter().map(|branch| branch.code.clone()).collect();
        let db_meta = match load_branch_contacts(&branch_codes) {
            Ok(meta) => meta,
            Err(e) => {
                warn!("读取分行资料缓存失败: {}", e);
                BTreeMap::new()
            }
        };

        for branch in &branches {
            if let Some(meta) = branch_meta_cache.get(&branch.code) {
                known_meta.insert(branch.code.clone(), meta.clone());
            } else if let Some(meta) = db_meta.get(&branch.code) {
                known_meta.insert(branch.code.clone(), meta.clone());
            } else if !missing_codes.contains(&branch.code) {
                missing_codes.push(branch.code.clone());
            }
        }

        let mut fetched_meta = BTreeMap::new();
        for code in &missing_codes {
            match fetch_branch_detail(&client, code).await {
                Ok(resp) => {
                    fetched_meta.insert(code.clone(), parse_branch_detail(&resp));
                }
                Err(e) => warn!("会话 {} 查询分行详情 {} 失败: {}", session_tag, code, e),
            }
        }

        if !fetched_meta.is_empty() {
            for (code, meta) in &fetched_meta {
                branch_meta_cache.insert(code.clone(), meta.clone());
            }

            let mut refreshed_branches = Vec::new();
            for branch in &branches {
                if let Some((address_cn, tel_no)) = fetched_meta.get(&branch.code) {
                    let mut updated_branch = branch.clone();
                    updated_branch.address_cn = address_cn.clone();
                    updated_branch.tel_no = tel_no.clone();
                    refreshed_branches.push(updated_branch);
                }
            }
            if !refreshed_branches.is_empty() {
                if let Err(e) = upsert_branch_catalog(
                    &refreshed_branches,
                    &chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                ) {
                    warn!("回写分行资料缓存失败: {}", e);
                }
            }
        }

        for branch in &mut branches {
            if let Some((address_cn, tel_no)) = known_meta
                .get(&branch.code)
                .or_else(|| fetched_meta.get(&branch.code))
            {
                branch.address_cn = address_cn.clone();
                branch.tel_no = tel_no.clone();
            }
        }

        slot_map.insert(
            slot_id.clone(),
            SlotDetail {
                date: format_date(date),
                time: time_str,
                time_slot_id: slot_id,
                branches,
            },
        );
    }

    (
        date.to_string(),
        slot_map.into_values().filter(|d| !d.branches.is_empty()).collect(),
        soft_skipped_slots,
    )
}

async fn fetch_districts_with_retry(
    client: &reqwest::Client,
    api_date: &str,
    slot_id: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    match fetch_branches(client, api_date, slot_id, "", "D").await {
        Ok(resp) => Ok(parse_available_districts(&resp)),
        Err(first_err) => {
            if !first_err.to_string().contains("业务错误 WHKEQR888") {
                return Err(first_err);
            }

            warn!(
                "时段 {}/{} 首次获取区域返回 WHKEQR888，250ms 后重试一次",
                api_date, slot_id
            );
            tokio::time::sleep(Duration::from_millis(250)).await;

            let retry_resp = fetch_branches(client, api_date, slot_id, "", "D").await?;
            Ok(parse_available_districts(&retry_resp))
        }
    }
}
