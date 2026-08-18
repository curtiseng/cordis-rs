# Spatiotemporal Agent

插件化 agent harness：在浏览器里多轮对话、调工具；**创造模式**可审批热装 script 叶子（如 **code-stats**），热装绑定会话并持久化到 `.agent/sessions/`。宿主几乎只做两件事：建一张插件注册表，再用 `Loader` 把 `cordis.yml` 对账成一棵 fiber 树。

![创造模式：会话热装 code-stats，统计本仓库代码行数；左侧插件/工具树，右侧工作区目录](assets/ui.jpg)

## 一眼能做什么

典型试玩（见 [`DEMO.md`](DEMO.md)）：

1. 浏览器切到 **创造** 模式，新开会话。
2. 让 agent 用 `define_script` 安装 `code-stats`（脚本已在 `plugins/generated/code-stats.js`），在界面点 **批准**。
3. 发送「统计代码行数」——agent 通过 `host.callTool` 编排 `bash` / `read`，返回按扩展名分组的行数、注释占比等（见上图）。

`code-stats` **只存在于当前会话**；新开会话需重新安装，或 `save_patch` 导出到 `cordis.patch.yml`。

## 启动

### 1. 准备工具链

需要 Rust **1.94+**（wasmtime 的 MSRV），以及 wasm guest 目标：

```bash
rustup toolchain install stable
rustup target add wasm32-wasip2
```

在仓库根目录。

### 2. 编 wasm 插件

`outline` 是 wasm 叶子工具，产物不入库，每次换机器或清 `target/` 都要编一次：

```bash
./spatiotemporal-agent/scripts/build-guests.sh
```

成功后应看到 `spatiotemporal-agent/target/guests/outline.wasm`。缺这一步，装配 `outline` 那一行会直接失败并提示你跑这个脚本。

### 3. 准备 API Key（不要写进仓库）

