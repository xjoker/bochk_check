# BOCHK 预约监控工具

中银香港（Bank of China Hong Kong）开户预约名额实时监控工具。自动轮询 BOCHK 预约系统 API，检测放号变化，通过 Bark 推送通知，并提供 Web 实时状态页面。

## 功能特性

- **实时监控**：每 10-30 秒轮询 dateQuota 接口，检测可预约日期变化
- **深度查询**：检测到放号后，流式并发查询具体时段、区域、分行（目标 5 秒内完成）
- **Bark 推送**：聚合通知，包含日期变化 + 分行详情，支持多人通知
- **Web 状态页**：内嵌 axum HTTP 服务器，默认端口 32141，10 秒自动刷新
- **异常告警**：连续 3 次失败即告警，含代理地址、耗时等诊断信息
- **跨平台**：macOS / Linux（musl 静态编译）/ Windows 均可运行
- **代理支持**：SOCKS5 代理（`socks5h://` 远端 DNS 解析）
- **API 重试**：每次请求最多 3 次重试，间隔 300ms
- **配置热重载**：每轮检测自动重新读取 config.toml

## 快速开始

```bash
# 1. 克隆 & 编译
git clone <repo-url>
cd bochk_check
cargo build --release

# 2. 配置
cp config.toml.example config.toml
# 编辑 config.toml，填入代理地址和 Bark 推送 URL

# 3. 运行
./target/release/bochk_check

# 4. 访问 Web 状态页
open http://localhost:32141
```

## 配置说明

配置文件：`config.toml`（与可执行文件同目录，或当前工作目录）

```toml
[proxy]
# SOCKS5 代理地址，留空则直连
# 使用 socks5h:// 让代理端做 DNS 解析（推荐）
url = "socks5h://127.0.0.1:1080"

[monitor]
# 请求间隔（秒），建议 10-30
interval_secs = 10

[bark]
# Bark 推送地址列表，支持多人
urls = ["https://api.day.app/your_token_here"]

[web]
# Web 状态页开关和端口（可选，默认启用 32141）
enabled = true
port = 32141
```

## BOCHK API 接口文档

### 基础信息

| 项目 | 值 |
|------|------|
| Base URL | `https://transaction.bochk.com/whk/form/openAccount/` |
| Content-Type | `application/x-www-form-urlencoded; charset=UTF-8` |
| 认证方式 | 无（公开接口） |

### 接口列表

#### 1. 获取日期配额 — `jsonAvailableDateAndTime.action`

查询所有可预约日期及其状态。

**请求参数**：`bean.appDate=`（空值获取全部日期）

**响应字段**：
```json
{
  "dateQuota": {
    "20260305": "A",   // A=Available 可预约
    "20260306": "F",   // F=Full 已满
    "20260310": "F"
  },
  "eaiCode": "SUCCESS"
}
```

#### 2. 获取时间段 — `jsonAvailableDateAndTime.action`

查询指定日期的可用时间段。

**请求参数**：`bean.appDate=DD/MM/YYYY`（如 `05/03/2026`）

**响应字段**：
```json
{
  "dateTimeQuota": {
    "P01_F": "09:00",   // P01=时段ID, F=Full
    "P04_A": "11:15",   // A=Available
    "P05_A": "14:00",
    "P09_D": "17:00"    // D=Disabled 不可选
  }
}
```

时段 ID 格式：`{slot_id}_{status}`，status 取值 A/F/D。

#### 3. 获取区域/分行 — `jsonAvailableBrsByDT.action`

根据日期和时段查询可用区域或分行。

**请求参数**：
```
bean.appDate=DD/MM/YYYY
bean.appTime=P05            # 时段 ID
bean.district=              # 空=返回区域列表; 有值=返回该区分行
bean.precondition=D         # D=日期优先模式
```

**响应（district 为空时 → 区域列表）**：
```json
{
  "branchDistrictList": [
    {
      "messageCn": "西贡区",
      "value": "_sai_kung_district_A"    // _A 后缀=有号
    },
    {
      "messageCn": "中西区",
      "value": "_central_western_district_F"  // _F=无号
    }
  ]
}
```

**响应（district 有值时 → 分行列表）**：
```json
{
  "availableBranchList": [
    {
      "messageCn": "日出康城银行服务中心",
      "value": "952_A"    // 分行代码_状态
    },
    {
      "messageCn": "西贡分行",
      "value": "617_F"
    }
  ]
}
```

### 状态码含义

| 状态码 | 含义 | 说明 |
|--------|------|------|
| `A` | Available | 有号可预约 |
| `F` | Full | 已满 |
| `D` | Disabled | 不可选/不提供服务 |

### API 调用链路

