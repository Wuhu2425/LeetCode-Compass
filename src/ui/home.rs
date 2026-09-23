//! 首页：题库列表。
//!
//! ## 需求对应
//!
//! - "按顺序列出题库所有题目" → 默认按题号排序
//! - "以及题目相关标签" → 标签胶囊
//! - "显示是否完成" → 完成状态列（三态，未授权时显示"未知"）
//! - "点击题库的题目，跳转到浏览器并且打开题目" → 点击行调用 `open`
//!
//! ## 性能考虑
//!
//! 题库有 4000+ 条。egui 的 `ScrollArea::show_rows` 只构建可视区域内的
//! 行，因此即使总量很大也不会卡顿——前提是每帧只访问被要求绘制的那些行。
//! 这也是为什么筛选结果被缓存为索引数组而非克隆数据。

use egui::{RichText, Ui};

use crate::app::CompassApp;
use crate::models::{Difficulty, Problem, SolveStatus};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets;

/// 每行的高度。必须固定，`show_rows` 依赖它计算可视范围。
///
/// 由 26.0 提升到 30.0：中文在 26px 行高下上下留白过少，密集阅读时
/// 眼睛容易串行。30px 是本项目字号体系（正文 12.5pt）下的舒适下限。
const ROW_HEIGHT: f32 = 30.0;

/// 列宽定义。集中管理以保证表头与数据列对齐。
mod cols {
    pub const ID: f32 = 62.0;
    pub const TITLE: f32 = 400.0;
    pub const DIFFICULTY: f32 = 56.0;
    pub const AC_RATE: f32 = 64.0;
    pub const STATUS: f32 = 78.0;
    pub const TAGS: f32 = 300.0;
}

/// 绘制首页。
pub fn draw(ui: &mut Ui, app: &mut CompassApp) {
    let p = app.palette();

    draw_toolbar(ui, app, &p);
    ui.add_space(theme::space::MD);

    // 空状态处理。
    if app.problems.is_empty() {
        if app.syncing {
            widgets::loading_state(ui, &p, "正在首次拉取题库数据，这可能需要一分钟左右…");
        } else {
            widgets::empty_state(
                ui,
                &p,
                "◆",
                "题库尚未加载",
                "点击上方「同步题库」按钮从 LeetCode 拉取题目数据",
            );
            ui.vertical_centered(|ui| {
                if ui.button("立即同步").clicked() {
                    app.maybe_sync_problems(true);
                }
            });
        }
        return;
    }

    // 提示未授权会影响完成状态的准确性。
    if !app
        .config
        .try_read()
        .map(|c| c.is_authenticated())
        .unwrap_or(false)
    {
        widgets::info_banner(
            ui,
            &p,
            "当前未配置会话凭据，「完成状态」列无法显示真实进度。\
             如需查看哪些题已完成，请在「设置」页填入 LEETCODE_SESSION 与 csrftoken。",
        );
        ui.add_space(theme::space::MD);
    }

    let indices = app.filtered_problems();
    draw_stats_line(ui, app, &p, indices.len());
    ui.add_space(theme::space::SM);

    if indices.is_empty() {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "没有符合条件的题目",
            "试试清空筛选条件或更换关键词",
        );
        ui.vertical_centered(|ui| {
            if ui.button("清空筛选").clicked() {
                app.home.filter = Default::default();
                app.invalidate_filter_cache();
            }
        });
        return;
    }

    draw_table(ui, app, &p, &indices);
}

