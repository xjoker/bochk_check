use tracing::{error, info};

use crate::models::{SharedWebData, WebBranchCatalogEntry, WebData, WebHistoryData};

const HISTORY_PAGE_SIZE: usize = 10;

#[derive(serde::Deserialize, Default)]
pub struct HistoryQuery {
    page: Option<usize>,
}

pub async fn web_index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("web.html"))
}

pub async fn web_api_status(
    axum::extract::State(state): axum::extract::State<SharedWebData>,
) -> axum::Json<WebData> {
    let data = state.read().await;
    axum::Json(data.clone())
}

pub async fn web_api_history(
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> axum::Json<WebHistoryData> {
    let page = query.page.unwrap_or(1).max(1);
    match crate::state::load_web_history(7, page, HISTORY_PAGE_SIZE) {
        Ok(data) => axum::Json(data),
        Err(e) => {
            error!("读取历史数据失败: {}", e);
            axum::Json(WebHistoryData::default())
        }
    }
}

pub async fn web_api_branches() -> axum::Json<Vec<WebBranchCatalogEntry>> {
    match crate::state::load_branch_catalog() {
        Ok(data) => axum::Json(data),
        Err(e) => {
            error!("读取分行目录失败: {}", e);
            axum::Json(Vec::new())
        }
    }
}

pub async fn start_web_server(port: u16, state: SharedWebData) {
    let app = axum::Router::new()
        .route("/", axum::routing::get(web_index))
        .route("/api/status", axum::routing::get(web_api_status))
        .route("/api/history", axum::routing::get(web_api_history))
        .route("/api/branches", axum::routing::get(web_api_branches))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("Web 服务启动: http://0.0.0.0:{}", port);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Web 服务绑定端口 {} 失败: {}", port, e);
            return;
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        error!("Web 服务异常退出: {}", e);
    }
}
