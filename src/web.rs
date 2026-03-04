use tracing::{info, error};

use crate::models::{SharedWebData, WebData};

pub async fn web_index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("web.html"))
}

pub async fn web_api_status(
    axum::extract::State(state): axum::extract::State<SharedWebData>,
) -> axum::Json<WebData> {
    let data = state.read().await;
    axum::Json(data.clone())
}

pub async fn start_web_server(port: u16, state: SharedWebData) {
    let app = axum::Router::new()
        .route("/", axum::routing::get(web_index))
        .route("/api/status", axum::routing::get(web_api_status))
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
