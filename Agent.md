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
- Agent 字段包含 `id`、`name`、`system_prompt`、`model_provider_id`、`model_id`、`created_at`、`updated_at`
- Rust Web 框架使用 `axum`
- 前端使用 `React + Vite + TypeScript`
- 前后端分离，前端通过 API 调用后端
- API Key 只放在后端环境变量里，前端不接触密钥
- 聊天记录保存到 JSON 文件，但不作为长期记忆系统使用
- `cargo run dev` 会启动 Rust API 和 Vite 开发服务器；Vite 固定监听 `127.0.0.1:5173` 并提供前端热更新，API 代理目标从 `data/settings.json` 读取；正式二进制运行时使用编译进 exe 的前端资源

## JSON 配置

项目不再从 `.env` 或系统环境变量读取运行配置。`cargo run dev` 使用项目根目录下的 `data`；直接运行打包后的二进制文件时，使用二进制文件同目录下的 `data`，不存在则自动初始化。

- `data/settings.json`：保存应用监听地址、端口和日志级别；不存在时初始化为 `127.0.0.1:8080`、`info`
- `data/model_providers.json`：保存 OpenAI 兼容模型提供商、Base URL、API Key 和模型列表；API Key 本地明文保存，但接口不会完整返回给前端
- `data/agents.json`：保存 Agent 配置，Agent 通过 `model_provider_id` 和 `model_id` 绑定模型
- `data/conversations.json` 与 `data/conversations/{conversation_id}.json`：保存多会话元数据和消息

旧版 `model` 字段和 `data/chats/{agent_id}.json` 会在启动时迁移到 Provider 与 Conversation 结构。

## 第一版 API

- `GET /api/settings` / `PUT /api/settings`：读取和保存后端设置
- `GET /api/model-providers`：获取模型提供商列表
- `POST /api/model-providers`：创建模型提供商
- `PUT /api/model-providers/{provider_id}`：更新模型提供商
- `DELETE /api/model-providers/{provider_id}`：删除模型提供商
- `GET /api/agents`：获取 Agent 列表
- `POST /api/agents`：创建 Agent
- `PUT /api/agents/{agent_id}`：更新 Agent
- `DELETE /api/agents/{agent_id}`：删除 Agent
- `GET /api/agents/{agent_id}/conversations`：读取某个 Agent 的会话列表
- `POST /api/agents/{agent_id}/conversations`：创建会话
- `GET /api/conversations/{conversation_id}/messages`：读取会话消息
- `POST /api/conversations/{conversation_id}/messages`：非流式发送消息
- `POST /api/conversations/{conversation_id}/messages/stream`：SSE 流式发送消息
- `DELETE /api/conversations/{conversation_id}`：删除会话
