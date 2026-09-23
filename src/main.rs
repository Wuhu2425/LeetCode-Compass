// 隐藏 Windows 控制台窗口。
//
// 仅**发布构建**（`cargo build --release`）隐藏：双击 exe 只出现主窗口，
// 没有黑色命令行；`eprintln!` 的排障日志在发布版中静默丢弃。
// **调试构建**（`cargo run`）保留控制台，方便看运行日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LeetCode Compass — 应用入口。
//!
//! 负责：初始化日志、创建 tokio 运行时、加载配置、启动 egui 主循环。
//! 业务逻辑一律不放在本文件。

mod app;
mod config;
mod error;
mod fonts;
mod leetcode;
mod llm;
mod logic;
mod models;
mod storage;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    // 必须在创建任何 HTTP 客户端之前执行。
    //
    // 本项目对 reqwest 使用 `rustls-no-provider`（为规避 aws-lc-sys 的
    // CMake/NASM 编译依赖），该配置下 rustls 没有内置默认加密后端，
    // 若不在启动阶段显式安装，第一次发请求时会 panic。
    error::install_crypto_provider();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 600.0])
            .with_title("LeetCode Compass"),
        ..Default::default()
    };

    eframe::run_native(
        "LeetCode Compass",
        options,
        Box::new(|cc| Ok(Box::new(app::CompassApp::new(cc)?))),
    )
    .map_err(|e| anyhow::anyhow!("启动 GUI 失败: {e}"))?;

    Ok(())
}