到 [DeepSeek 开放平台](https://platform.deepseek.com/) 建一把 key。只放进**当前 shell 的环境变量**，不要写进 `cordis.yml`、不要写进 README、不要提交 `.env`。

```bash
export DEEPSEEK_API_KEY=sk-你的key
```

可选：

| 变量 | 默认 | 作用 |
|---|---|---|
| `DEEPSEEK_API_KEY` | （必填，对话时） | Bearer token |
| `DEEPSEEK_BASE_URL` | `https://api.deepseek.com` | 网关 |
| `DEEPSEEK_MODEL` | `deepseek-chat` | 模型名 |
| `PORT` | `8787`（也可写在 `cordis.yml` 的 `ui.config.port`） | 网页端口 |

缺 key 时进程仍能起来，左侧会标「缺 DEEPSEEK_API_KEY」，发消息会得到明确错误，而不是静默失败。

### 4. 启动

仍在仓库根目录：

```bash
cargo run -p spatiotemporal-agent
```

**编码模式**（写代码任务，更多 tool 轮次，注入 `CODING.prompt.md`；轮次见 `cordis.coding.yml`）：

```bash
cargo run -p spatiotemporal-agent -- --coding
# 或浏览器点「标准 → 编码 → 创造」
```

编码模式禁用 demo 文档类 tool，避免分散轮次。改仓库代码时**优先从编码模式启动**。

也可 `POST /api/mode`：`{"profile": "coding"}` 或 `{"profile": "creation"}`（三档互斥）。

**创造模式**（可运行时切换，不必重启）：

```bash
cargo run -p spatiotemporal-agent -- --creation   # 启动即创造模式
# 或标准模式启动后，在浏览器点「标准 / 创造」切换
```

也可 `POST /api/mode`：`{"profile": "creation"}` 或 `{"creation": true}`。

## GitHub Release（开箱即用）

无需本地 Rust 工具链：在 [Releases](https://github.com/zifeng-yang/spatiotemporal/releases) 下载对应平台的 `spatiotemporal-agent-{version}-{target}.tar.gz`（Windows 为 `.zip`），解压后：

```bash
export DEEPSEEK_API_KEY=sk-你的key
cd /path/to/your/project          # 工作区 = 启动时的 cwd
/path/to/spatiotemporal-agent     # 编码：--coding；创造：--creation
```

包内已含 `cordis.yml`、`assets/`、`plugins/`、`outline.wasm` 等运行时资源；二进制会自动在**同目录**（或 `bin/` 的上一级）查找这些文件。也可用 `SPATIOTEMPORAL_AGENT_HOME` 指向资源目录。

**维护者发版**：打 tag 触发 CI（例如 `git tag v0.5.0 && git push origin v0.5.0`）。本地打包：

```bash
./spatiotemporal-agent/scripts/build-guests.sh
cargo build -p spatiotemporal-agent --release
./spatiotemporal-agent/scripts/package-release.sh
```

产物在 `spatiotemporal-agent/dist/`。

看到 `打开 http://127.0.0.1:8787` 后，用浏览器打开。默认以**工作区根**（启动时的 cwd；`WORKSPACE` 可覆盖）为 fs/bash 沙箱；右侧为**工作区目录树**。`outline` / `cite` / `stats` 等 demo 工具读工作区 `README.md` 快照；`AGENTS.md` 注入 system prompt。界面底部有**试玩按钮**；完整脚本见 [`DEMO.md`](DEMO.md)。Markdown 渲染依赖 jsDelivr（marked@15 / DOMPurify@3；离线回退纯文本）。

**左栏**：**plugins** = 已挂载 fiber 组件；**tools** = LLM 可调用的函数（native 常一对多，script/wasm 叶子常与插件同名）。完整配置树用创造模式 `inspect_config` 查看。

多轮对话会保留 tool 消息，模型能记住之前调用了哪些工具。左栏可**新开会话**、切换历史会话、悬停后点 **×** 删除（`DELETE /api/session`）。`agent-loop` 会在上下文过长时按 `cordis.yml` 的 `compaction` 配置压缩 history（截断 tool 输出、保留最近 N 条）。

## 工作区与会话

### 工作区

- **初始根**：`cargo run` 时的 cwd，或环境变量 `WORKSPACE`。
- **运行时切换**：浏览器顶栏工作区下拉（最近 12 个目录）或 **+** 输入绝对路径；`GET /api/workspaces`、`POST /api/workspaces/switch`。
- **持久化**：在**启动时的工作区**下写 `.agent/workspaces.json`（`current` + `recent`）。
- **沙箱**：`read` / `write` / `edit` / `bash` 的根目录随当前工作区热切换，无需重启进程。

### 多会话（按工作区隔离）

每个工作区有自己的 `.agent/sessions/`：

| 文件 | 内容 |
|---|---|
| `{id}.jsonl` | 聊天消息 + `turn_steps` 等事件；刷新或切换会话时重建中间栏 UI |
| `{id}.patch.json` | 该会话绑定的 cordis patch **层栈**（创造模式热装、审批通过后的 script/wasm 等） |

浏览器 `localStorage` 按工作区记当前 session id：`spatiotemporal-session:{workspacePath}`。

切换会话时服务端 `activate_session`：从磁盘加载对应 `.patch.json`，对账 fiber 树；左栏 **plugins / tools** 随当前会话自动刷新。

### 配置层叠与隔离边界

运行时 `AgentRuntime` 按顺序叠层并对账：

```
bootstrap → profile（cordis.coding.yml / cordis.creation.yml）→ cordis.patch.yml → 当前会话 patch
```

| 层 | 作用域 | 典型来源 |
|---|---|---|
| `cordis.yml` | **全局**（所有工作区、所有会话） | 仓库基础组合 |
| profile / `cordis.patch.yml` | **进程**（切换 profile 或 reload 影响全部会话的基底） | `--coding` / `--creation`、文件 patch |
| **会话 patch** | **仅当前会话** | `define_script` 审批通过、`run_patch`、`push_layer` |

要点：

- 创造模式 **`define_script` / `run_patch` 必须已有激活会话**；审批项带 `session_id`，批准时先切到该会话再热装。
- **`push_layer` / `pop_layer` 只改当前会话**，并自动写回 `{id}.patch.json`。
- **不要把会话级工具写进 `cordis.yml`**（会污染所有新会话）；试验叶子用 `define_script` + 审批，或 `save_patch` 导出到 `cordis.patch.yml` 作可选文件层。
- `save_patch` 导出的是**当前会话** patch 栈的扁平 YAML，不是旧版「全局 dynamic 层」。

### 相关 HTTP API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/workspaces` | 当前工作区与 recent 列表 |
| POST | `/api/workspaces/switch` | `{"path": "/abs/path"}` |
| GET | `/api/sessions` | 当前工作区下的会话摘要 |
| POST | `/api/session` | 新建会话并激活 |
| GET | `/api/session?session_id=…` | 加载 history/events，并 **activate_session** |
| DELETE | `/api/session?session_id=…` | 删除 jsonl + patch.json |
| GET | `/api/status` | 含 `active_session`、当前 runtime 的 plugins/tools |

在工作区根目录放 `AGENTS.md` 可注入项目级指令；`system-prompt` 插件还会把各工具的 JSON schema 写进 system prompt。

可选：启动时传入路径可改 markdown 快照（仅影响 `outline` / `cite` / `stats` / `read_doc`）：

```bash
cargo run -p spatiotemporal-agent -- docs/paper.md
```

### 5. 不调网络的自检

没有 key、或不想打真实 API 时：

```bash
cargo run -p spatiotemporal-agent -- --smoke
```

这会叠一层 `cordis.smoke.yml`：DeepSeek 换成脚本 `echo`，Web 换成命令行 `probe`。它会打印已装插件、真的跑一遍 `outline` 和 `cite`，并对 echo 模型 `ping` 一次，然后退出。CI 用的就是这个。

## 这棵树里有什么

`cordis.yml` 每一行是一项能力，跟 dsh 同形。换实现不是改 `name`（那是断言），而是关掉旧行再 `insert` 新行。

| id | name | 基质 | 提供 |
|---|---|---|---|
| `fs-sandbox` | `fs-sandbox` | native | `fs` 工作区沙箱 |
| `tool-fs` | `tool-fs` | native | 工具 `read` / `write` / `edit` |
| `bash-sandbox` | `bash-sandbox` | native | `shell` 工作区沙箱 |
| `tool-bash` | `tool-bash` | native | 工具 `bash` |
| `tool-web-fetch` | `tool-web-fetch` | native | 工具 `web_fetch` |
| `system-prompt` | `system-prompt` | native | `system-prompt`（AGENTS.md + schema） |
| `agent-loop` | `agent-loop` | native | `agent-loop`（可插拔多轮循环） |
| `doc` | `doc` | native | `markdown` 能力 |
| `read-doc` | `read-doc` | native | 工具 `read_doc` |
| `outline` | `wasm` | wasm | 工具 `outline` |
| `cite` | `script` | script | 工具 `cite` |
| `stats` | `script` | script | 工具 `stats`（字数/标题统计） |
| `llm` | `deepseek` | native | `llm`（HTTP，不进 wasm） |
| `ui` | `web` | native | `surface`（听端口） |
| `creation` | `creation-tools` | native | 创造模式元工具（`--creation` 时启用） |

创造模式额外提供：`inspect_plugins` / `inspect_tools` / `inspect_config` / `define_plugin`（script / wasm / 已有 native）/ `define_script`（`define_plugin` 别名）/ `run_patch` / `revert_patch` / `reload_patch` / `undefine_plugin` / `save_patch`。

`--creation` 还会加载 `cordis.creation.yml`：启用 `approval-policy`（script/wasm/process/patch 需审批，审计写入 `.agent/approvals.jsonl`）、`patch-watcher` 与 `creation-tools`。动态 patch 分层：启动时读 `cordis.patch.yml` 作为 file 层；**会话级**热装持久化到 `.agent/sessions/{id}.patch.json`；`save_patch` 可把当前会话 patch 导出为 `cordis.patch.yml`。

wasm / script 适合叶子工具：调用稀疏、payload 小、能力面由 WIT / `host.*` 钉死。LLM 适配器和听端口的界面留在 native——前者每次要搬整份对话且需要 HTTP，后者 WASI 里没有「绑 8787」。

**Tool 桥接**：script guest 可 `host.callTool(name, argsJson)`，wasm guest 可 WIT `call-tool`，走与 LLM 相同的宿主 `Toolbox`。demo 里 script/wasm 通常只 `grant: [markdown]`；需要读文件时由 LLM 直接调 `read`/`bash`，或在 define_script 的叶子代码里 `callTool`，**不要** grant `fs`/`shell`。

## 常见问题

**`编不了 …/outline.wasm`**  
还没跑 `scripts/build-guests.sh`，或缺 `wasm32-wasip2`。

**页面提示缺 key / 回复是「没有 DEEPSEEK_API_KEY」**  
当前 shell 没有导出该变量。`export` 过再重新 `cargo run`。不要把 key 写进 yaml。

**端口被占**  
`PORT=8788 cargo run -p spatiotemporal-agent`，或改 `cordis.yml` 里 `ui.config.port`。

**只想确认插件装上了**  
`--smoke`，不听端口。

**LLM 报 tool 消息格式错误**  
多为长会话 history 脏（compaction 后 orphan tool 消息）。**新开会话**再试；编码任务用 `--coding`。

**写 script 工具却要读工作区文件**  
不要 grant `fs`；LLM 直接 `read`/`bash`，或 script 内 `host.callTool("read", json)`。

## Demo 试玩

详见 [`DEMO.md`](DEMO.md)：**创造模式安装 code-stats 并统计代码**（推荐）、标准模式 outline/bash/web_fetch、smoke、编码模式。可选 `cordis.patch.example.yml` 演示 file 层 patch。