/// 工具栏：筛选、排序、搜索、刷新。
///
/// 整体包在一个玻璃卡片里：改造前这些控件直接铺在页面上，与下方内容
/// 缺少视觉分界，用户难以一眼分清"控制区"与"结果区"。
fn draw_toolbar(ui: &mut Ui, app: &mut CompassApp, p: &Palette) {
    let mut changed = false;

    widgets::glass_card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            // 搜索框。
            ui.label(
                RichText::new("搜索")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
            let resp = ui.add(
                egui::TextEdit::singleline(&mut app.home.filter.keyword)
                    .desired_width(180.0)
                    .hint_text("题号 / 标题"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.separator();

            // 难度筛选。
            ui.label(
                RichText::new("难度")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
            for d in Difficulty::all() {
                let on = app.home.filter.difficulties.contains(&d);
                if widgets::difficulty_pill(ui, p, d, on).clicked() {
                    if on {
                        app.home.filter.difficulties.retain(|x| *x != d);
                    } else {
                        app.home.filter.difficulties.push(d);
                    }
                    changed = true;
                }
            }

            ui.separator();

            // 状态筛选。仅在已授权时才有意义——未授权时所有状态都是
            // Unknown，按状态筛只会得到空结果，因此在这种情况下禁用并
            // 说明原因。
            let authenticated = app
                .config
                .try_read()
                .map(|c| c.is_authenticated())
                .unwrap_or(false);

            ui.label(
                RichText::new("状态")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
            for s in [
                SolveStatus::Solved,
                SolveStatus::Attempted,
                SolveStatus::Todo,
            ] {
                let on = app.home.filter.statuses.contains(&s);
                let resp = ui
                    .add_enabled(authenticated, egui::Button::selectable(on, s.label_zh()))
                    .on_disabled_hover_text("需先配置会话凭据才能按完成状态筛选");
                if resp.clicked() {
                    if on {
                        app.home.filter.statuses.retain(|x| *x != s);
                    } else {
                        app.home.filter.statuses.push(s);
                    }
                    changed = true;
                }
            }

            ui.separator();

            // 排序。
            ui.label(
                RichText::new("排序")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
            egui::ComboBox::from_id_salt("sort_combo")
                .selected_text(RichText::new(app.home.sort.label_zh()).size(theme::text::BODY_SMALL))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for s in crate::models::SortBy::all() {
                        if ui
                            .selectable_value(&mut app.home.sort, s, s.label_zh())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                });

            // 会员题开关。
            if ui
                .checkbox(
                    &mut app.home.filter.hide_paid_only,
                    RichText::new("隐藏会员题").size(theme::text::BODY_SMALL),
                )
                .changed()
            {
                changed = true;
            }

            ui.separator();

            // 刷新与清空。
            if app.syncing {
                ui.spinner();
                ui.label(
                    RichText::new("同步中…")
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_tertiary),
                );
            } else if ui
                .button(RichText::new("同步题库").size(theme::text::BODY_SMALL))
                .on_hover_text("从 LeetCode 重新拉取全部题目（缓存有效期内通常无需手动刷新）")
                .clicked()
            {
                app.maybe_sync_problems(true);
            }

            if ui
                .button(RichText::new("清空筛选").size(theme::text::BODY_SMALL))
                .clicked()
            {
                app.home.filter = Default::default();
                changed = true;
            }
        });

        // 标签筛选（可折叠，因为标签数量多）。
        ui.add_space(theme::space::XS);
        let tag_count = app.home.filter.tags.len();
        let header = if tag_count > 0 {
            format!("标签筛选（已选 {tag_count} 个）")
        } else {
            "标签筛选".to_string()
        };
        if ui
            .selectable_label(
                app.home.expanded_tags,
                RichText::new(header).size(theme::text::BODY_SMALL),
            )
            .clicked()
        {
            app.home.expanded_tags = !app.home.expanded_tags;
        }

        if app.home.expanded_tags {
            draw_tag_filter(ui, app, p, &mut changed);
        }
    });

    if changed {
        app.invalidate_filter_cache();
    }
}

