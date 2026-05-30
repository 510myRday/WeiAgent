# WeiAgent 架构讨论记录

## 协作约定

以后关于 WeiAgent 的架构讨论、设计变更、阶段目标、功能取舍，都优先记录并修改在这个 `Agent.md` 文件里。我们后续沟通时，也以这个文件作为主要上下文和持续更新的设计文档。

## 项目定位

WeiAgent 是一个使用 Rust 编写的 Agent 项目。

项目不使用已有的 Agent 框架，Agent 的核心逻辑、执行循环、工具系统、多 Agent 管理等能力都由项目自己实现。

## 第一版目标

第一版先做最简单可运行的版本，重点是把基础架构跑通。

暂时不做记忆系统。

第一版需要支持：

- 生成多个 Agent
- 使用 OpenAI 兼容格式接入大模型
- Rust 后端暴露 API 服务
- React + Vite + TypeScript 前端单独运行
- 前端中间是一个简单对话框
- 前端左侧有一个侧边栏
- 左侧侧边栏用于创建、选择、编辑、删除 Agent
- Agent 配置和聊天记录使用 JSON 文件保存
- 同时支持普通完整回复和 SSE 流式回复

## 初始架构想法

第一版可以先按以下模块拆分：

- Rust API 服务层：使用 `axum` 启动 HTTP 服务，并提供 Agent 与聊天接口
- React 页面层：使用 `Vite + React + TypeScript`，提供基础对话界面和左侧 Agent 侧边栏
- LLM 适配层：使用 OpenAI 兼容 Chat Completions API 发送请求和接收响应
- Agent 管理层：负责创建、保存、更新、删除和选择多个 Agent
- Agent 执行层：负责单个 Agent 的一次对话执行流程
- JSON 存储层：负责保存 Agent 配置和每个 Agent 的聊天记录

## 暂不实现

第一版暂时不做：

- 长期记忆系统
- 向量数据库
- 复杂工具调用
- 多 Agent 自动协作流程
- 权限系统
- 用户登录系统

## 第一版已确定决策

- 多 Agent 第一版只做手动切换，不做主 Agent 调度
- Agent 字段（第一版）：`id`、`name`、`model_provider_id`、`model_id`、`created_at`、`updated_at`（注：第二阶段已扩展，见下方）
- Rust Web 框架使用 `axum`
- 前端使用 `React + Vite + TypeScript`
- 前后端分离，前端通过 API 调用后端
- API Key 只放在后端环境变量里，前端不接触密钥（注：第二阶段已改为明文存储在 JSON 文件中，API 响应通过 masked_api_key 和 has_api_key 字段脱敏返回）
- 聊天记录保存到 JSON 文件，但不作为长期记忆系统使用
- `cargo run dev` 会启动 Rust API 并 spawn Vite 开发服务器子进程；Vite 固定监听 `127.0.0.1:5173` 并提供前端热更新，API 代理目标从 `data/settings.json` 读取；正式二进制运行时使用 `rust-embed` 编译进 exe 的前端 `dist` 资源，通过 fallback 路由返回 `index.html`（支持 SPA 路由）

## 项目目录结构

