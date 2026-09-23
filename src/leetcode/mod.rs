//! LeetCode 数据接入层。
//!
//! 三个子模块分工明确：
//! - [`queries`]：GraphQL 查询文本（远端契约的唯一来源）
//! - [`types`]：响应 DTO（远端 schema 的唯一映射点）
//! - [`client`]：HTTP 传输与凭据注入
//!
//! 领域模型位于 [`crate::models`]，与远端解耦。持有本模块类型的地方应
//! 尽快转换为领域模型，避免远端细节扩散到业务层与 UI 层。

pub mod client;
pub mod queries;
pub mod types;

pub use client::LeetCodeClient;
