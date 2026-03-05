use chrono::Local;
use std::collections::{BTreeMap, BTreeSet};

use crate::models::{BranchInfo, ChangeEntry, FieldDiff, SlotDetail};

/// 从 dateQuota 响应中提取可预约日期（仅状态为 "A"）
pub fn extract_available_dates(response: &serde_json::Value) -> Vec<String> {
    let mut dates = Vec::new();
    if let Some(quota) = response.get("dateQuota").and_then(|v| v.as_object()) {
        for (date, status) in quota {
            if status.as_str() == Some("A") {
                dates.push(date.clone());
            }
        }
    }
    dates.sort();
    dates
}

/// 将 YYYYMMDD 格式转换为 API 所需的 DD/MM/YYYY 格式
pub fn to_api_date(date: &str) -> String {
    if date.len() == 8 {
        format!("{}/{}/{}", &date[6..8], &date[4..6], &date[0..4])
    } else {
        date.to_string()
    }
}

/// 将 YYYYMMDD 格式转换为可读的 YYYY-MM-DD 格式
pub fn format_date(date: &str) -> String {
    if date.len() == 8 {
        format!("{}-{}-{}", &date[0..4], &date[4..6], &date[6..8])
    } else {
        date.to_string()
    }
}

/// 从 dateTimeQuota 响应中解析可用时段，返回 (slot_id, 时间文本, 状态)
/// 抓包内前端脚本只把 `A` 渲染为可选项；`F` 显示为已满，`D` 直接跳过。
pub fn parse_time_slots(response: &serde_json::Value) -> Vec<(String, String, String)> {
    let mut slots = Vec::new();
    if let Some(dtq) = response.get("dateTimeQuota").and_then(|v| v.as_object()) {
        let mut entries: Vec<_> = dtq.iter().collect();
        entries.sort_by_key(|(k, _)| k.to_string());
        for (key, time_val) in entries {
            let time_str = time_val.as_str().unwrap_or("").to_string();
            if let Some(pos) = key.rfind('_') {
                let slot_id = &key[..pos];
                let status = &key[pos + 1..];
                if status == "A" {
                    slots.push((slot_id.to_string(), time_str, status.to_string()));
                }
            }
        }
    }
    slots
}

/// 从 branchDistrictList 响应中解析有号区域，返回 (区域编码, 中文名)
/// 区域 `value` 末尾状态同样仅 `A` 可继续下钻。
pub fn parse_available_districts(response: &serde_json::Value) -> Vec<(String, String)> {
    let mut districts = Vec::new();
    if let Some(list) = response
        .get("branchDistrictList")
        .and_then(|v| v.as_array())
    {
        for item in list {
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let name_cn = item.get("messageCn").and_then(|v| v.as_str()).unwrap_or("");
            if value.is_empty() || name_cn.is_empty() {
                continue;
            }
            if let Some(pos) = value.rfind('_') {
                let status = &value[pos + 1..];
                let district_key = &value[..pos];
                if status == "A" {
                    districts.push((district_key.to_string(), name_cn.to_string()));
                }
            }
        }
    }
    districts
}

/// 从 availableBranchList 响应中解析可用分行列表
pub fn parse_branches(response: &serde_json::Value) -> Vec<BranchInfo> {
    let mut branches = Vec::new();
    if let Some(list) = response
        .get("availableBranchList")
        .and_then(|v| v.as_array())
    {
        for item in list {
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let name = item
                .get("messageCn")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| item.get("messageHk").and_then(|v| v.as_str()).unwrap_or(""));
            if value.is_empty() || name.is_empty() {
                continue;
            }
            if let Some(pos) = value.rfind('_') {
                let code = &value[..pos];
                let status = &value[pos + 1..];
                if status == "A" {
                    branches.push(BranchInfo {
                        name: name.to_string(),
                        code: code.to_string(),
                        status: status.to_string(),
                        address_cn: String::new(),
                        tel_no: String::new(),
                    });
                }
            }
        }
    }
    branches
}

/// 从 jsonBranchDetail 响应中提取分行中文地址与电话
pub fn parse_branch_detail(response: &serde_json::Value) -> (String, String) {
    let address_cn = response
        .get("addressCn")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tel_no = response
        .get("telNo")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (address_cn, tel_no)
}

