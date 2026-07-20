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
│   │   ├── agent_executor.rs # 普通/流式统一的多轮 Agent + Tool 执行器
│   │   ├── static_assets.rs  # rust-embed 前端资源服务
│   │   └── handlers/
│   │       ├── agents.rs
│   │       ├── conversations.rs  # 流式/非流式发送消息、SSE 事件
│   │       ├── providers.rs
│   │       ├── settings.rs
│   │       ├── tools.rs      # 工具配置、审批、后台进程 API
│   │       └── llm_logs.rs
│   ├── domain/
│   │   ├── agents.rs         # Agent、AgentWorkspace、请求/响应类型
│   │   ├── chat.rs           # ChatMessage、ChatRole、SendMessageRequest
│   │   ├── conversations.rs  # Conversation
│   │   ├── providers.rs      # ModelProvider、ProviderType、请求/响应类型
│   │   ├── settings.rs       # AppSettings（含默认值）
│   │   └── tools.rs          # 工具、审批、沙箱和进程类型
│   ├── llm/
│   │   ├── client.rs         # ChatClient trait、RequestOptions
│   │   ├── registry.rs       # LlmRegistry trait、DefaultLlmRegistry
│   │   ├── types.rs          # OpenAI 请求/响应类型、build_chat_request
│   │   ├── sse.rs            # SSE chunk 解析
│   │   └── providers/
│   │       └── openai_compatible.rs  # OpenAI 兼容客户端实现（含重试）
│   ├── tools/
│   │   ├── registry.rs       # ToolRegistry、AgentTool、内置工具注册
│   │   ├── terminal.rs       # Codex 终端执行、会话、输出和后台进程管理
│   │   └── web_search/       # 网络搜索：调度、Key 池、Tavily/Exa 适配
│   │   ├── codex_app_server.rs # Codex app-server JSON-RPC 连接
│   │   ├── runtime.rs        # Codex CLI 发现、下载、校验和隔离环境
│   │   ├── approval.rs       # 审批管理和自动 TTL 过期
│   │   └── prompt.rs         # 终端环境和权限策略提示
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
    ├── tool_defaults.json
    ├── agents.json
    ├── conversations.json
    ├── agents/{agent_id}/    # 工作空间文件
    │   ├── personality.md
    │   ├── memory.md
    │   ├── instructions.md
    │   └── tools.json         # Agent 工具覆盖项和上下文授权
    ├── runtimes/              # WeiAgent 托管的 Codex 运行时和隔离 CODEX_HOME
    └── conversations/{conversation_id}.json
```

## JSON 配置

项目不再从 `.env` 或系统环境变量读取运行配置。`cargo run dev` 使用项目根目录下的 `data`；直接运行打包后的二进制文件时，使用二进制文件同目录下的 `data`，不存在则自动初始化。

- `data/settings.json`：保存应用监听地址、端口、日志级别、用户头像图标和颜色、语言、请求超时秒数、超时重试次数。默认值：`127.0.0.1:18283`、`info`、`bot/#22c55e`、`zh`、`60`、`3`
- `data/weiagent.db` 表 `model_providers`：保存模型提供商、Base URL、API Key、模型列表和 extra_body。API Key 本地明文保存，API 响应通过 `has_api_key` / `masked_api_key` 脱敏
- `data/tool_defaults.json`：保存所有 Agent 默认继承的工具配置，不保存任何 Agent 上下文授权
- `data/agents.json`：保存 Agent 配置，包含 `id`、`name`、`description`、`model_provider_id`、`model_id`、`is_builtin`、`avatar_icon`、`avatar_color`、`created_at`、`updated_at`
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
- `GET /api/agents/{agent_id}/runs`：读取该 Agent 下正在运行的后台 run
- `GET /api/conversations/{conversation_id}/messages`：读取会话消息
- `POST /api/conversations/{conversation_id}/runs`：创建独立后台 Agent run，返回 `run_id`
- `GET /api/conversations/{conversation_id}/run`：读取会话当前活动 run
- `GET /api/runs/{run_id}`：读取 run 状态
- `GET /api/runs/{run_id}/events?after=N`：按序号订阅或重放 run 事件
- `DELETE /api/runs/{run_id}`：显式取消 run，并等待取消完成
- `DELETE /api/conversations/{conversation_id}`：删除会话