```
WeiAgent/
├── Agent.md                  # 本文档
├── Cargo.toml                # Rust 项目配置
├── build.rs                  # 构建脚本：自动编译前端并嵌入二进制
├── src/
│   ├── main.rs               # 入口：启动 axum、spawn Vite 子进程
│   ├── config.rs             # 配置加载与校验
│   ├── error.rs              # 错误类型与 HTTP 响应映射
│   ├── app/
│   │   ├── mod.rs            # 集成测试（mock LLM + 完整 HTTP 测试）
│   │   ├── router.rs         # API 路由定义
│   │   ├── state.rs          # AppState（Arc<JsonStore> + DynLlmRegistry）
│   │   ├── chat.rs           # 聊天准备：构建 system_prompt、组装消息
│   │   ├── static_assets.rs  # rust-embed 前端资源服务
│   │   └── handlers/
│   │       ├── agents.rs
│   │       ├── conversations.rs  # 流式/非流式发送消息、SSE 事件
│   │       ├── providers.rs
│   │       └── settings.rs
│   ├── domain/
│   │   ├── agents.rs         # Agent、AgentWorkspace、请求/响应类型
│   │   ├── chat.rs           # ChatMessage、ChatRole、SendMessageRequest
│   │   ├── conversations.rs  # Conversation
│   │   ├── providers.rs      # ModelProvider、ProviderType、请求/响应类型
│   │   └── settings.rs       # AppSettings（含默认值）
│   ├── llm/
│   │   ├── client.rs         # ChatClient trait、RequestOptions
│   │   ├── registry.rs       # LlmRegistry trait、DefaultLlmRegistry
│   │   ├── types.rs          # OpenAI 请求/响应类型、build_chat_request
│   │   ├── sse.rs            # SSE chunk 解析
│   │   └── providers/
│   │       └── openai_compatible.rs  # OpenAI 兼容客户端实现（含重试）
│   └── storage/
│       ├── mod.rs            # 常量、测试
│       ├── json_store.rs     # JsonStore 结构体与文件管理
│       ├── agents.rs         # Agent CRUD
│       ├── providers.rs      # Provider CRUD（含关联 Agent 校验）
│       ├── conversations.rs  # Conversation CRUD、消息持久化
│       ├── workspaces.rs     # Workspace 读写、system_prompt 拼接、内置 Agent 初始化
│       ├── validation.rs     # 字段校验（avatar 去重、hex 颜色、provider/model 关联）
│       └── files.rs          # JSON 文件读写工具
├── frontend/                 # React 前端（独立 npm 项目）
│   ├── src/
│   │   ├── api.ts            # 所有 API 调用（fetch + SSE 流解析）
│   │   ├── types.ts          # 前端类型定义
│   │   ├── hooks/            # useAgents、useProviders、useSettings、useConversations
│   │   ├── components/       # UI 组件 + layout
│   │   ├── features/         # 功能面板（chat、agents、settings、history）
│   │   └── styles/           # CSS 样式（base、components、layout、responsive、tokens）
│   ├── locales/              # 国际化（zh.json、en.json）
│   └── vite.config.ts        # Vite 配置（proxy、vendor chunk 分割）
└── data/                     # 运行时数据（dev 模式在项目根目录，release 在 exe 同目录）
    ├── settings.json
    ├── model_providers.json
    ├── agents.json
    ├── conversations.json
    ├── agents/{agent_id}/    # 工作空间文件
    │   ├── personality.md
    │   ├── memory.md
    │   ├── instructions.md
    │   └── tools.json
    └── conversations/{conversation_id}.json
```

## JSON 配置

项目不再从 `.env` 或系统环境变量读取运行配置。`cargo run dev` 使用项目根目录下的 `data`；直接运行打包后的二进制文件时，使用二进制文件同目录下的 `data`，不存在则自动初始化。

- `data/settings.json`：保存应用监听地址、端口、日志级别、用户头像图标和颜色、语言、请求超时秒数、超时重试次数。默认值：`127.0.0.1:18283`、`info`、`bot/#22c55e`、`zh`、`60`、`3`
- `data/model_providers.json`：保存 OpenAI 兼容模型提供商、Base URL、API Key、模型列表和 extra_body（额外请求体 JSON）。API Key 本地明文保存，但 API 响应通过 `has_api_key` 布尔值和 `masked_api_key`（如 `****cret`）脱敏返回
- `data/agents.json`：保存 Agent 配置，包含 `id`、`name`、`model_provider_id`、`model_id`、`is_builtin`、`avatar_icon`、`avatar_color`、`created_at`、`updated_at`
- `data/conversations.json` 与 `data/conversations/{conversation_id}.json`：保存多会话元数据和消息。会话标题自动从第一条用户消息截取（最多 80 字符）；`message_count` 在列表查询时自动修正

旧版 `model` 字段和 `data/chats/{agent_id}.json` 会在启动时迁移到 Provider 与 Conversation 结构。

## 第一版 API