```
dateQuota(空) → 识别有号日期
    ↓ 并发
dateTimeQuota(日期) → 识别有号时段
    ↓ 流式（不等待全部日期）
branchDistrictList(日期+时段, district=空) → 识别有号区域
    ↓ 并发
availableBranchList(日期+时段+区域) → 获取具体分行
```

理论最小 RTT：**3 层**（第1层时段查询返回后立即启动第2层区域→分行流水线）

## Web API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/` | GET | Web 状态页（HTML） |
| `/api/status` | GET | JSON 格式的实时状态数据 |

### `/api/status` 响应示例

```json
{
  "updated_at": "2026-03-04 13:14:00",
  "monitoring": true,
  "total_checks": 42,
  "date_quota": {
    "20260305": "A",
    "20260306": "F"
  },
  "dates": ["2026-03-05", "2026-03-06"],
  "time_slots": ["11:15", "14:00"],
  "branches": [
    {
      "name": "日出康城银行服务中心",
      "code": "952",
      "availability": {
        "2026-03-05": {
          "11:15": "A",
          "14:00": "A"
        }
      }
    }
  ]
}
```

## 项目结构

```
bochk_check/
├── src/
│   ├── main.rs          # 主循环协调（285行）
│   ├── config.rs        # 配置结构体与加载（78行）
│   ├── models.rs        # 数据模型、WebData（103行）
│   ├── client.rs        # HTTP 客户端、API 重试、fetch_*（146行）
│   ├── parser.rs        # JSON 解析、diff、格式化（225行）
│   ├── monitor.rs       # drill_down 深度查询流水线（139行）
│   ├── notifier.rs      # Bark 推送通知（75行）
│   ├── web.rs           # axum Web 服务（35行）
│   └── web.html         # 前端状态页
├── tests/
│   ├── mock_server.rs   # 基于抓包数据的 Mock API Server
│   └── integration_test.rs  # 集成测试（6 个测试用例）
├── config.toml.example  # 配置模板
├── Cargo.toml
└── .gitignore
```

### 模块依赖关系

```
main → config, client, models, monitor, notifier, parser, web
monitor → client, parser, models
parser → models, config
models → parser (format_date)
client → config (base_dir)
web → models
notifier → (独立)
config → (独立)
```

## 通知示例

### 放号通知（聚合）
```
🟢 2026-03-05 出现可预约
🟢 2026-03-09 出现可预约

📅 2026-03-05
  ⏰ 11:15 → 观塘分行, 日出康城银行服务中心
  ⏰ 14:00 → 观塘分行, 日出康城银行服务中心

📅 2026-03-09
  ⏰ 09:45 → 观塘分行, 日出康城银行服务中心
```

### 异常告警
```
⚠️ 监控连续失败 3 次
最后错误: connection timed out
耗时: 10032ms
代理: socks5h://10.0.4.29:7005
已运行: 42分钟
已检查: 15次
```

## 性能指标

| 指标 | 数值 | 说明 |
|------|------|------|
| dateQuota 单次请求 | ~350ms | 含代理延迟 |
| drill_down 全链路 | ~100ms (mock) | 4日期×7时段×2分行 |
| drill_down 实际环境 | <2s (预估) | 含代理网络延迟 |
| Mock 测试 5 轮平均 | 104ms | 30ms/请求模拟延迟 |

## 交叉编译

```bash
# Linux (musl 静态编译)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# Windows
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## 运行日志示例

```
INFO bochk_check: BOCHK 预约监控启动
INFO bochk_check::client: 使用代理: socks5h://10.0.4.29:7005
INFO bochk_check::web: Web 服务启动: http://0.0.0.0:32141
INFO bochk_check: [2026-03-04 13:14:00] 首次获取数据 (372ms)
INFO bochk_check: 当前 dateQuota: {"20260305":"F","20260306":"F"}
INFO bochk_check: [2026-03-04 13:14:30] 无变化 (331ms)
INFO bochk_check: [2026-03-04 13:15:00] 检测到 1 处变化 (fetch: 345ms)
INFO bochk_check::monitor: 第1层 2026-03-05 完成 (89ms): 2 个可用时段
INFO bochk_check::monitor: 深度查询完成: 2 个可预约时段, 耗时 312ms
INFO bochk_check: Bark 通知已发送
```

## 依赖

| 依赖 | 版本 | 用途 |
|------|------|------|
| reqwest | 0.12 | HTTP 客户端（SOCKS5 + rustls） |
| tokio | 1 | 异步运行时 |
| axum | 0.8 | Web 服务框架 |
| serde / serde_json | 1 | JSON 序列化 |
| chrono | 0.4 | 时间处理 |
| tracing | 0.1 | 结构化日志 |
| futures | 0.3 | FuturesUnordered 流式并发 |
| toml | 0.8 | 配置文件解析 |