### SSE 流式协议

前端 `api.ts` 中 `streamRunEvents` 使用 `fetch` + `ReadableStream` 消费可重放 SSE，事件类型：

- `user`：用户消息已持久化，`data` 为 `ChatMessage` JSON
- `assistant`：助手消息已创建，`data` 为 `ChatMessage` JSON（`status: "streaming"`）
- `delta`：增量文本内容，`data` 为纯文本字符串
- `done`：流式传输完成
- `cancelled`：run 被显式取消
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

内置 Agent 在后端启动时自动补齐。内置身份直接写入 `data/agents.json` 的 `is_builtin` 字段。Agent 简介为必填字段；旧数据缺少简介时会在启动时迁移补齐。内置 Agent 可以修改模型、头像和 Markdown 文档，但不能修改名称和简介，也不能删除。

如果启动时没有任何模型提供商，后端会初始化一个本地默认 OpenAI 兼容 Provider，用于让内置 Agent 能够落库。用户后续需要在设置里补充 API Key、模型列表和实际可用模型。

## Agent 工作空间

每个 Agent 都有独立工作空间：

- `data/agents/{agent_id}/personality.md`：个性、人设、沟通风格
- `data/agents/{agent_id}/memory.md`：长期记忆、用户偏好、项目背景和重要决策
- `data/agents/{agent_id}/instructions.md`：核心工作规则、输出要求和默认流程
- `data/agents/{agent_id}/tools.json`：只保存该 Agent 相对全局默认值的覆盖字段，以及该 Agent 的上下文授权；旧版完整配置会自动迁移为差异覆盖

运行时 system prompt 由以下几部分按固定顺序拼接后发送给模型：

1. `## Environment`：运行时环境前缀（应用名/版本、OS/架构、Shell、Default workspace）
2. `## Agent`：一句身份背景 + `Name`/`ID`，标明当前处理本轮对话的智能体
3. `## Personality` / `## Memory` / `## Instructions`：分别来自工作空间的 `personality.md`、`memory.md`、`instructions.md`

工具执行使用“全局默认 + Agent 覆盖 + Agent 授权”合并后的有效配置，`tools.json` 本身不进入提示词。

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

终端工具复用 Codex CLI 的 OS 沙箱与 app-server 终端能力，并由 WeiAgent 管理人工审批、后台记录和会话续写；不使用 Docker，也不提供自动审批。

### 权限模式

- `request_approval`：普通命令通过 Codex OS sandbox helper 的 `:workspace`/`workspaceWrite` 策略执行。工作区内写入自动完成，网络和工作区外写入被沙箱拒绝。
- `full_access`：命令通过 Codex app-server 的 `command/exec` 以完整宿主权限执行，不产生审批提示。
- 模型只有显式设置 `sandbox_permissions = "require_escalated"` 并提供 `justification` 时，才会在 `request_approval` 模式下请求人工批准。
- 用户可允许一次、记住 24 小时或拒绝。记忆授权精确绑定命令和工作目录。

### 工具配置

- 全局工具页编辑 `data/tool_defaults.json`，作为所有 Agent 的默认配置。
- Agent 的 Tools 标签按字段选择“继承默认”或保存独立覆盖，不复制整份全局配置。
- `approval_grants` 始终属于具体 Agent，不参与全局继承。
- API 同时返回 `defaults`、`overrides`、`effective`，聊天执行只使用合并后的 `effective`。

```json
{
  "max_llm_iterations": 99,
  "terminal": {
    "enabled": true,
    "permission_mode": "request_approval",
    "approval_grants": [],
    "default_timeout_ms": 30000,
    "allowed_cwd_roots": []
  }
}
```

### 执行模型