- `GET /api/settings` / `PUT /api/settings`：读取和保存后端设置
- `GET /api/model-providers`：获取模型提供商列表
- `POST /api/model-providers`：创建模型提供商
- `PUT /api/model-providers/{provider_id}`：更新模型提供商（如果缩减模型列表会校验是否有关联 Agent 使用被移除的模型）
- `DELETE /api/model-providers/{provider_id}`：删除模型提供商（有关联 Agent 时拒绝）
- `GET /api/model-providers/{provider_id}/models`：通过已保存提供商的凭证，调用远程 `/models` 接口获取模型列表
- `POST /api/model-providers/models`：根据临时参数（不保存 provider）获取远程模型列表
- `GET /api/agents`：获取 Agent 列表
- `POST /api/agents`：创建 Agent（自动初始化工作空间文件）
- `PUT /api/agents/{agent_id}`：更新 Agent（内置 Agent 不可改名）
- `DELETE /api/agents/{agent_id}`：删除 Agent（内置 Agent 不可删除；同时清理关联的会话文件和工作空间目录）
- `GET /api/agents/{agent_id}/conversations`：读取某个 Agent 的会话列表
- `POST /api/agents/{agent_id}/conversations`：创建会话
- `GET /api/conversations/{conversation_id}/messages`：读取会话消息
- `POST /api/conversations/{conversation_id}/messages`：非流式发送消息（支持 timeout + retry）
- `POST /api/conversations/{conversation_id}/messages/stream`：SSE 流式发送消息
- `DELETE /api/conversations/{conversation_id}`：删除会话

### SSE 流式协议

前端 `api.ts` 中 `streamMessage` 使用 `fetch` + `ReadableStream` 消费 SSE，事件类型：

- `user`：用户消息已持久化，`data` 为 `ChatMessage` JSON
- `assistant`：助手消息已创建，`data` 为 `ChatMessage` JSON（`status: "streaming"`）
- `delta`：增量文本内容，`data` 为纯文本字符串
- `done`：流式传输完成
- `error`：错误信息，`data` 为错误描述字符串

后端流式保存策略：每 500ms 或每 240 字符触发一次增量持久化。如果流被中断（连接断开），`StreamCleanup` 的 `Drop` 实现会自动将已部分保存的消息标记为 `Failed`。

### 前端 Abort 支持

前端 `sendMessage` 和 `streamMessage` 均接受 `AbortSignal` 参数，`useConversations` hook 在发送流式消息时持有 `AbortController`，用户可随时停止生成。

## 第二阶段状态

项目已进入第二阶段。第二阶段的核心目标是把 Agent 从"只有 JSON 配置的空壳"推进为"有内置核心 Agent、有独立工作空间、有可持续扩展配置"的结构。

第二阶段第一步已经确定并开始实现：内置第一个核心 Agent，并建立 Agent 工作空间规范。

## 第二阶段第一步：内置 CEO Agent

第一个内置 Agent 定位为用户之下的 CEO / 总控 Agent，是用户最常对话的核心入口。

- 默认名称：中文显示为`总管`，英文显示为 `Master`；底层固定保存为 `Master`
- 内置标识：`is_builtin: true`
- 默认头像：`crown` + `#2563eb`（蓝色皇冠）
- 行为风格：统筹 + 追问
- 角色定位：帮助用户澄清目标、拆解任务、判断优先级、识别风险，并在未来建议创建或调用其他 Agent
- 模型建议：建议配置能力强、上下文长、推理稳定的模型；当前只做文案提醒，不强制限制模型选择

内置 Agent 在后端启动时自动补齐。内置身份直接写入 `data/agents.json` 的 `is_builtin` 字段。内置 Agent 可以修改模型、头像和 Markdown 文档，但不能重命名、不能删除。

如果启动时没有任何模型提供商，后端会初始化一个本地默认 OpenAI 兼容 Provider，用于让内置 Agent 能够落库。用户后续需要在设置里补充 API Key、模型列表和实际可用模型。

## Agent 工作空间

每个 Agent 都有独立工作空间：

- `data/agents/{agent_id}/personality.md`：个性、人设、沟通风格
- `data/agents/{agent_id}/memory.md`：长期记忆、用户偏好、项目背景和重要决策
- `data/agents/{agent_id}/instructions.md`：核心工作规则、输出要求和默认流程
- `data/agents/{agent_id}/tools.json`：未来工具调用配置占位，本阶段不参与提示词

运行时会将 `personality.md`、`memory.md`、`instructions.md` 按 `## Personality`、`## Memory`、`## Instructions` 标题合并为 system prompt 发送给模型。`tools.json` 不进入提示词，后续工具阶段会接入模型工具调用接口。

Agent 已不再使用单一的 `system_prompt` 字段，工作空间的三份 Markdown 文档完全取代了它。

## 第二阶段 Agent API 补充

- `GET /api/agents/{agent_id}/workspace`：读取 Agent 工作空间文档和扩展设置
- `PUT /api/agents/{agent_id}/workspace`：保存 `personality.md`、`memory.md`、`instructions.md`

