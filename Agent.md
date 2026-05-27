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
- Agent 字段包含 `id`、`name`、`system_prompt`、`model`、`created_at`、`updated_at`
- Rust Web 框架使用 `axum`
- 前端使用 `React + Vite + TypeScript`
- 前后端分离，前端通过 API 调用后端
- API Key 只放在后端环境变量里，前端不接触密钥
- 聊天记录保存到 JSON 文件，但不作为长期记忆系统使用

## 环境变量

- `OPENAI_API_KEY`：OpenAI 兼容接口密钥
- `OPENAI_BASE_URL`：OpenAI 兼容接口 Base URL，默认 `https://api.openai.com/v1`
- `WEIAGENT_HOST`：后端监听地址，默认 `127.0.0.1`
- `WEIAGENT_PORT`：后端监听端口，默认 `8080`

JSON 数据目录固定使用项目根目录下的 `data`。模型配置绑定在 Agent 上，保存到 JSON 配置文件中；前端通过后端 `GET /api/models` 从 OpenAI 兼容接口获取可选模型，获取失败时仍可手填。

## 第一版 API

- `GET /api/agents`：获取 Agent 列表
- `GET /api/models`：从 OpenAI 兼容接口获取可选模型列表
- `POST /api/agents`：创建 Agent
- `PUT /api/agents/{agent_id}`：更新 Agent
- `DELETE /api/agents/{agent_id}`：删除 Agent
- `GET /api/chats/{agent_id}`：读取某个 Agent 的聊天记录
- `POST /api/chats/{agent_id}/messages`：非流式发送消息
- `POST /api/chats/{agent_id}/messages/stream`：SSE 流式发送消息
