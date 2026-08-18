# AGENTS.md

Spatiotemporal 是论文 [*A Programming Paradigm for Spatiotemporal Composability*](https://github.com/cordiverse/paper) 的 Rust 演算实现；`spatiotemporal-agent/` 是其上的插件化 agent harness，形态对齐 [DeepSeek Harness (dsh)](https://github.com/deepseek-ai/deepseek-harness)：**一切都是插件**。

改内核前读根 [README.md](README.md) 的「与论文的对应」和「Rust 里的七个设计决定」；改 agent 前读 [spatiotemporal-agent/README.md](spatiotemporal-agent/README.md)。

---

## 本文件的两种用途

1. **给编码 agent（Cursor 等）**：改这个仓库时遵守下面的布局、命令与约定。
2. **给运行中的 spatiotemporal-agent**：`system-prompt` 插件会把工作区根目录的 `AGENTS.md`（默认文件名，见 `cordis.yml`）注入 model 的 system prompt。在仓库根启动 agent 时，本节与下文「运行中 agent 的行为」会一并生效。

---

## 仓库布局

```
src/                      演算内核（Context、Loader、fiber、Steps、compose）
tests/                    内核与配置层测试（对着论文定理写）
examples/                 swap_provider、watch_config
crates/
  spatiotemporal-wasm/    wasm 基质适配器 + WIT + guest 测试
  spatiotemporal-script/  QuickJS 脚本基质适配器
  spatiotemporal-process/ 子进程（NDJSON stdio）基质适配器
spatiotemporal-agent/     agent harness（宿主 + cordis.yml + 浏览器 UI）
  src/
    host.rs               Toolbox、Llm、Surface、AgentLoop、SystemPrompt
    registry.rs           插件注册表（name → 工厂）
    runtime.rs            AgentRuntime（热对账，创造模式）
    plugins/              native 插件实现
    keys.rs               coeffect 键（llm、fs、shell、agent-loop…）
  plugins/                script 叶子（cite.js、echo.js）
  guests/outline/         wasm 叶子（outline）
  cordis.yml              基础组合
  cordis.smoke.yml        CI：echo LLM + probe 界面
  cordis.creation.yml     创造模式 patch
  cordis.coding.yml       编码模式 patch
  assets/                 Web UI、sample.md
scripts/                  build-guests.sh（各子项目各一份）
.github/workflows/ci.yml  kernel / msrv / wasm / script / agent 五条 job
```

**边界**

| 层 | 拥有什么 | 不拥有什么 |
|---|---|---|
| 内核 `spatiotemporal` | effect/coeffect、Loader 对账、fiber 生命周期 | HTTP、bash、浏览器、LLM |
| 适配器 `spatiotemporal-wasm/script` | guest 生命周期、WIT/`host.*`、燃料/中断 | 新的 coeffect 种类 |
| Agent `spatiotemporal-agent` | cordis 组合、工具登记、Web、session | 不应「偷偷补齐」内核缺口（见 README 330–342 行） |

---

## 命令

在仓库根目录：

```bash
# 内核（default-members 只含根 crate，日常最快）
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --example swap_provider
cargo run --example watch_config

# MSRV 1.94（与 workspace 一致）
cargo +1.94.0 build -p spatiotemporal

# wasm 适配器（需 wasm32-wasip2 + guest 产物）
rustup target add wasm32-wasip2
./scripts/build-guests.sh          # 在 crates/spatiotemporal-wasm 下执行
cargo clippy -p spatiotemporal-wasm --all-targets -- -D warnings
cargo test -p spatiotemporal-wasm

# script 适配器
cargo clippy -p spatiotemporal-script --all-targets -- -D warnings
cargo test -p spatiotemporal-script

# 子进程适配器
./crates/spatiotemporal-process/scripts/build-guests.sh
cargo clippy -p spatiotemporal-process --all-targets -- -D warnings
cargo test -p spatiotemporal-process

# agent（需编 outline.wasm）
./spatiotemporal-agent/scripts/build-guests.sh
cargo clippy -p spatiotemporal-agent --all-targets -- -D warnings
cargo run -p spatiotemporal-agent -- --smoke    # 无 API key、不听端口
export DEEPSEEK_API_KEY=sk-...
cargo run -p spatiotemporal-agent                 # Web http://127.0.0.1:8787
cargo run -p spatiotemporal-agent -- --creation   # 创造模式
cargo run -p spatiotemporal-agent -- --coding     # 编码模式（更多 tool 轮次）
```

**改什么跑什么**：只动内核不必编 wasm/agent；动 agent 插件至少 `--smoke`；动 wasm guest 要 rebuild guests + `cargo test -p spatiotemporal-wasm`。

---

## 密钥与环境变量

| 变量 | 默认 | 用途 |
|---|---|---|
| `DEEPSEEK_API_KEY` | （对话必填） | DeepSeek Bearer token |
| `DEEPSEEK_BASE_URL` | `https://api.deepseek.com` | API 网关 |
| `DEEPSEEK_MODEL` | `deepseek-chat` | 模型名 |
| `PORT` | `8787` | Web 端口（或 `cordis.yml` `ui.config.port`） |
| `WORKSPACE` | 当前工作目录 | fs/bash 沙箱根（`root: .` 时） |

**永远不要**把 key 写进 `cordis.yml`、README 或提交 `.env`。`--smoke` 用脚本 `echo` 代替 DeepSeek，CI 不需要 key。

---

## 架构约定（对齐 dsh，用 Rust 表述）

### 组合，不是 monolith

- 能力来自 `cordis.yml` 每一行；`name` 是**断言**不是赋值——换实现 = `disabled: true` + `insert` 新行（见 `cordis.smoke.yml`）。
- 插件通过 `ctx.set::<Key>`（native）或 `host.registerTool` / `host.register-llm`（guest）贡献能力；卸载时宿主持有登记的逆。
- Agent 侧热对账走 `AgentRuntime::push_layer`，仅创造模式/审批通过后使用；不要绕过 Loader 直接改 fiber 树。

### 三种基质

| 基质 | 适合 | 不适合 |
|---|---|---|
| **native** | LLM HTTP、Web 界面、fs/bash 沙箱、agent-loop | — |
| **wasm** | 小 payload 叶子工具（outline） | 整段对话进 guest、新 coeffect |
| **script** | 快速试验叶子（cite、echo） | 不可信代码无审批热装 |
| **process** | MCP 桥、已有 CLI guest（NDJSON stdio） | 新 coeffect、大 payload |

### Context 键（agent 已用）

| 键 | 提供者示例 | 消费者 |
|---|---|---|
| `markdown` | `doc` | read-doc、outline、cite |
| `llm` | `deepseek` / echo 脚本 | agent-loop、web |
| `fs` | `fs-sandbox` | tool-fs |
| `shell` | `bash-sandbox` | tool-bash |
| `system-prompt` | `system-prompt` | agent-loop |
| `agent-loop` | `agent-loop` | web |
| `surface` | `web` / `probe` | main |

新增键：在 `keys.rs` 声明 `Key` trait，提供者 `apply` 里 `ctx.set`，消费者 `inject` + `resolve`。

### 工具登记

- 每个工具必须有**真实 JSON schema**（`tool_schema.rs` + `Toolbox::insert_with_schema`），与 handler 解析的字段一致。
- wasm/script 工具走 `ToolHost::register`；schema 目前默认为 `{ query }`，若 guest 需要结构化参数要在宿主侧扩展。
- 工具描述会进入 OpenAI `tools` 与 `system-prompt` 的 schema 段落。

### Agent 循环

- 逻辑在 `agent-loop` 插件，不在 Web 硬编码；换循环 = 换 cordis 行。
- 多轮 tool 消息写入 session history；Web 用 JSONL 持久化到 `{workspace}/.agent/sessions/`。
- 创造模式 `define_script` **必须**走 `ApprovalQueue`，禁止模型直接热装。

---

## 改代码时的约定

1. **最小 diff**：只改任务相关文件；不要顺手重构无关模块。
2. **MSRV 1.94**，`edition 2024`；workspace 内 `unsafe_code = forbid`（guest 边界除外）。
3. **Clippy**：CI 用 `-D warnings`；新增代码不应引入 warning。
4. **注释**：只解释非显而易见的业务/演算细节；不要复述代码。
5. **测试**：内核行为改动机理测试；配置改 `tests/loader.rs` / `tests/config.rs`；agent 至少 `--smoke`。
6. **新 native 插件**：`plugins/*.rs` + `plugins/mod.rs` + `registry.rs` + `cordis.yml` 一行；需要键则改 `keys.rs`。
7. **不要**在 agent 层用 `std::process` 直接冒充一等 fiber；子进程插件走 `spatiotemporal-process` + registry `process`。不要假装已有 dsh 的 session fork、MCP 官方客户端、PTY；compaction 已有初版，但勿夸大成完整事件流压缩。
8. **提交**：用户明确要求才 commit；不 force push main。

---

## 运行中 agent 的行为

在**本仓库**作为工作区使用时：

- **工作区根** = 启动 `cargo run -p spatiotemporal-agent` 时的 cwd（通常是仓库根）。`read` / `write` / `edit` / `bash` 只能在此根内操作。
- **先工具后结论**：改代码、查文档、验证假设时先 `read` / `bash` / `web_fetch`，再回答；引用内核/agent 行为时尽量贴路径或命令输出。
- **默认文档**：未传路径时读 `spatiotemporal-agent/assets/sample.md`；可用 `read_doc` / `outline` / `cite`。
- **网络**：`web_fetch` 仅 GET；抓取 crates.io、GitHub raw、论文链接时使用。
- **bash**：优先 `cargo test`、`cargo clippy`、`./scripts/build-guests.sh` 等；不要 `git push --force` 除非用户明确要求。
- **创造模式**（`--creation` 或浏览器「标准 / 创造」/`POST /api/mode`）：先 `inspect_*` 再看运行时；`define_script` 只提交审批；用户在浏览器点「批准」后才热装。
- **回答语言**：中文；代码与路径保持英文。

---

## 与 dsh 的对照（当前差距，勿夸大）

| dsh 能力 | spatiotemporal-agent 现状 |
|---|---|
| profile / bundle 多层组合 | bootstrap + file（`cordis.patch.yml`）+ dynamic 三层；`run_patch` / `revert_patch` |
| `ctx.sessions` 完整事件流 | JSONL 消息子集 + deriveMessages |
| `ctx.systemPrompt` 分段装配 | `system-prompt` 插件（AGENTS.md + schema） |
| `ctx.agentLoop` 可替换 | `agent-loop` 插件 |
| tool-cordis 审批 + preset | `approval-policy` 插件 + 多队列 + 审计 JSONL；`run_patch` 门控 |
| web_search / MCP / subagent | 未实现 |
| sandbox-policy / permission-presets | fs/bash 工作区沙箱 only |
| 异步 session 事件流 | `tiny_http` 同步阻塞 |
| 运行时 profile 切换 | 浏览器 / `POST /api/mode` 热对账创造层 |

路线图：P0–P2（已完成）→ **P3 agent 层（进行中/部分完成）**：审批策略、session compaction、LLM 后台线程；**P3 内核待做**：`Send` 执行器与多线程 Loader。

---

## 发布

- crates.io：`spatiotemporal`、`spatiotemporal-wasm`、`spatiotemporal-script`、`spatiotemporal-process`（agent 不发布）。
- 版本与 MSRV 在根 `Cargo.toml` workspace 统一；发版前跑全 CI。

---

## 编辑本文件

- 根目录 `AGENTS.md` 是 `system-prompt` 的默认输入；改措辞会直接影响 Web 里模型的 system prompt。
- 编码约定与运行行为分开成节，方便以后拆成「仅仓库开发」与「用户项目 AGENTS.md 模板」。
- 保持每条规则自洽；细节链到 README，不在此重复论文定理证明。
