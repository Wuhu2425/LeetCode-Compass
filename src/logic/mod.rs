//! 业务逻辑层。
//!
//! - [`recommend`]：推荐引擎（薄弱知识点驱动 + 进度感知）
//! - [`contest`]：竞赛记录分析（趋势 / 稳定性 / 解题率）
//! - [`assistant`]：AI 小助理的提示词编排
//!
//! 本层是**纯函数式的**：不持有状态、不做 I/O、不依赖 UI。
//! 这使得它可以被完整单元测试，也让 UI 层与网络层的改动不会波及算法
//! 正确性。
//!
//! 调用方一律通过完整路径引用（如 `logic::recommend::RecommendationEngine`），
//! 不做 re-export——本层类型名较长且与 UI 层类型有重名，显式路径反而更清晰。

pub mod assistant;
pub mod contest;
pub mod recommend;