/// 递归比较两个 JSON Value，返回所有差异字段
pub fn diff_json(path: &str, old: &serde_json::Value, new: &serde_json::Value) -> Vec<FieldDiff> {
    let mut diffs = Vec::new();
    if old == new {
        return diffs;
    }
    match (old, new) {
        (serde_json::Value::Object(old_map), serde_json::Value::Object(new_map)) => {
            for (key, old_val) in old_map {
                let fp = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                match new_map.get(key) {
                    Some(new_val) => diffs.extend(diff_json(&fp, old_val, new_val)),
                    None => diffs.push(FieldDiff {
                        path: fp,
                        old_value: old_val.clone(),
                        new_value: serde_json::Value::Null,
                    }),
                }
            }
            for (key, new_val) in new_map {
                if !old_map.contains_key(key) {
                    let fp = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    diffs.push(FieldDiff {
                        path: fp,
                        old_value: serde_json::Value::Null,
                        new_value: new_val.clone(),
                    });
                }
            }
        }
        _ => diffs.push(FieldDiff {
            path: path.to_string(),
            old_value: old.clone(),
            new_value: new.clone(),
        }),
    }
    diffs
}

/// 判断当前深度查询结果是否至少覆盖了所有可预约日期
pub fn details_cover_dates(expected_dates: &[String], details: &[SlotDetail]) -> bool {
    let expected: BTreeSet<String> = expected_dates.iter().map(|d| format_date(d)).collect();
    if expected.is_empty() {
        return true;
    }

    let actual: BTreeSet<String> = details.iter().map(|detail| detail.date.clone()).collect();
    expected.into_iter().all(|date| actual.contains(&date))
}

/// 对比两轮完整明细，返回新增可约与失约的“分行-日期-时间点”
pub fn diff_detail_snapshots(
    old_details: &[SlotDetail],
    new_details: &[SlotDetail],
) -> (Vec<SlotDetail>, Vec<SlotDetail>) {
    #[derive(Clone)]
    struct DetailPoint {
        date: String,
        time: String,
        time_slot_id: String,
        branch: BranchInfo,
    }

    fn flatten(details: &[SlotDetail]) -> BTreeMap<(String, String, String, String), DetailPoint> {
        let mut map = BTreeMap::new();
        for slot in details {
            for branch in &slot.branches {
                map.insert(
                    (
                        branch.code.clone(),
                        slot.date.clone(),
                        slot.time.clone(),
                        slot.time_slot_id.clone(),
                    ),
                    DetailPoint {
                        date: slot.date.clone(),
                        time: slot.time.clone(),
                        time_slot_id: slot.time_slot_id.clone(),
                        branch: branch.clone(),
                    },
                );
            }
        }
        map
    }

    fn to_slot_details(points: Vec<DetailPoint>) -> Vec<SlotDetail> {
        points
            .into_iter()
            .map(|point| SlotDetail {
                date: point.date,
                time: point.time,
                time_slot_id: point.time_slot_id,
                branches: vec![point.branch],
            })
            .collect()
    }

    let old_map = flatten(old_details);
    let new_map = flatten(new_details);

    let added = new_map
        .iter()
        .filter(|(key, _)| !old_map.contains_key(*key))
        .map(|(_, point)| point.clone())
        .collect();

    let removed = old_map
        .iter()
        .filter(|(key, _)| !new_map.contains_key(*key))
        .map(|(_, point)| point.clone())
        .collect();

    (to_slot_details(added), to_slot_details(removed))
}

/// 统计当前可预约的具体点位数量（分行 + 日期 + 时间）
pub fn count_detail_points(details: &[SlotDetail]) -> usize {
    details.iter().map(|slot| slot.branches.len()).sum()
}

fn format_compact_details(details: &[SlotDetail]) -> String {
    let mut date_map: BTreeMap<String, BTreeMap<String, BTreeMap<String, usize>>> = BTreeMap::new();

    for slot in details {
        let times = date_map.entry(slot.date.clone()).or_default();
        let branches = times.entry(slot.time.clone()).or_default();
        for branch in &slot.branches {
            *branches.entry(branch.name.clone()).or_insert(0) += 1;
        }
    }

    let mut sections = Vec::new();
    for (date, time_map) in date_map {
        let mut lines = vec![format!("### {}", date)];
        for (time, branches) in time_map {
            for (branch, count) in branches {
                if count > 1 {
                    lines.push(format!("- `{}` {} x{}", time, branch, count));
                } else {
                    lines.push(format!("- `{}` {}", time, branch));
                }
            }
        }
        sections.push(lines.join("\n"));
    }

    sections.join("\n\n")
}

