use tracing::{info, warn, error};

/// 向多个 Bark 端点发送推送通知
pub async fn send_bark_notifications(
    bark_client: &reqwest::Client,
    bark_urls: &[String],
    title: &str,
    markdown: &str,
) {
    if bark_urls.is_empty() {
        return;
    }
    for bark_url in bark_urls {
        if bark_url.is_empty() {
            continue;
        }
        let url = bark_url.trim_end_matches('/').to_string();
        let payload = serde_json::json!({
            "title": title,
            "markdown": markdown,
            "group": "bochk",
            "sound": "minuet",
            "level": "timeSensitive",
        });
        match bark_client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Bark 通知已发送: {}", mask_url(bark_url));
                } else {
                    warn!(
                        "Bark 通知发送失败: {} ({})",
                        mask_url(bark_url),
                        resp.status()
                    );
                }
            }
            Err(e) => error!("Bark 通知请求失败: {} ({})", mask_url(bark_url), e),
        }
    }
}

/// 对 URL 中的 token 部分进行脱敏处理
pub fn mask_url(url: &str) -> String {
    if let Some(pos) = url.rfind('/') {
        let token = &url[pos + 1..];
        if token.len() > 6 {
            format!(
                "{}{}...{}",
                &url[..pos + 1],
                &token[..3],
                &token[token.len() - 3..]
            )
        } else {
            format!("{}***", &url[..pos + 1])
        }
    } else {
        "***".to_string()
    }
}

/// 对字符串进行百分号编码（RFC 3986 非保留字符不编码）
pub fn urlenc(s: &str) -> String {
    let mut result = String::new();
    for c in s.bytes() {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(c as char)
            }
            _ => result.push_str(&format!("%{:02X}", c)),
        }
    }
    result
}

/// 用“香港 + 地址 + 分行名称”生成地图搜索链接
pub fn build_map_links(name: &str, address_cn: &str) -> (String, String) {
    let query = format!("香港 {} {}", address_cn, name).trim().to_string();
    let encoded = urlenc(&query);
    (
        format!("https://www.google.com/maps/search/?api=1&query={}", encoded),
        format!("https://www.bing.com/maps?q={}", encoded),
    )
}