/// 标签多选面板。
///
/// 只列出题库中实际出现的标签，并按题目数量降序——这样用户先看到
/// 主要知识点，不必在几百个标签里翻找。
fn draw_tag_filter(ui: &mut Ui, app: &mut CompassApp, p: &Palette, changed: &mut bool) {
    use std::collections::HashMap;

    // 统计标签出现频次。
    let mut counts: HashMap<(&str, &str), u32> = HashMap::new();
    for p in &app.problems {
        for t in &p.tags {
            if !t.slug.is_empty() {
                *counts.entry((t.name.as_str(), t.slug.as_str())).or_insert(0) += 1;
            }
        }
    }
    let mut sorted: Vec<((&str, &str), u32)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0 .1.cmp(b.0 .1)));

    egui::Frame::NONE
        .fill(p.bg_sunken)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(theme::space::SM as i8))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(
                            theme::space::XS,
                            theme::space::XS,
                        );
                        // 限制展示数量，避免渲染上千个按钮。
                        for ((name, slug), count) in sorted.iter().take(120) {
                            let selected = app
                                .home
                                .filter
                                .tags
                                .iter()
                                .any(|t| t.eq_ignore_ascii_case(slug));
                            let label = format!("{name} ({count})");
                            if ui
                                .selectable_label(
                                    selected,
                                    RichText::new(label).size(theme::text::CAPTION),
                                )
                                .clicked()
                            {
                                if selected {
                                    app.home
                                        .filter
                                        .tags
                                        .retain(|t| !t.eq_ignore_ascii_case(slug));
                                } else {
                                    app.home.filter.tags.push(slug.to_string());
                                }
                                *changed = true;
                            }
                        }
                    });
                });
        });
}

/// 统计信息行。
fn draw_stats_line(ui: &mut Ui, app: &CompassApp, p: &Palette, filtered: usize) {
    let total = app.problems.len();

    // 从题库实际数据中统计各难度数量。
    let mut easy = 0usize;
    let mut medium = 0usize;
    let mut hard = 0usize;
    for prob in &app.problems {
        match prob.difficulty {
            Difficulty::Easy => easy += 1,
            Difficulty::Medium => medium += 1,
            Difficulty::Hard => hard += 1,
        }
    }

    ui.horizontal_wrapped(|ui| {
        let text = if filtered == total {
            format!("共 {total} 道题")
        } else {
            format!("筛选出 {filtered} 道 / 共 {total} 道")
        };
        ui.label(
            RichText::new(text)
                .size(theme::text::BODY_SMALL)
                .strong()
                .color(p.text_primary),
        );

        ui.separator();
        ui.label(
            RichText::new(format!("简单 {easy}"))
                .size(theme::text::CAPTION)
                .color(widgets::difficulty_color(p, Difficulty::Easy)),
        );
        ui.label(
            RichText::new(format!("中等 {medium}"))
                .size(theme::text::CAPTION)
                .color(widgets::difficulty_color(p, Difficulty::Medium)),
        );
        ui.label(
            RichText::new(format!("困难 {hard}"))
                .size(theme::text::CAPTION)
                .color(widgets::difficulty_color(p, Difficulty::Hard)),
        );
    });

    // 难度构成比例条。数值已在上方给出，这里提供的是"一眼看出结构"的
    // 视觉判断，因此不重复标注数字。
    if total > 0 {
        ui.add_space(theme::space::XS);
        ui.allocate_ui_with_layout(
            egui::vec2(420.0, 6.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                widgets::segmented_bar(
                    ui,
                    p,
                    &[
                        (easy as f64, widgets::difficulty_color(p, Difficulty::Easy)),
                        (
                            medium as f64,
                            widgets::difficulty_color(p, Difficulty::Medium),
                        ),
                        (hard as f64, widgets::difficulty_color(p, Difficulty::Hard)),
                    ],
                    6.0,
                );
            },
        );
    }

    // 账号进度与活跃度单独一行，避免主行过长导致换行错位。
    draw_account_progress_line(ui, app, p, total);
}

