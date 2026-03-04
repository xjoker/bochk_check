use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::future::join_all;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::client::{fetch_branches, fetch_time_slots};
use crate::models::SlotDetail;
use crate::parser::{format_date, parse_available_districts, parse_branches, parse_time_slots, to_api_date};

pub async fn drill_down(
    client: &reqwest::Client,
    available_dates: &[String],
) -> Vec<SlotDetail> {
    let start = std::time::Instant::now();

    let slot_map = Arc::new(Mutex::new(
        BTreeMap::<(String, String), SlotDetail>::new(),
    ));

    let mut time_stream: FuturesUnordered<_> = available_dates
        .iter()
        .map(|date| {
            let api_date = to_api_date(date);
            let client = client.clone();
            let date_owned = date.clone();
            async move {
                let result = fetch_time_slots(&client, &api_date).await;
                (date_owned, result)
            }
        })
        .collect();

    let mut layer2_futs: FuturesUnordered<tokio::task::JoinHandle<()>> = FuturesUnordered::new();

    while let Some((date, result)) = time_stream.next().await {
        match result {
            Ok(resp) => {
                let parsed = parse_time_slots(&resp);
                info!(
                    "第1层 {} 完成 ({}ms): {} 个可用时段",
                    format_date(&date),
                    start.elapsed().as_millis(),
                    parsed.len()
                );

                for (slot_id, time_str, _) in parsed {
                    let client = client.clone();
                    let api_date = to_api_date(&date);
                    let date_raw = date.clone();
                    let slot_map = slot_map.clone();

                    layer2_futs.push(tokio::spawn(async move {
                        let districts = match fetch_branches(&client, &api_date, &slot_id, "", "D").await {
                            Ok(resp) => {
                                let d = parse_available_districts(&resp);
                                d
                            }
                            Err(e) => {
                                warn!("查询 {}/{} 区域失败: {}", date_raw, slot_id, e);
                                return;
                            }
                        };

                        if districts.is_empty() {
                            return;
                        }

                        let branch_futs: Vec<_> = districts
                            .iter()
                            .map(|(dk, _)| {
                                let client = client.clone();
                                let api_date = api_date.clone();
                                let slot_id = slot_id.clone();
                                let dk = dk.clone();
                                async move {
                                    fetch_branches(&client, &api_date, &slot_id, &dk, "D").await
                                }
                            })
                            .collect();

                        let branch_results = join_all(branch_futs).await;

                        let mut branches = Vec::new();
                        for result in branch_results {
                            if let Ok(resp) = result {
                                branches.extend(parse_branches(&resp));
                            }
                        }

                        if !branches.is_empty() {
                            let key = (date_raw.clone(), slot_id.clone());
                            let mut map = slot_map.lock().await;
                            let detail = map.entry(key).or_insert_with(|| SlotDetail {
                                date: format_date(&date_raw),
                                time: time_str.clone(),
                                time_slot_id: slot_id.clone(),
                                branches: Vec::new(),
                            });
                            detail.branches.extend(branches);
                        }
                    }));
                }
            }
            Err(e) => warn!("查询 {} 时间段失败: {}", format_date(&date), e),
        }
    }

    info!("第1层全部完成: {}ms，等待第2层流水线", start.elapsed().as_millis());

    while let Some(result) = layer2_futs.next().await {
        if let Err(e) = result {
            warn!("第2层任务 panic: {}", e);
        }
    }

    let inner_map = match Arc::try_unwrap(slot_map) {
        Ok(mutex) => mutex.into_inner(),
        Err(arc) => {
            let guard = arc.lock().await;
            guard.clone()
        }
    };

    let all_details: Vec<SlotDetail> = inner_map
        .into_values()
        .filter(|d| !d.branches.is_empty())
        .collect();

    let elapsed = start.elapsed().as_millis();
    info!(
        "深度查询完成: {} 个可预约时段, 耗时 {}ms",
        all_details.len(),
        elapsed
    );

    all_details
}
