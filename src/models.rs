use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(serde::Serialize)]
pub struct ChangeEntry {
    pub timestamp: String,
    pub raw_response: serde_json::Value,
    pub diff: Option<Vec<FieldDiff>>,
    pub available_dates: Vec<String>,
    pub details: Option<Vec<SlotDetail>>,
}

#[derive(serde::Serialize, Clone)]
pub struct SlotDetail {
    pub date: String,
    pub time: String,
    pub time_slot_id: String,
    pub branches: Vec<BranchInfo>,
}

#[derive(serde::Serialize, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub code: String,
    pub status: String,
    pub address_cn: String,
    pub tel_no: String,
}

#[derive(serde::Serialize)]
pub struct FieldDiff {
    pub path: String,
    pub old_value: serde_json::Value,
    pub new_value: serde_json::Value,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebData {
    pub updated_at: String,
    pub last_release_at: String,
    pub monitoring: bool,
    pub total_checks: u64,
    pub date_quota: BTreeMap<String, String>,
    pub dates: Vec<String>,
    pub time_slots: Vec<String>,
    pub branches: Vec<WebBranch>,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebHistoryData {
    pub today: WebHistoryDaySummary,
    pub recent_days: Vec<WebHistoryDaySummary>,
    pub recent_events: Vec<WebHistoryEvent>,
    pub recent_events_pagination: WebPagination,
    pub top_release_branches: Vec<WebBranchReleaseStat>,
    pub top_appointment_times: Vec<WebAppointmentTimeStat>,
    pub top_release_windows: Vec<WebReleaseWindowStat>,
    pub all_release_windows: Vec<WebReleaseBucketStat>,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebHistoryDaySummary {
    pub date: String,
    pub appeared_count: u32,
    pub disappeared_count: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebHistoryEvent {
    pub event_at: String,
    pub event_type: String,
    pub appointment_date: String,
    pub appointment_time: String,
    pub branch_name: String,
    pub duration_secs: u64,
    pub address_cn: String,
    pub tel_no: String,
    pub google_maps_url: String,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebBranchReleaseStat {
    pub branch_name: String,
    pub release_count: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebAppointmentTimeStat {
    pub appointment_time: String,
    pub release_count: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebReleaseWindowStat {
    pub center_time: String,
    pub range_start: String,
    pub range_end: String,
    pub minus_minutes: u32,
    pub plus_minutes: u32,
    pub sample_count: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebReleaseBucketStat {
    pub bucket_label: String,
    pub observed_start: String,
    pub observed_end: String,
    pub sample_count: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebPagination {
    pub page: usize,
    pub page_size: usize,
    pub total_items: usize,
    pub total_pages: usize,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebAvailabilityCell {
    pub status: String,
    pub first_seen_at: String,
}

#[derive(serde::Serialize, Clone)]
pub struct WebBranch {
    pub name: String,
    pub code: String,
    pub address_cn: String,
    pub tel_no: String,
    pub availability: BTreeMap<String, BTreeMap<String, WebAvailabilityCell>>,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct WebBranchCatalogEntry {
    pub code: String,
    pub name: String,
    pub address_cn: String,
    pub tel_no: String,
    pub is_enabled: bool,
    pub updated_at: String,
}

pub type SharedWebData = Arc<RwLock<WebData>>;

pub fn build_web_data(
    details: &[SlotDetail],
    date_quota: &BTreeMap<String, String>,
    total_checks: u64,
    last_release_at: &str,
    first_seen_map: &BTreeMap<(String, String, String), String>,
) -> WebData {
    use std::collections::BTreeSet;

    let mut all_times: BTreeSet<String> = BTreeSet::new();
    let mut branch_map: BTreeMap<
        (String, String),
        (String, String, BTreeMap<String, BTreeMap<String, WebAvailabilityCell>>),
    > = BTreeMap::new();

    for slot in details {
        all_times.insert(slot.time.clone());
        for b in &slot.branches {
            let entry = branch_map
                .entry((b.code.clone(), b.name.clone()))
                .or_insert_with(|| (b.address_cn.clone(), b.tel_no.clone(), BTreeMap::new()));
            if entry.0.is_empty() && !b.address_cn.is_empty() {
                entry.0 = b.address_cn.clone();
            }
            if entry.1.is_empty() && !b.tel_no.is_empty() {
                entry.1 = b.tel_no.clone();
            }
            let date_map = entry.2.entry(slot.date.clone()).or_default();
            date_map.insert(
                slot.time.clone(),
                WebAvailabilityCell {
                    status: b.status.clone(),
                    first_seen_at: first_seen_map
                        .get(&(b.code.clone(), slot.date.clone(), slot.time.clone()))
                        .cloned()
                        .unwrap_or_default(),
                },
            );
        }
    }

    let branches: Vec<WebBranch> = branch_map
        .into_iter()
        .map(|((code, name), (address_cn, tel_no, availability))| WebBranch {
            name,
            code,
            address_cn,
            tel_no,
            availability,
        })
        .collect();

    let dates: Vec<String> = date_quota
        .keys()
        .map(|d| crate::parser::format_date(d))
        .collect();

    WebData {
        updated_at: chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        last_release_at: last_release_at.to_string(),
        monitoring: true,
        total_checks,
        date_quota: date_quota.clone(),
        dates,
        time_slots: all_times.into_iter().collect(),
        branches,
    }
}