Agent 创建和更新接口新增头像字段：

- `avatar_icon`
- `avatar_color`

Agent 响应会额外返回后端保存的内置身份信息：

- `is_builtin`

## 第二阶段 Agent 字段（完整）

创建/更新后持久化的 Agent 字段：

- `id`（UUID v4）
- `name`
- `model_provider_id`（UUID，关联 Provider）
- `model_id`
- `is_builtin`（bool，内置 Agent 受保护）
- `avatar_icon`（9 个预设之一）
- `avatar_color`（hex 颜色，如 `#22c55e`）
- `created_at`
- `updated_at`

## 第二阶段 settings.json 字段（完整）

- `host`：监听地址，默认 `127.0.0.1`
- `port`：监听端口，默认 `18283`
- `log_level`：日志级别（trace/debug/info/warn/error），默认 `info`
- `avatar_icon`：用户头像图标，默认 `bot`
- `avatar_color`：用户头像颜色（hex），默认 `#22c55e`
- `language`：界面语言 `zh` / `en`，默认 `zh`
- `request_timeout_seconds`：LLM 请求超时秒数，默认 `60`
- `request_retry_attempts`：超时重试次数，默认 `3`

## 第二阶段头像规则

用户头像和 Agent 头像复用同一套头像组件和图标预设。

- 头像图标扩展为 9 个：`bot`、`sparkles`、`zap`、`heart`、`star`、`smile`、`crown`、`brain`、`compass`
- 支持预设色板选择颜色
- Agent 头像保存到 `data/agents.json`
- 用户头像保存到 `data/settings.json`
- Agent 不允许出现"图标相同且颜色相同"的重复头像组合（`validate_unique_avatar` 校验，大小写不敏感）

## 第二阶段 Provider 字段与规则

- `provider_type`：当前仅支持 `openai_compatible`
- `extra_body`：额外请求体 JSON（在 Chat Completions 请求中与基础字段合并发送）
- API Key 存储为明文，API 响应返回 `has_api_key: true/false` 和 `masked_api_key: "****后4位"`
- 更新 Provider 时如果传入空字符串 `api_key`，后端会跳过更新保留原 key
- 更新 Provider 模型列表时，会校验当前关联该 Provider 的所有 Agent 使用的模型仍在新列表中
- 删除 Provider 时，如果有 Agent 仍关联它，拒绝删除

## 第二阶段聊天特性

### 会话标题自动生成

- 创建会话时标题默认 `"New Conversation"`
- 首次发送消息后，后端自动用第一条用户消息的前 80 个字符替换标题
- 标题为占位值（`"New Conversation"` / `"新会话"` / `"新建会话"` / 空字符串）时才自动替换

### 流式消息保存与中断恢复

- 流式消息首次持久化时作为新消息插入，后续每 500ms 或 240 字符增量更新
- `StreamCleanup` 的 `Drop` 确保连接中断时已保存的内容标记为 `Failed` 而非丢失
- 前端 `cancelActiveSend` 主动中断时，如果助手已有内容则标记 `Failed`，否则移除空消息

### LLM 请求重试

- 非流式和流式请求均支持超时重试（次数由 `request_retry_attempts` 决定）
- 重试间隔递增：250ms × 尝试次数

## 后续方向

第二阶段后续可以继续推进：

- 记忆系统：让 `memory.md` 从手动编辑升级为可控沉淀
- 多 Agent 协作：由内置核心 Agent 建议、调度或组织其他 Agent
- Agent 模板：继续增加更多内置 Agent 或可导入模板
## 第三阶段：终端工具系统

第三阶段为 Agent 添加终端命令执行能力，采用黑名单安全策略，审批交互嵌入聊天气泡。

### 安全策略

- **黑名单模式**：命令不在黑名单和审批名单中的默认放行
- **拒绝名单**（`blocked_patterns`）：匹配后直接拦截，如 `rm -rf /`、`curl|sh`、`sudo`、`chmod 777`、命令注入等
- **审批名单**（`approval_required_patterns`）：匹配后在聊天气泡内显示审批卡片，用户点击允许/始终允许/拒绝
- 所有正则模式可在前端 Agent 编辑页的 Tools tab 中实时修改

### 工具配置

每个 Agent 的 `data/agents/{agent_id}/tools.json` 存储终端工具配置：

