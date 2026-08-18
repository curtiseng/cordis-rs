# Spatiotemporal Agent Demo

这是一篇**给 agent 读的演示文档**，覆盖当前 harness 里的主要能力。讲解员应当**先调工具、再下结论**——不要编造下文没有的内容。

## 这篇 demo 里有什么

| 能力 | 基质 | 工具 / 插件 |
|---|---|---|
| 读 Markdown | native | `read_doc`、`doc` |
| 抽标题大纲 | wasm | `outline` |
| 按关键词引用 | script | `cite` |
| 文档统计 | script | `stats` |
| 工作区文件 | native | `read`、`write`、`edit` |
| Shell | native | `bash`（工作区沙箱，30s 超时） |
| 抓取网页 | native | `web_fetch`（GET，512KB 上限） |
| 多轮对话 | native | `agent-loop` + JSONL session |
| 项目指令 | native | `system-prompt` 读工作区 `AGENTS.md` |
| LLM | native | `deepseek`（或 smoke 时 script `echo`） |
| 界面 | native | `web`（或 smoke 时 `probe`） |

创造模式（`cargo run -p spatiotemporal-agent -- --creation`）还会叠 `creation-tools`、`approval-policy`、`patch-watcher`。

## 可撤销 effect

每个改动上下文的动作都配一个逆。逆是值，卸载时按后进先出回放。组件作者不写卸载路径——拆得干净由演算保证，不依赖每个人的勤谨。

**试玩**：让 agent 用 `outline` 列出本文标题，再用 `cite` 搜「后进先出」。

## 响应式 coeffect

组件只声明「我需要 markdown」，不关心是谁在提供。提供者出现即激活，离开即去激活。把本地文件换成另一篇文档，消费者自己重载，没有人写重连逻辑。

## 四种基质

- **native**：编译进宿主，`fs` / `shell` / HTTP / Web UI；LLM 通过 `read` / `bash` 等 tool 使用 IO。
- **wasm**：`outline.wasm`，能力面由 WIT 钉死；叶子内可用 **`call-tool`** 桥接宿主工具表。
- **script**：`cite.js`、`stats.js` 等字符串 guest；叶子内可用 **`host.callTool(name, argsJson)`** 桥接宿主工具表（与 LLM 调 tool 同路径）。
- **process**：`spatiotemporal-process` crate（NDJSON stdio），MCP 桥接用；默认可执行文件需另编 guest。

**grant 与桥接**：demo 里 script/wasm 通常只 `grant: [markdown]`（读当前文档快照）。**不要**给 guest grant `fs`/`shell`——需要读文件或跑命令时，由 LLM 直接调 native tool，或在 define_script 的叶子代码里 `callTool`。

## 惯性

一次转换一旦开始就跑到完成。期间目标变了也不打断；完成时若目标已变，立刻链接进下一次转换。这是并发正确性的来源。

## 工具演示场景

### stats + cite + outline

问：「这篇有多少字、几个标题？」——应先 `stats`，必要时 `outline` 核对标题数。

问：「三种基质分别是什么？」——应先 `cite` 或 `read_doc`，引用原文。

### bash（工作区沙箱）

仓库根即工作区。可让 agent 跑只读命令，例如：

```bash
cargo test -p spatiotemporal-agent -- stats compaction approval 2>/dev/null | tail -5
```

或 `cargo run -p spatiotemporal-agent -- --smoke` 验证装配（不需 API key）。

### web_fetch

可抓取公开文档验证网络工具，例如 crates.io 上 `spatiotemporal` 的页面摘要（GET only）：

`https://crates.io/api/v1/crates/spatiotemporal`

### read / write / edit

`read` 可读工作区内任意文本；`write` / `edit` 会改文件——demo 时优先读 `AGENTS.md` 或 `spatiotemporal-agent/DEMO.md`，不要随便覆盖源码。

## Session 与 compaction

多轮对话写入 `.agent/sessions/*.jsonl`。上下文过长时 `agent-loop` 按 `cordis.yml` 的 `compaction` 截断 tool 输出并保留最近消息（轮次上限见 `max_rounds`）。

**操作建议**：代码任务用编码模式；history 过长或 LLM 报 tool 消息格式错误时**新开会话**，避免在压缩后的脏 history 上继续硬聊。

## 创造模式试玩

启动 `--creation` 后可用元工具：

- `inspect_plugins` / `inspect_tools` / `inspect_config`
- `define_plugin` / `define_script`（提交审批，浏览器点「批准」才热装）
- `run_patch`（试跑 YAML patch，同样需审批）
- `save_patch` / `reload_patch` / `revert_patch`

示例：提交一段简单 script 插件，或 `inspect_tools` 看 `stats` 是否已登记。

## 论文与仓库

演算内核见仓库根 [README.md](../README.md)。论文：[*A Programming Paradigm for Spatiotemporal Composability*](https://github.com/cordiverse/paper)。

更完整的试玩脚本见 [DEMO.md](../DEMO.md)。
