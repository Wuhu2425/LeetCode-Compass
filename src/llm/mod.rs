//! 大模型接入层。
//!
//! - [`client`]：统一客户端，支持 OpenAI 兼容协议与 Anthropic 协议
//! - [`types`]：请求/响应 DTO 与错误信息处理
//!
//! 支持的厂商（通过 OpenAI 兼容协议）：OpenAI、DeepSeek、Moonshot、
//! 智谱 GLM、通义千问、Groq、SiliconFlow、OpenRouter、本地 Ollama /
//! LM Studio 等。只需填写对应的 Base URL 与模型名。

pub mod client;
pub mod types;

pub use client::LlmClient;
pub use types::WireMessage;