```json
{
  "terminal": {
    "enabled": true,
    "blocked_patterns": ["rm\\s+-rf\\s+/", "curl.*\\|\\s*sh", "sudo\\s+"],
    "approval_required_patterns": ["rm\\s+", "git\\s+push", "docker\\s+rm"],
    "default_timeout_ms": 30000,
    "allowed_cwd_roots": []
  }
}
```

### 后端新增模块

- `src/domain/tools.rs`：`TerminalToolConfig`、`AgentToolsConfig`、`CommandDecision`、`ToolResult` 类型定义
- `src/tools/terminal.rs`：`CommandChecker` 正则匹配引擎，含单元测试
- `src/tools/process_manager.rs`：`DashMap` 后台进程管理，输出缓存上限 1MB
- `src/tools/execute.rs`：`execute_terminal` 异步执行函数，支持超时转后台
- `src/app/handlers/tools.rs`：`GET/PUT /api/agents/{agent_id}/tools-config` 端点
- `AppState` 新增 `process_manager: Arc<ProcessManager>`
- `Cargo.toml` 新增 `regex`、`dashmap` 依赖

### 前端新增组件

- `ToolCallCard.tsx`：5 种状态渲染（running/pending_approval/completed/blocked/rejected/error）
- `TerminalToolSettings.tsx`：Agent 编辑页 Tools tab，可视化编辑黑名单/审批名单/超时/cwd
- `types.ts` 新增 `ToolCallMessage`、`UiMessageItem`、扩展 `StreamEvent` 支持 `tool_call`/`tool_result`/`approval_required`
- `useConversations` hook 处理工具相关 SSE 事件
- `ChatPanel.tsx` 消息流区分普通消息和工具调用卡片，审批按钮直接在气泡内交互
- `api.ts` 新增 `readAgentToolsConfig`、`updateAgentToolsConfig`、`approveToolCall`

### SSE 事件扩展

- `tool_call`：通知前端工具开始执行
- `tool_result`：返回工具执行结果（stdout/stderr/exit_code/duration）
- `approval_required`：触发气泡内审批卡片

### 已完成补充

- **Phase 3.2**：`POST /api/tools/approve` 审批端点。内存 `ApprovalManager`（DashMap）管理待审批条目，支持 approve/reject/always_approve 三种决策，10 分钟自动过期清理。
### Function Calling 架构

第三阶段工具系统使用正规 **OpenAI Function Calling 协议**，工具定义通过请求体的 `tools` 字段发送，不进入 system prompt。

#### 核心改动

- **LLM Client Trait**：`ChatClient::stream()` 返回 `Stream<StreamEvent>` 枚举（TextDelta / ToolCallStart / ToolCallArgsDelta / FinishedWithToolCalls / Finished），替代原来的 `Stream<String>`；`complete()` 和 `stream()` 均新增 `tools: Option<Vec<ToolDefinition>>` 参数
- **请求构建**：`build_chat_request()` 接受 `tools` 参数，生成符合 OpenAI 规范的 `tools` JSON Schema；`OpenAiMessage` 支持 `role: "tool"` + `tool_call_id` 字段
- **SSE 解析**：`parse_sse_line()` 区分 `content` delta 和 `tool_calls` delta，按 `index` 累积 `arguments`，检测 `finish_reason: "tool_calls"`
- **执行循环**：SSE 流中实时累积 tool_calls → 流结束后安全检查 → 执行/审批 → 追加 assistant `tool_calls` 消息和 `role: "tool"` 结果消息 → 二次调用 LLM 获得最终回复
- **Domain 扩展**：`ChatRole` 新增 `Tool` 变体；`ChatMessage` 新增 `tool_call_id: Option<String>` 字段

#### 数据流

```
User Message → prepare_chat(tools_config) → LLM stream(tools=[execute_terminal])
  ↓ StreamEvent::TextDelta → 前端 delta 事件
  ↓ StreamEvent::ToolCallStart/ArgsDelta → 累积 tool_calls
  ↓ StreamEvent::FinishedWithToolCalls → 触发工具执行
  ↓ CommandChecker → Allowed/Blocked/NeedsApproval
  ↓ execute_terminal → tool_result SSE 事件
  ↓ ChatMessage{role:Tool, tool_call_id} → 追加到 messages
  ↓ LLM complete(messages+tool_results) → follow-up 文本
  ↓ done SSE 事件
```


### 待完成

- Phase 6：端到端联调测试（放行/拦截/审批/后台进程全流程验证）
