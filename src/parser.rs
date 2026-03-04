use chrono::Local;

use crate::config::base_dir;
use crate::models::{BranchInfo, ChangeEntry, FieldDiff, SlotDetail};

/// 从 dateQuota 响应中提取可预约日期（状态非 "F"）
pub fn extract_available_dates(response: &serde_json::Value) -> Vec<String> {
    let mut dates = Vec::new();
    if let Some(quota) = response.get("dateQuota").and_then(|v| v.as_object()) {
        for (date, status) in quota {
            if status.as_str() != Some("F") {
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
                if status != "F" {
                    slots.push((slot_id.to_string(), time_str, status.to_string()));
                }
            }
        }
    }
    slots
}

/// 从 branchDistrictList 响应中解析有号区域，返回 (区域编码, 中文名)
pub fn parse_available_districts(response: &serde_json::Value) -> Vec<(String, String)> {
    let mut districts = Vec::new();
    if let Some(list) = response.get("branchDistrictList").and_then(|v| v.as_array()) {
        for item in list {
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let name_cn = item.get("messageCn").and_then(|v| v.as_str()).unwrap_or("");
            if value.is_empty() || name_cn.is_empty() {
                continue;
            }
            if let Some(pos) = value.rfind('_') {
                let status = &value[pos + 1..];
                let district_key = &value[..pos];
                if status != "F" {
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
    if let Some(list) = response.get("availableBranchList").and_then(|v| v.as_array()) {
        for item in list {
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let name = item
                .get("messageCn")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| {
                    item.get("messageHk")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                });
            if value.is_empty() || name.is_empty() {
                continue;
            }
            if let Some(pos) = value.rfind('_') {
                let code = &value[..pos];
                let status = &value[pos + 1..];
                if status != "F" {
                    branches.push(BranchInfo {
                        name: name.to_string(),
                        code: code.to_string(),
                        status: status.to_string(),
                    });
                }
            }
        }
    }
    branches
}

/// 递归比较两个 JSON Value，返回所有差异字段
pub fn diff_json(
    path: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> Vec<FieldDiff> {
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

/// 将 SlotDetail 列表格式化为可读的通知消息文本
pub fn format_details_message(details: &[SlotDetail]) -> String {
    let mut lines = Vec::new();
    let mut current_date = String::new();
    for slot in details {
        if slot.date != current_date {
            if !current_date.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("\u{1f4c5} {}", slot.date));
            current_date = slot.date.clone();
        }
        let branch_names: Vec<&str> = slot.branches.iter().map(|b| b.name.as_str()).collect();
        lines.push(format!(
            "  \u{23f0} {} \u{2192} {}",
            slot.time,
            branch_names.join(", ")
        ));
    }
    lines.join("\n")
}

/// 将 dateQuota 相关的 FieldDiff 格式化为人类可读的变化描述
pub fn format_date_quota_changes(diffs: &[FieldDiff]) -> Vec<String> {
    let mut messages = Vec::new();
    for d in diffs {
        if d.path.starts_with("dateQuota.") {
            let date = d.path.strip_prefix("dateQuota.").unwrap_or(&d.path);
            let formatted = format_date(date);
            let old_str = d.old_value.as_str().unwrap_or("\u{65e0}");
            let new_str = d.new_value.as_str().unwrap_or("\u{65e0}");
            let label = match (old_str, new_str) {
                ("F", s) if s != "F" => format!("\u{1f7e2} {} \u{51fa}\u{73b0}\u{53ef}\u{9884}\u{7ea6}", formatted),
                (s, "F") if s != "F" => format!("\u{1f534} {} \u{5df2}\u{7ea6}\u{6ee1}", formatted),
                _ => format!("{} : {} \u{2192} {}", formatted, old_str, new_str),
            };
            messages.push(label);
        }
    }
    messages
}

/// 将变化记录追加写入当天的 JSONL 日志文件
pub fn append_change_log(
    entry: &ChangeEntry,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::fs::OpenOptions;
    use std::io::Write;
    let today = Local::now().format("%Y%m%d").to_string();
    let log_path = base_dir().join(format!("changes_{}.jsonl", today));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{}", line)?;
    Ok(())
}