```text
User Message
  -> LLM Function Calling
  -> execute_terminal
  -> use_default
       -> Codex OS sandbox helper
       -> workspace write / network denied
  -> require_escalated
       -> approval_required SSE
       -> user approve
       -> Codex app-server full-access session
  -> tool_result
  -> role=tool message
  -> follow-up LLM response
```

生产环境中，WeiAgent 只负责启动 Codex CLI/app-server 并发送结构化执行请求，不直接绕过 Codex 来执行模型命令。Codex 会在其控制的执行链路中启动 WeiAgent 已探测的本机 Shell：Windows 依次选择 PowerShell 7 (`pwsh.exe`)、Windows PowerShell (`powershell.exe`) 和 CMD (`cmd.exe`)，macOS 和 Linux 使用 POSIX `sh`。Shell 是命令解释器，但沙箱策略、进程生命周期、stdin/stdout 和终止控制均由 Codex 路径承载。

### 终端能力

- 内置工具：`execute_terminal`、`terminal_session`、`multi_edit_file`（隐藏内置编辑器，与终端同启停）
- `execute_terminal` 参数：`command`、`cwd`、`timeout_ms`、`background`、`tty`、`sandbox_permissions`、`justification`
- `multi_edit_file`：单文件/多文件精确替换，以及 create/delete/move；不出现在 Tools UI，仅随 `terminal.enabled` 注册到模型 schema。参数含 `operations`、`cwd`、`sandbox_permissions`、`justification`。`request_approval` 下工作区内写文件需审批（支持 24h always-allow）；工作区外路径必须 `sandbox_permissions=require_escalated` 并附 `justification` 后走审批，不得用 `execute_terminal` 绕过编辑工具。`full_access` 下可直接写工作区内外路径
- `terminal_session` 操作：`poll`、`write`、`submit`、`interrupt`、`eof`、`terminate`、`resize`，通过 `output_cursor` 增量读取输出
- `cwd` 省略、空字符串或 `.` 都使用配置的默认工作区；`allowed_cwd_roots` 为空时默认工作区是 WeiAgent 数据目录（开发模式为 `data`，生产模式默认为 `%USERPROFILE%\.WeiAgent`）；非空 `cwd` 必须是已存在目录，不允许失败后静默切换到猜测路径
- 实际执行器、终端工具定义、系统环境提示和命令示例共用同一个 Shell 探测结果，避免 PowerShell、CMD 与 POSIX 语法混用
- 支持 Codex PTY、stdin 续写、Ctrl+C/EOF、终止和尺寸调整；后台或超时转后台后返回 `process_id`
- Windows 的 Codex unelevated 沙箱是上游例外：只允许短时、非交互、前台命令；后台、stdin 续写、PTY 和超时转后台必须请求完整权限
- 普通前台命令只在执行期间由进程管理器捕获输出，完成后立即移除，不出现在后台进程列表
- 只有显式 `background: true` 或超过前台等待时间的任务会进入后台进程列表
- stdout/stderr 各保留最后 1MB，返回模型时限制为最后 64KB
- 真正的后台进程最长运行 1 小时，完成记录保留 24 小时
- 全局最多 32 个运行进程，每个 Agent 最多 8 个
- 单轮最多 8 个工具调用，单次 Agent run 最多 256 个，最长运行 60 分钟
- LLM 工具循环默认 99 轮，可配置最大 999 轮；耗尽后执行一次无工具总结
- 同一会话只允许一个活跃 Agent run
- 完整权限和非 Windows 沙箱会话支持交互续写；Windows 沙箱保持非交互，以避免 Codex 上游不支持的伪会话

### Codex 终端运行时