/// 账号进度与活跃度行。
fn draw_account_progress_line(ui: &mut Ui, app: &CompassApp, p: &Palette, total: usize) {
    // 仅在已授权时才有意义——否则 `solved` 是公开数据但活跃度不可得。
    let authenticated = app
        .config
        .try_read()
        .map(|c| c.is_authenticated())
        .unwrap_or(false);
    if !authenticated {
        return;
    }

    let Some(profile) = &app.profile else {
        return;
    };

    ui.horizontal_wrapped(|ui| {
        let solved = profile.solved.total();
        // 分母优先用官方全球总量：本地缓存可能因会员题等原因少于真实总量。
        let denom = app
            .global_counts
            .as_ref()
            .map(|c| c.total())
            .filter(|n| *n > 0)
            .unwrap_or(total as u32);
        let ratio = if denom > 0 {
            solved as f32 / denom as f32
        } else {
            0.0
        };

        ui.label(
            RichText::new(format!(
                "已通过 {solved} / {denom} 道（{:.1}%）",
                ratio * 100.0
            ))
            .size(theme::text::CAPTION)
            .color(p.success),
        )
        .on_hover_text("分母为 LeetCode 官方公布的全球题量");

        // 难度维度的已解明细。
        ui.label(
            RichText::new(format!(
                "简 {} / 中 {} / 难 {}",
                profile.solved.easy, profile.solved.medium, profile.solved.hard
            ))
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );

        // 活跃度：连续天数是最强的坚持度指标。
        if let Some(streak) = profile.streak {
            if streak > 0 {
                ui.label(
                    RichText::new(format!("连续活跃 {streak} 天"))
                        .size(theme::text::CAPTION)
                        .color(p.warning),
                );
            }
        }
        if let Some(days) = profile.total_active_days {
            if days > 0 {
                ui.label(
                    RichText::new(format!("累计活跃 {days} 天"))
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }
        }
    });
}

/// 题目表格。
fn draw_table(ui: &mut Ui, app: &mut CompassApp, p: &Palette, indices: &[usize]) {
    let authenticated = app
        .config
        .try_read()
        .map(|c| c.is_authenticated())
        .unwrap_or(false);

    // 表头。用实心底（非玻璃）——表头需要与数据行清晰分界，
    // 半透明会让下方滚动的行透出来，破坏"表头"的空间语义。
    egui::Frame::NONE
        .fill(p.bg_sunken)
        .corner_radius(egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        })
        .inner_margin(egui::Margin::symmetric(
            theme::space::SM as i8,
            theme::space::XS as i8 + 1,
        ))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let header_size = theme::text::CAPTION;

                // 表头单元格统一走这个宏，避免 7 处重复的排布样板。
                macro_rules! header_cell {
                    ($w:expr, $label:expr) => {
                        ui.allocate_ui_with_layout(
                            egui::vec2($w, ROW_HEIGHT * 0.8),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    RichText::new($label)
                                        .size(header_size)
                                        .strong()
                                        .color(p.text_secondary),
                                );
                            },
                        );
                    };
                }

                header_cell!(cols::ID, "题号");
                header_cell!(cols::TITLE, "标题");
                header_cell!(cols::DIFFICULTY, "难度");
                header_cell!(cols::AC_RATE, "通过率");

                ui.allocate_ui_with_layout(
                    egui::vec2(cols::STATUS, ROW_HEIGHT * 0.8),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new("状态")
                                .size(header_size)
                                .strong()
                                .color(p.text_secondary),
                        );
                        if !authenticated {
                            ui.label(
                                RichText::new("(未授权)")
                                    .size(theme::text::CAPTION - 1.5)
                                    .color(p.text_disabled),
                            );
                        }
                    },
                );
                ui.label(
                    RichText::new("标签")
                        .size(header_size)
                        .strong()
                        .color(p.text_secondary),
                );
            });
        });

    // 行区域。使用虚拟滚动：只绘制可视范围内的行。
    let row_height = ROW_HEIGHT;
    let mut clicked_slug: Option<String> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, indices.len(), |ui, range| {
            let problems = &app.problems;
            for i in range {
                let Some(&idx) = indices.get(i) else { continue };
                let Some(prob) = problems.get(idx) else {
                    continue;
                };

                let row_resp = draw_row(ui, p, prob, i);
                if row_resp.clicked() {
                    clicked_slug = Some(prob.title_slug.clone());
                }
                if row_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        });

    // 在滚动区域外执行跳转，避免在闭包内可变借用 app 造成冲突。
    if let Some(slug) = clicked_slug {
        open_problem_in_browser(app, &slug);
    }
}

