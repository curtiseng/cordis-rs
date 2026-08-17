# Demo 试玩指南

在仓库根目录操作。完整能力需先编 wasm guest：

```bash
./spatiotemporal-agent/scripts/build-guests.sh
export DEEPSEEK_API_KEY=sk-你的key   # 标准模式；smoke 不需要
```

## 1. 标准模式（浏览器）

```bash
cargo run -p spatiotemporal-agent
```

打开 http://127.0.0.1:8787 。界面左侧是**插件树**（基质颜色：native / wasm / script），中间会话，右侧 `assets/sample.md`。

### 推荐第一轮

> 用 outline 列出文档结构，用 stats 报字数和标题数，再解释「四种基质」分别是什么。

期望：至少调用 `outline`、`stats` 或 `cite`，左侧 tool trace 可见基质标签。

### 推荐第二轮（bash）

> 在工作区跑 `cargo run -p spatiotemporal-agent -- --smoke`，把输出里 cite 那一段贴给我。

期望：调用 `bash`，cwd 限制在工作区根。

### 推荐第三轮（web_fetch）

> 抓取 https://crates.io/api/v1/crates/spatiotemporal ，告诉我最新版本号。

期望：调用 `web_fetch`，返回 JSON 摘要。

### Session

刷新页面后会话应恢复（localStorage + `.agent/sessions/`）。可连续多轮追问「刚才 stats 的结果是多少」。

---

## 2. Smoke（无 API key / CI）

```bash
cargo run -p spatiotemporal-agent -- --smoke
```

- LLM 换成 script **echo**（回复 `echo: …`）
- 界面换成 **probe**（打印插件/工具后退出）
- 仍会跑 `outline`、`cite`、`stats` 等工具链

---

## 3. 创造模式

```bash
cargo run -p spatiotemporal-agent -- --creation
```

或在**标准模式**浏览器里点 **「标准 / 创造」** 按钮（`POST /api/mode`），插件树会热对账，无需重启。

浏览器会多出**待审批**面板。`approval-policy` 默认要求 script / wasm / process / patch 走审批；审计在 `.agent/approvals.jsonl`。

### 3.1 检视运行时

> inspect_tools 和 inspect_plugins 有什么区别？各列几个名字。

### 3.2 试装脚本（需点「批准」）

> 用 define_script 提交 id `demo-hello`：登记工具 hello，参数 JSON `{"name":"..."}`，返回 `Hello, {name}`。

批准后左侧应多一个 script 工具；拒绝则不会热装。

### 3.3 Patch 试跑（需审批）

> run_patch 试跑一层：insert 一行 disabled cite（id cite），看 inspect_tools 是否少了 cite；然后 revert_patch。

---

## 4. 四种基质对照

| 基质 | demo 中的例子 | 换实现 |
|---|---|---|
| native | `tool-fs`、`deepseek`、`web` | `cordis.smoke.yml` 把 llm/ui 换成 echo/probe |
| wasm | `outline` | 换 guest 文件名，仍用 `name: wasm` |
| script | `cite`、`stats` | `define_script` 或改 `config.file` |
| process | 需自编 guest | `name: process`，`config.command: …` |

子进程 guest 构建：

```bash
./crates/spatiotemporal-process/scripts/build-guests.sh
```

在 `cordis.patch.yml` 里 insert 一行 process（见 `cordis.patch.example.yml`）。

---

## 5. 可选：叠 patch 文件

复制示例并按需修改：

```bash
cp spatiotemporal-agent/cordis.patch.example.yml cordis.patch.yml
cargo run -p spatiotemporal-agent
```

或用创造模式 `reload_patch` / `save_patch` 持久化动态层。

---

## 6. 常见问题

| 现象 | 处理 |
|---|---|
| 编不了 outline.wasm | 跑 `spatiotemporal-agent/scripts/build-guests.sh` |
| 缺 DEEPSEEK_API_KEY | `export` 后重启，或用 `--smoke` |
| 创造模式 define 没生效 | 浏览器点「批准」；看 `.agent/approvals.jsonl` |
| 端口占用 | `PORT=8788 cargo run -p spatiotemporal-agent` |