- `WEIAGENT_CODEX_BIN` 显式指定的兼容版本优先；否则 WeiAgent 在托管版本和系统 Codex 中选择版本较高的一份。
- 全局工具页可以查看、刷新、安装或更新托管 Codex 运行时；启动时使用本地状态，后台刷新 npm Registry 的稳定版信息并缓存 24 小时。
- 安装时优先使用 npm Registry 的当前稳定版及对应 OS/CPU 原生包；查询失败且没有可用缓存时回退到 Codex CLI `0.137.0`，不要求用户安装 Node.js 或 npm。
- Codex CLI `0.137.0` 是当前最低兼容版本；更旧的系统安装不会被选用。
- 托管包覆盖 Windows x64/ARM64、Linux x64/ARM64、macOS x64/Apple Silicon；三系统 CI 持续执行前端构建、Rust 测试和 Clippy。
- 下载过程流式写入临时文件，限制最大体积，使用 Registry 提供的 SHA-512 integrity 校验后安全解包，并通过版本检查和 app-server 初始化握手后再启用。
- 托管运行时安装到 `data/runtimes/codex/{version}/{platform}`，独立 `CODEX_HOME` 保存到 `data/runtimes/codex-home`。
- Codex 运行时不可用时不会向模型暴露终端工具，也不会自动降级为直接宿主执行。
- Windows 沙箱命令复用用户 Codex 的默认 `CODEX_HOME` 沙箱设置；app-server 使用 WeiAgent 隔离的 `CODEX_HOME`，且启动环境会移除无关敏感变量。
- Codex CLI 采用 Apache-2.0 许可证，第三方归属信息见 `THIRD_PARTY_NOTICES.md`。

### 验证

- 后端单元与端到端测试覆盖权限模式、人工审批、授权记忆、真实 OS 沙箱边界、后台进程和工具预算
- `cargo clippy --all-targets --all-features -- -D warnings`
- `npm run build`


## 第四阶段：网络搜索工具

网络搜索是第二个内置工具组，模型侧只暴露 `web_search`。密钥仅保存在全局 `data/tool_defaults.json`；Agent 只能覆盖启用状态和 `max_results`。

### 配置结构

全局默认（`tool_defaults.json`）：

```json
{
  "max_llm_iterations": 99,
  "terminal": { "...": "..." },
  "web_search": {
    "enabled": false,
    "max_results": 5,
    "timeout_ms": 15000,
    "key_selection": "sequential",
    "sources": [
      { "id": "tavily", "provider": "tavily", "enabled": true, "api_keys": [] },
      { "id": "exa", "provider": "exa", "enabled": true, "api_keys": [] }
    ]
  }
}
```

- `key_selection`: `sequential` | `round_robin` | `random`（仅作用于同一源内的多 Key）
- 源数组顺序 = 跨源失败降级优先级
- v1 内置 provider：`tavily`、`exa`；可选 `base_url` 覆盖默认端点
- 读 API 返回脱敏 Key（`****` + 后 4 位）；写更新：`["__preserve__"]` 保留、`[]` 清空、明文列表替换

Agent `tools.json` 覆盖仅允许：

```json
{
  "version": 1,
  "web_search": {
    "enabled": true,
    "max_results": 8
  }
}
```

### 工具面

- 工具名：`web_search`
- 参数：`query`（必填，1–500 字符）、`max_results`（可选，1–20）
- 启用条件：`web_search.enabled == true` 且至少一个 enabled 源有非空 API Key
- 执行结果：`ToolResult::Completed`，`runtime=builtin_web_search`，`stdout` 为统一 JSON：
  - `provider`、`source_id`、`query`、`results[{title,url,snippet,score?}]`、`result_count`
- 失败旋转：同源内按策略换 Key；`401/403/429`/网络错误可旋转；同源耗尽后切下一源；全部失败返回 `ToolResult::Error`
- 无审批、无沙箱、无进程管理（与终端工具不同）

### 前端

- 全局工具页新增「网络搜索」配置：开关、max_results、timeout、key_selection、源排序、多 Key 编辑
- Agent 工具区：仅开关与 max_results 覆盖；密钥提示去全局配置

### 验证

- 配置反序列化兼容旧 `tool_defaults`（缺 `web_search` 时默认关闭）
- Key 选择策略、无 Key 不注册、脱敏读写
- Provider JSON 映射与错误路径单测
