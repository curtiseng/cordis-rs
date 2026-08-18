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

打开 http://127.0.0.1:8787 。界面左侧是 **plugins**（已挂载 fiber 组件）与 **tools**（LLM 可调用函数），中间会话，右侧**工作区**预览（默认 `README.md` markdown 快照；改文件用 read/bash）。

### 左栏 plugins vs tools

| 栏 | 含义 | 例子 |
|---|---|---|
| plugins | 运行时 fiber 组件 | `fs-sandbox`、`tool-fs`、`stats`、`creation-tools` |
| tools | 模型 function call 入口 | `read`/`write`/`edit`（来自 `tool-fs`）、`stats`、`outline` |

### 推荐第一轮

> 用 outline 列出文档结构，用 stats 报字数和标题数，再解释「四种基质」分别是什么。

期望：至少调用 `outline`、`stats` 或 `cite`，左侧 tool trace 可见基质标签。

### 推荐第二轮（bash）

> 在工作区跑 `cargo run -p spatiotemporal-agent -- --smoke`，把输出里 cite 那一段贴给我。

期望：调用 `bash`，cwd 限制在工作区根。

### 推荐第三轮（web_fetch）

> 抓取 https://crates.io/api/v1/crates/spatiotemporal ，告诉我最新版本号。

期望：调用 `web_fetch`，返回 JSON 摘要。

### Session 与工作区

- **工作区**：顶栏下拉切换项目目录（recent 记在启动 cwd 的 `.agent/workspaces.json`）；fs/bash 沙箱随切换更新。
- **会话**：每个工作区独立 `.agent/sessions/`；`{id}.jsonl` 存聊天与工具链路，`{id}.patch.json` 存该会话热装的 patch。
- 切换会话后中间栏 history 与左栏 plugins/tools **自动刷新**（不必整页 reload）；可连续多轮追问「刚才 stats 的结果是多少」，或在左栏切到带/不带热装工具的其它会话做对比。

### 创造模式下的会话 patch

在会话 A 批准 `define_script` 后，仅会话 A 的左栏出现新 tool；**新开会话 B 默认没有**（除非 B 自己再装，或写进了全局 `cordis.yml` / `cordis.patch.yml`）。切回 A 会从 `.patch.json` 恢复。

---

## 2. Smoke（无 API key / CI）

```bash
cargo run -p spatiotemporal-agent -- --smoke
```

- LLM 换成 script **echo**（回复 `echo: …`）
- 界面换成 **probe**（打印插件/工具后退出）
- 仍会跑 `outline`、`cite`、`stats` 等工具链

---

## 3. 编码模式

```bash
cargo run -p spatiotemporal-agent -- --coding
```

或在浏览器点 **「标准 → 编码 → 创造」** 切到编码（`POST /api/mode` `{"profile":"coding"}`）。

- `max_rounds` 见 `cordis.coding.yml`，compaction 限额提高
- 禁用 `outline` / `cite` / `stats` / `read_doc`
- 注入 `assets/CODING.prompt.md`（读→改→测，少重复 read）

---

## 4. 创造模式

```bash
cargo run -p spatiotemporal-agent -- --creation
```

或在浏览器点 **「标准 → 编码 → 创造」** 切到创造（`POST /api/mode` `{"profile":"creation"}`），插件树会热对账，无需重启。

浏览器会多出**待审批**面板。`approval-policy` 默认要求 script / wasm / process / patch 走审批；审计在 `.agent/approvals.jsonl`。

### 4.1 检视运行时

> inspect_tools 和 inspect_plugins 有什么区别？各列几个名字。

### 4.2 试装脚本（需点「批准」）

> 用 define_script 提交 id `demo-hello`：登记工具 hello，参数 JSON `{"name":"..."}`，返回 `Hello, {name}`。

批准后左侧应多一个 script 工具（**仅当前会话**）；拒绝则不会热装。切到其它会话或新开会话，不应看到该 tool，除非它们也有自己的 `.patch.json`。

### 4.3 Patch 试跑（需审批）

> run_patch 试跑一层：insert 一行 disabled cite（id cite），看 inspect_tools 是否少了 cite；然后 revert_patch。

---

## 5. 四种基质对照

| 基质 | demo 中的例子 | 换实现 |
|---|---|---|
| native | `tool-fs`、`deepseek`、`web` | `cordis.smoke.yml` 把 llm/ui 换成 echo/probe |
| wasm | `outline` | 换 guest 文件名，仍用 `name: wasm` |
| script | `cite`、`stats` | `define_script` 或改 `config.file` |
| process | 需自编 guest | `name: process`，`config.command: …` |

**IO 分工**：native 的 `read`/`bash` 负责工作区 IO；script/wasm 叶子默认只 grant `markdown`。叶子内要调其它 tool 用 **`host.callTool` / `call-tool`**，不要 grant `fs`。

子进程 guest 构建：

```bash
./crates/spatiotemporal-process/scripts/build-guests.sh
```

在 `cordis.patch.yml` 里 insert 一行 process（见 `cordis.patch.example.yml`）。

---

## 6. 可选：叠 patch 文件

复制示例并按需修改：

```bash
cp spatiotemporal-agent/cordis.patch.example.yml cordis.patch.yml
cargo run -p spatiotemporal-agent
```

或用创造模式 `reload_patch` / `save_patch`：前者重载 **file 层** `cordis.patch.yml`；后者把**当前会话** patch 导出为 YAML（会话本身已自动存于 `.agent/sessions/*.patch.json`）。

---

## 7. 常见问题

| 现象 | 处理 |
|---|---|
| 编不了 outline.wasm | 跑 `spatiotemporal-agent/scripts/build-guests.sh` |
| 缺 DEEPSEEK_API_KEY | `export` 后重启，或用 `--smoke` |
| 创造模式 define 没生效 | 浏览器点「批准」；看 `.agent/approvals.jsonl` |
| 端口占用 | `PORT=8788 cargo run -p spatiotemporal-agent` |
| 工具调用轮次用尽 | 切编码模式、拆小任务，或调高 `cordis.yml` 的 `max_rounds` |
| LLM 报 tool 消息格式错误 | **新开会话**；旧 session 经 compaction 后可能 orphan tool 消息（已修复，但脏 history 仍建议丢弃） |
| 写 script 工具却要读文件 | 不要 grant fs；LLM 直接 `read`/`bash`，或 script 内 `host.callTool` |
| 想加扫描/统计类能力 | 优先 `bash` 或正式 native 插件，不要为一次性需求写半截插件再热装 |
| 右侧/回复无 Markdown 样式 | 检查网络能否访问 jsDelivr（marked@15、DOMPurify@3）；离线时回退纯文本 |