/// 绘制单行题目。返回该行的点击响应。
///
/// 整行可点击：`Sense::click()` 让整行成为一个响应区域，
/// 而不仅是标题文字。
///
/// **刻意不使用半透明底**：表格是内容密集区，若行底半透明，滚动时
/// 下层内容会透出，可读性显著下降。
///
/// 悬停底同样取**不透明**色（`p.hover_overlay`）。这里曾用过低透明度
/// 叠加色，结果两套主题双双失效：浅色下被冲成纯白（毫无反馈），
/// 深色下也冲成纯白（整行变白，标题对比度掉到 1.08:1，完全看不清）。
/// 反馈类颜色必须先用"与相邻状态可区分"做验收，通透度是次要目标。
fn draw_row(ui: &mut Ui, p: &Palette, prob: &Problem, row_index: usize) -> egui::Response {
    let full_width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(full_width, ROW_HEIGHT), egui::Sense::click());

    // 斑马纹 + 悬停高亮。
    //
    // 长表格里斑马纹把行与行分开，避免视线横向漂移；悬停则给出
    // "这一行可点"的即时反馈。
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.hover_overlay);
    } else if row_index % 2 == 1 {
        ui.painter().rect_filled(rect, 4.0, p.bg_sunken);
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(theme::space::SM, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    // 单元格排布样板统一走宏。
    macro_rules! cell {
        ($w:expr, $body:expr) => {
            child.allocate_ui_with_layout(
                egui::vec2($w, ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                $body,
            );
        };
    }

    // 题号。
    cell!(cols::ID, |ui: &mut Ui| {
        ui.label(
            RichText::new(&prob.frontend_id)
                .size(theme::text::MONO)
                .color(p.text_tertiary),
        );
    });

    // 标题（含会员标记）。
    cell!(cols::TITLE, |ui: &mut Ui| {
        ui.label(
            RichText::new(&prob.title)
                .size(theme::text::BODY_SMALL)
                .color(p.text_primary),
        );
        if prob.is_paid_only {
            ui.label(
                RichText::new(" 会员 ")
                    .size(theme::text::CAPTION - 2.0)
                    .color(p.warning)
                    .background_color(p.bg_sunken),
            );
        }
    });

    // 难度。
    cell!(cols::DIFFICULTY, |ui: &mut Ui| {
        widgets::difficulty_badge(ui, p, prob.difficulty);
    });

    // 通过率。
    cell!(cols::AC_RATE, |ui: &mut Ui| {
        ui.label(
            RichText::new(format!("{:.1}%", prob.ac_rate))
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
    });

    // 完成状态。
    cell!(cols::STATUS, |ui: &mut Ui| {
        widgets::status_badge(ui, p, prob.status);
    });

    // 标签。
    cell!(cols::TAGS, |ui: &mut Ui| {
        widgets::tag_pills(ui, p, &prob.tags, 3);
    });

    response.on_hover_text(format!(
        "{}. {}\n\n难度：{}\n通过率：{:.1}%\n状态：{}\n标签：{}\n\n点击在浏览器中打开题目",
        prob.frontend_id,
        prob.title,
        prob.difficulty.label_zh(),
        prob.ac_rate,
        prob.status.label_zh(),
        prob.tags
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join("、")
    ))
}

/// 避免把远端返回的任意字符串交给系统 shell。
fn open_problem_in_browser(app: &mut CompassApp, slug: &str) {
    let Some(p) = app.problems.iter().find(|p| p.title_slug == slug) else {
        app.show_toast(crate::app::ToastKind::Error, "未找到该题目");
        return;
    };

    // 站点随登录会话：登的是国内站跳 leetcode.cn，登的是国际站跳 leetcode.com。
    let Some(url) = p.url(app.current_site()) else {
        app.show_toast(
            crate::app::ToastKind::Error,
            format!("题目标识「{}」不合法，无法构造链接", p.title_slug),
        );
        return;
    };

    match open::that(&url) {
        Ok(()) => app.show_toast(crate::app::ToastKind::Info, format!("已在浏览器打开：{}", p.title)),
        Err(e) => app.show_toast(
            crate::app::ToastKind::Error,
            format!("无法打开浏览器：{e}"),
        ),
    }
}
