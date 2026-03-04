# BOCHK 项目补充说明

本文件是 `README.md` 的补充，面向协作开发与维护场景，记录当前项目的真实实现状态、关键约束与已确认的接口结论。内容以仓库现状为准。

## 项目定位

`bochk_check` 是一个 Rust 编写的 BOCHK 开户预约监控工具，用于：

- 轮询 BOCHK 预约接口
- 检测 `dateQuota` 变化
- 在存在可预约日期时继续深度查询时段、区域、分行
- 通过 Bark 发送通知
- 通过本地 Web 页面展示实时状态

## 当前实现概览

- 每轮检测都会重新加载 `config.toml`
- 每个 `interval` 都会新建一个独立 `reqwest::Client`
- 每轮都会先请求 `continueInput.action` 初始化新会话
- 因此每轮都会使用新的 `JSESSIONID`
- 仅状态为 `A` 的日期、时段、区域、分行会进入下一层请求
- 只要当前存在 `A` 日期，每轮都会执行深度查询
- 深度查询命中分行后，会额外调用 `jsonBranchDetail.action` 补全地址和电话

## 已确认的接口语义

这些结论来自当前仓库内完整抓包与前端页面脚本，不再只是样本推断：

- `A`：可预约，可继续下钻
- `F`：已满；前端会显示为禁用项并标记“已满”
- `D`：不可选；前端不会把它渲染为可选项
- `WHKEQR888`：业务失败，前端提示为“操作逾时，请重新提交”
- `precondition=D`：日期优先模式
- `precondition=B`：分行优先模式

## 当前主链路

当前程序使用“日期优先”路径：

```text
continueInput.action
  -> jsonAvailableDateAndTime.action (bean.appDate=)
  -> jsonAvailableDateAndTime.action (bean.appDate=DD/MM/YYYY)
  -> jsonAvailableBrsByDT.action (date + time + district=空 + precondition=D)
  -> jsonAvailableBrsByDT.action (date + time + district + precondition=D)
  -> jsonBranchDetail.action (bean.branchCode=...)
```

其中：

- 第一步负责建立当前轮次的会话
- 第二步获取全局 `dateQuota`
- 第三步获取单日 `dateTimeQuota`
- 第四步获取可用区域
- 第五步获取区域内可用分行
- 第六步补充分行地址与电话

## 配置现状

配置文件 `config.toml` 是可选的。程序配置优先级如下：

1. `BOCHK_*` 环境变量
2. `config.toml`
3. 代码内默认值

当前默认值：

```toml
[proxy]
url = ""

[monitor]
interval_secs = 30
max_fail_count = 5

[bark]
urls = []

[logging]
persist_jsonl = false

[web]
enabled = true
port = 32141
```

关键点：

- `proxy.url` 为空表示直连
- `bark.urls` 为空表示关闭推送
- `logging.persist_jsonl=false` 时，不额外写 JSONL 文件
- `monitor.max_fail_count` 已正式接入异常告警阈值

## 日志与调试

默认行为：

- 运行日志通过 `tracing` 输出到 `stderr`
- 请求级追踪日志输出到 `bochk_check::request_log`
- 不会额外写文件日志

调试模式：

- 若设置 `logging.persist_jsonl=true`
- 程序会在基准目录写入：
  - `api_log_YYYYMMDD.jsonl`
  - `changes_YYYYMMDD.jsonl`

这两个文件主要用于抓取原始响应和变化快照，便于排查接口行为。

## Bark 与通知

当前通知策略：

- 首次启动且发现可预约日期时立即通知
- `dateQuota` 发生变化时通知
- 连续失败达到 `monitor.max_fail_count` 时触发异常告警
- 之后每额外增加 10 次失败再重复告警

当前详情通知格式：

```text
分行
  -> 日期
    -> 可预约时间
```

每个分行会附带：

- `addressCn`
- `telNo`

## Web 页面现状

- 默认监听 `32141`
- 提供 `/` 和 `/api/status`
- 页面支持桌面和手机端
- 已移除地图跳转链接
- 当前直接展示分行名称、地址、电话和可预约情况

需要注意：

- `web.enabled` / `web.port` 仅在启动时读取
- 运行中修改这两项配置不会自动重启 Web 服务

## 当前目录结构

```text
bochk_check/
├── src/
│   ├── main.rs
│   ├── config.rs
│   ├── models.rs
│   ├── client.rs
│   ├── parser.rs
│   ├── monitor.rs
│   ├── notifier.rs
│   ├── web.rs
│   └── web.html
├── config.toml.example
├── Cargo.toml
├── README.md
└── AGENTS.md
```

说明：

- 仓库当前不包含 `tests/` 目录
- 仓库当前未提供 `Dockerfile`
- `temp/` 仅作为人工分析抓包的临时资料，不属于正式运行依赖

## 当前已知限制

- Web 服务相关配置不支持运行中热生效
- 若启用 `logging.persist_jsonl`，JSONL 文件会直接写到基准目录
- 当前仍未提供 Docker 化部署文件

## 维护建议

- 调整接口判定逻辑前，优先对照抓包里的前端脚本
- 更新 `README.md` 时，同步检查本文件是否仍与代码一致
- 若继续公开发布，优先补齐部署说明与日志策略说明
