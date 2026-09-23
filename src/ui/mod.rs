//! UI 层：基于 egui 的即时模式界面。
//!
//! 模块划分与页面一一对应：
//!
//! | 模块        | 页面       | 职责                                   |
//! |-------------|------------|----------------------------------------|
//! | `home`      | 题库       | 全量题目列表、筛选、跳转浏览器         |
//! | `recommend` | 智能推荐   | 薄弱点诊断 + 推荐题目及理由            |
//! | `contest`   | 竞赛复盘   | Rating 趋势、稳定性、文字总结          |
//! | `assistant` | 学习助理   | 与大模型对话，基于账号数据回答         |
//! | `settings`  | 设置       | 账号绑定、会话凭据、大模型接入         |
//! | `theme`     | —          | 主题模式、语义色板、玻璃材质、自绘图标  |
//! | `widgets`   | —          | 跨页面复用的绘制组件与配色             |
//! | `guards`    | —          | 源码守卫测试（测试专用，强制颜色约定）  |
//!
//! ## 约定
//!
//! - 每个页面暴露一个 `pub fn draw(ui: &mut Ui, app: &mut CompassApp)` 入口，
//!   由 `app::draw_body()` 分发。
//! - **不得在 UI 层发起网络请求**。所有 IO 通过 `app` 上的方法派发到
//!   tokio 运行时，结果经消息通道回传。UI 只做绘制与状态变更派发。
//! - **颜色一律取自 [`theme::Palette`]，页面不得出现 `Color32` 字面量。**
//!   语义色（难度、状态、趋势）经 [`widgets`] 的取色函数获得。
//!
//!   这条约定由 [`guards`] 的源码扫描测试**强制**执行——改造前
//!   它只是一句注释，结果被违反了 188 次，导致主题无法覆盖全部组件。
//!   现在违反它会让 `cargo test` 失败。

pub mod assistant;
pub mod contest;
pub mod guards;
pub mod home;
pub mod recommend;
pub mod settings;
pub mod theme;
pub mod widgets;