fn format_duration_short(total_secs: u64) -> String {
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else {
        let minutes = total_secs / 60;
        if minutes < 60 {
            format!("{}m", minutes)
        } else {
            let hours = minutes / 60;
            let remain_minutes = minutes % 60;
            if hours < 24 {
                if remain_minutes > 0 {
                    format!("{}h{}m", hours, remain_minutes)
                } else {
                    format!("{}h", hours)
                }
            } else {
                let days = hours / 24;
                let remain_hours = hours % 24;
                if remain_hours > 0 {
                    format!("{}d{}h", days, remain_hours)
                } else {
                    format!("{}d", days)
                }
            }
        }
    }
}

fn format_removed_details(
    details: &[SlotDetail],
    removed_durations: &BTreeMap<(String, String, String, String), u64>,
) -> String {
    let mut date_map: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for slot in details {
        let lines = date_map.entry(slot.date.clone()).or_default();
        for branch in &slot.branches {
            let key = (
                branch.code.clone(),
                slot.date.clone(),
                slot.time.clone(),
                slot.time_slot_id.clone(),
            );
            let duration_text = removed_durations
                .get(&key)
                .map(|secs| format!(" (alive {})", format_duration_short(*secs)))
                .unwrap_or_default();
            lines.push(format!(
                "- `{}` {}{}",
                slot.time, branch.name, duration_text
            ));
        }
    }

    let mut sections = Vec::new();
    for (date, lines) in date_map {
        let mut block = vec![format!("### {}", date)];
        block.extend(lines);
        sections.push(block.join("\n"));
    }

    sections.join("\n\n")
}

fn format_branch_contacts_section(details: &[SlotDetail]) -> String {
    let mut branch_map: BTreeMap<(String, String), (String, String)> = BTreeMap::new();

    for slot in details {
        for branch in &slot.branches {
            let entry = branch_map
                .entry((branch.name.clone(), branch.code.clone()))
                .or_insert_with(|| (branch.address_cn.clone(), branch.tel_no.clone()));
            if entry.0.is_empty() && !branch.address_cn.is_empty() {
                entry.0 = branch.address_cn.clone();
            }
            if entry.1.is_empty() && !branch.tel_no.is_empty() {
                entry.1 = branch.tel_no.clone();
            }
        }
    }

    if branch_map.is_empty() {
        return String::new();
    }

    let mut sections = vec!["### 📍 分行联系信息".to_string()];
    for ((name, _code), (address_cn, tel_no)) in branch_map {
        let google_url = crate::notifier::build_map_link(&name, &address_cn);

        sections.push(format!(
            "#### {}\n- 地址：{}\n- 电话：{}\n- [Google 地图]({})",
            name,
            if address_cn.is_empty() {
                "(暂无)"
            } else {
                &address_cn
            },
            if tel_no.is_empty() {
                "(暂无)"
            } else {
                &tel_no
            },
            google_url
        ));
    }

    sections.join("\n\n")
}

/// 将新增/失约的明细差异格式化为 Bark 文本
pub fn format_detail_change_message(
    added: &[SlotDetail],
    removed: &[SlotDetail],
    removed_durations: &BTreeMap<(String, String, String, String), u64>,
) -> String {
    let mut sections = Vec::new();
    if !added.is_empty() {
        sections.push(format!("## 🟢 新增可预约（{}）", added.len()));
        sections.push(format_compact_details(added));
    }
    if !removed.is_empty() {
        sections.push(format!("## 🔴 已不可预约（{}）", removed.len()));
        sections.push(format_removed_details(removed, removed_durations));
    }

    let contacts = format_branch_contacts_section(added);
    if !contacts.is_empty() {
        sections.push(contacts);
    }

    sections.join("\n\n")
}

/// 将变化记录追加写入当天的 JSONL 日志文件
pub fn append_change_log(
    entry: &ChangeEntry,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::fs::OpenOptions;
    use std::io::Write;
    if !crate::config::persist_jsonl_enabled() {
        return Ok(());
    }
    let today = Local::now().format("%Y%m%d").to_string();
    let log_path = crate::config::log_dir().join(format!("changes_{}.jsonl", today));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{}", line)?;
    Ok(())
}
