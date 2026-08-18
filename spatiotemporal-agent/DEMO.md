# Demo 试玩指南

在仓库根目录操作。完整能力需先编 wasm guest：

```bash
./spatiotemporal-agent/scripts/build-guests.sh
export DEEPSEEK_API_KEY=sk-你的key   # 标准 / 创造；smoke 不需要
```

## 1. 推荐 Demo：创造模式 + code-stats（见 README 截图）

```bash
cargo run -p spatiotemporal-agent -- --creation
```

打开 http://127.0.0.1:8787 。**新开会话**，确认左栏模式为 **创造**。

### 1.1 安装 code-stats（需点「批准」）

> 用 define_script 安装 id `code-stats`：脚本文件 `plugins/generated/code-stats.js`，登记工具 `code-stats`，参数 JSON 含 `path`、`exts`、`mode`；内部用 host.callTool 调 bash 和 read。等我批准。

批准后左栏 **plugins / tools** 应出现 `code-stats`（**仅当前会话**）。

### 1.2 统计代码行数

> 统计代码行数

或更具体：

> 用 code-stats 统计整个工作区，按扩展名分组，给出总行数、代码/注释/空行占比。

期望：中间栏出现多步 tool 链路（`bash`、`read`、`code-stats`…），回复含总览表与按 `.rs` 等扩展名分组的明细——与 [`assets/ui.jpg`](assets/ui.jpg) 同类结果。

### 1.3 会话隔离（可选）

- **新开会话**：左栏不应再有 `code-stats`。
- **切回安装会话**：从 `.agent/sessions/{id}.patch.json` 恢复，`code-stats` 仍在。

---

## 2. 标准模式（outline / bash / web）

```bash
cargo run -p spatiotemporal-agent
```

界面左侧 **plugins**（fiber 组件）与 **tools**（LLM 可调用函数）；中间会话；右侧**工作区目录树**。`outline` / `cite` / `stats` 读工作区 `README.md` 快照。

### 推荐第一轮

> 用 outline 列出 README 结构，用 stats 报字数和标题数，再解释「四种基质」分别是什么。

### 推荐第二轮（bash）

> 在工作区跑 `cargo run -p spatiotemporal-agent -- --smoke`，把输出里 cite 那一段贴给我。

### 推荐第三轮（web_fetch）

> 抓取 https://crates.io/api/v1/crates/spatiotemporal ，告诉我最新版本号。

### Session 与工作区

- **工作区**：顶栏下拉切换项目目录（recent 记在 `.agent/workspaces.json`）。
- **会话**：`{id}.jsonl` 存聊天与工具链路，`{id}.patch.json` 存热装 patch；切换会话自动刷新左栏 runtime。

---

## 3. Smoke（无 API key / CI）

```bash
cargo run -p spatiotemporal-agent -- --smoke
```

- LLM 换成 script **echo**（回复 `echo: …`）
- 界面换成 **probe**（打印插件/工具后退出）
- 仍会跑 `outline`、`cite`、`stats` 等工具链

---

## 4. 编码模式

```bash
cargo run -p spatiotemporal-agent -- --coding
```

- `max_rounds` 见 `cordis.coding.yml`，compaction 限额提高
- 禁用 `outline` / `cite` / `stats` / `read_doc`
- 注入 `assets/CODING.prompt.md`（读→改→测，少重复 read）

---

## 5. 创造模式其它试玩

浏览器 **待审批** 面板：`approval-policy` 默认 script / wasm / process / patch 需审批；审计在 `.agent/approvals.jsonl`。

### inspect

> inspect_tools 和 inspect_plugins 有什么区别？各列几个名字。

### 小型 script 试装

> 用 define_script 提交 id `demo-hello`：工具 hello，参数 `{"name":"..."}`，返回 `Hello, {name}`。

### patch 试跑

> run_patch 试跑一层：disabled cite（id cite），看 inspect_tools 是否少了 cite；然后 revert_patch。

---

## 6. 四种基质对照

| 基质 | demo 中的例子 | 换实现 |
|---|---|---|
| native | `tool-fs`、`deepseek`、`web` | `cordis.smoke.yml` 把 llm/ui 换成 echo/probe |
| wasm | `outline` | 换 guest 文件名，仍用 `name: wasm` |
| script | `cite`、`stats`、**`code-stats`（创造热装）** | `define_script` 或改 `config.file` |
| process | 需自编 guest | `name: process`，`config.command: …` |

**IO 分工**：native 的 `read`/`bash` 负责工作区 IO；script/wasm 叶子默认只 `grant: [markdown]` 或不 grant。**code-stats** 不 grant fs，在脚本内 `host.callTool("bash"|"read", …)`。

---

## 7. 可选：叠 patch 文件

```bash
cp spatiotemporal-agent/cordis.patch.example.yml cordis.patch.yml
cargo run -p spatiotemporal-agent
```

`save_patch` 可把**当前会话** patch 导出为 YAML（会话已自动存于 `.agent/sessions/*.patch.json`）。

---

## 8. 常见问题

| 现象 | 处理 |
|---|---|
| 编不了 outline.wasm | 跑 `spatiotemporal-agent/scripts/build-guests.sh` |
| 缺 DEEPSEEK_API_KEY | `export` 后重启，或用 `--smoke` |
| define 后左栏没 code-stats | 浏览器点「批准」；确认未切到新会话 |
| 新会话又有 code-stats | 检查是否写进了全局 `cordis.yml`（应只用会话 patch） |
| 端口占用 | `PORT=8788 cargo run -p spatiotemporal-agent` |
| LLM 报 tool 消息格式错误 | **新开会话** |
| 写 script 却要读文件 | 不要 grant fs；用 `host.callTool("read", …)` |
