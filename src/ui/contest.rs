//! 竞赛复盘页：分析参赛记录并给出结构化总结。
//!
//! 页面结构：
//! 1. 顶部操作区（刷新账号数据）
//! 2. 关键指标概览（场次 / 当前 Rating / 均值 / 峰值 / 稳定性）
//! 3. Rating 趋势折线图（含趋势方向与变化量）
//! 4. 逐场明细表（时间升序，最新在上）
//! 5. 文字总结要点
//!
//! ## 设计要点
//!
//! **"给出总结"是本页面的核心交付**，不是把原始数据摆出来就完事。因此
//! 分析结论（`insights`）被放在显眼位置，且每一条都附带依据（如"波动
//! ±87 分"），而不是空泛的"要加油"。
//!
//! 另外，**数据不足时必须明确说"数据不足"**，不能用 0 或空图冒充结论。
//! 参赛 0 场、1 场、2 场的情形分别都有对应提示。
//!
//! 主题化：本文件**不含任何 `Color32::from_*` 字面量**，全部取色经
//! `&Palette`。这一约束由 `ui::guards` 的源码扫描测试强制。
//!
//! ## 胶囊标签的背景色问题
//!
//! 改造前用 `tint(color)` 把前景色以极低 alpha 叠在白底上做背景——这个
//! 做法**只在浅色主题下成立**。深色主题下需要的是"同样低 alpha、但叠加
//! 在深底上"的效果，且文字本身要换成亮色阶。现在 `chip` 接收
//! `ChipColors`（前景 + 背景成对给出），由色板负责这对取值。

use egui::{Color32, RichText, Ui};

use crate::app::CompassApp;
use crate::logic::contest::{ContestAnalysis, Stability, Trend};
use crate::models::ContestRecord;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets;

/// 逐场明细表最多显示的行数。
///
/// 参赛数百场的老手不需要把整张表滚动到底——图与总结已足够。限制行数
/// 同时也避免每帧构建大量 widget。
const MAX_DETAIL_ROWS: usize = 50;

/// 折线图高度。
const CHART_HEIGHT: f32 = 150.0;

/// 绘制竞赛复盘页。
pub fn draw(ui: &mut Ui, app: &mut CompassApp) {
    let p = app.palette();

    draw_header(ui, &p, app);
    ui.add_space(theme::space::MD);

    let has_account = app
        .config
        .try_read()
        .map(|c| c.has_account())
        .unwrap_or(false);

    if !has_account {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "尚未绑定 LeetCode 账号",
            "竞赛数据需要通过账号读取，请先到「设置」页填写用户名",
        );
        ui.vertical_centered(|ui| {
            if ui.button("前往设置").clicked() {
                app.page = crate::app::Page::Settings;
            }
        });
        return;
    }

    if app.refreshing_account {
        widgets::loading_state(ui, &p, "正在获取竞赛记录…");
        return;
    }

    // 站点能力边界优先于"暂无数据"判断。
    //
    // 这两者的用户认知完全不同：一种是"我没参赛/没刷新"（可通过操作改变），
    // 另一种是"这个站点根本不提供该接口"（无论怎么操作都不会有数据）。
    // 若混为一谈，中国站用户会反复点"刷新数据"却永远拿不到结果。
    if !app.current_site().supports_contest() {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "竞赛复盘暂不支持中国站",
            "力扣中国站的竞赛接口与国际站字段不同，当前版本尚未适配。\
             题库浏览、账号画像与题目推荐均可正常使用；\
             如需竞赛复盘，请在设置页改用国际站 leetcode.com 账号。",
        );
        ui.vertical_centered(|ui| {
            if ui.button("前往设置").clicked() {
                app.page = crate::app::Page::Settings;
            }
        });
        return;
    }

    let Some(analysis) = app.contest.analysis.clone() else {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "暂无竞赛数据",
            "可能是你还没参加过周赛，或账号数据尚未刷新。点击上方「刷新数据」获取。",
        );
        ui.vertical_centered(|ui| {
            if ui.button("刷新数据").clicked() {
                app.refresh_account();
            }
        });
        return;
    };

    // 参赛 0 场：分析结果存在但无有效场次。
    if analysis.attended_count == 0 {
        widgets::info_banner(
            ui,
            &p,
            "账号已绑定，但没有查询到实际参赛记录。\
             若你确实参加过周赛，可能是旧场次未被接口返回，或使用了非主账号。",
        );
        ui.add_space(theme::space::MD);
        draw_metrics(ui, &p, &analysis);
        return;
    }

    draw_metrics(ui, &p, &analysis);
    ui.add_space(theme::space::MD);

    draw_rating_trend(ui, &p, &analysis, &app.contest_records);
    ui.add_space(theme::space::MD);

    draw_activity(ui, &p, &app.contest_records);
    ui.add_space(theme::space::MD);

    draw_insights(ui, &p, &analysis);
    ui.add_space(theme::space::MD);

    draw_details(ui, &p, &app.contest_records);
}

/// 顶部：标题 + 刷新。
fn draw_header(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::glass_card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::SM;

            ui.label(
                RichText::new("竞赛复盘")
                    .size(theme::text::SUBHEADING)
                    .strong()
                    .color(p.text_primary),
            );

            if ui
                .button("刷新数据")
                .on_hover_text("重新从 LeetCode 拉取竞赛记录与排名")
                .clicked()
            {
                app.refresh_account();
            }

            if app.refreshing_account {
                ui.spinner();
                ui.label(
                    RichText::new("刷新中…")
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }

            ui.separator();
            ui.label(
                RichText::new("数据来源：LeetCode 公开竞赛接口")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        });
    });
}

/// 关键指标概览卡片。
fn draw_metrics(ui: &mut Ui, p: &Palette, a: &ContestAnalysis) {
    widgets::card(ui, p, |ui| {
        // 第一行：核心数值。
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::XL;

            widgets::metric_label(ui, p, "参赛场次", &format!("{}", a.attended_count));

            widgets::metric_label(
                ui,
                p,
                "当前 Rating",
                &a.latest_rating.map(|v| format!("{v:.0}")).unwrap_or_else(|| "—".into()),
            );

            widgets::metric_label(
                ui,
                p,
                "平均 Rating",
                &a.average_rating.map(|v| format!("{v:.0}")).unwrap_or_else(|| "—".into()),
            );

            widgets::metric_label(
                ui,
                p,
                "最高 Rating",
                &a.peak
                    .as_ref()
                    .map(|(v, _)| format!("{v:.0}"))
                    .unwrap_or_else(|| "—".into()),
            );

            widgets::metric_label(
                ui,
                p,
                "波动幅度",
                &a.rating_stddev
                    .map(|v| format!("±{v:.0}"))
                    .unwrap_or_else(|| "—".into()),
            );

            widgets::metric_label(
                ui,
                p,
                "平均解题率",
                &a.average_solve_ratio
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "—".into()),
            );
        });

        ui.add_space(theme::space::SM);
        ui.separator();
        ui.add_space(theme::space::XS);

        // 第二行：定性评级。
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::SM;

            trend_chip(ui, p, a.trend, a.rating_delta);
            stability_chip(ui, p, a.stability);

            if a.perfect_rounds > 0 {
                chip(
                    ui,
                    p,
                    &format!("全对 {} 场", a.perfect_rounds),
                    ChipColors::positive(p),
                );
            }

            if let Some(pct) = a.average_percentile {
                chip(
                    ui,
                    p,
                    &format!("平均排名前 {pct:.0}%"),
                    ChipColors::accent(p),
                );
            }

            if let Some(top) = a.summary.top_percentage {
                chip(
                    ui,
                    p,
                    &format!("全局前 {top:.1}%"),
                    ChipColors::accent(p),
                );
            }

            if let Some(rank) = a.summary.global_ranking {
                chip(
                    ui,
                    p,
                    &format!("全球排名 {}", format_thousands(rank)),
                    ChipColors::neutral(p),
                );
            }
        });
    });
}

/// Rating 趋势折线图。
///
/// 曲线数据取自原始记录（按时间升序），而非 `ContestAnalysis` 的聚合量——
/// 后者只保留均值/峰值，画不出真实走势。
fn draw_rating_trend(
    ui: &mut Ui,
    p: &Palette,
    a: &ContestAnalysis,
    records: &[ContestRecord],
) {
    widgets::card(ui, p, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Rating 变化趋势")
                    .size(theme::text::SUBHEADING)
                    .strong()
                    .color(p.text_primary),
            );
            ui.label(
                RichText::new(format!("{}（{} 场）", a.trend.symbol(), a.trend.label_zh()))
                    .size(theme::text::CAPTION)
                    .color(trend_color(p, a.trend)),
            );
        });

        ui.label(
            RichText::new("横轴按时间由早到晚排列；纵轴为每场 Rating")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        // 按时间升序取实际参赛场次的 Rating 序列。
        let mut series: Vec<(i64, f64)> = records
            .iter()
            .filter(|r| r.attended)
            .map(|r| (r.start_time, r.rating))
            .collect();
        series.sort_by_key(|(t, _)| *t);

        let values: Vec<f64> = series.iter().map(|(_, v)| *v).collect();

        if values.len() < 2 {
            widgets::info_banner(
                ui,
                p,
                "参赛场次不足 2 场，暂时画不出有意义的趋势曲线。\
                 建议至少连续参加 4 场周赛后再来查看。",
            );
            return;
        }

        widgets::line_chart(ui, p, &values, CHART_HEIGHT, trend_color(p, a.trend), "Rating");

        // 图下方给出首末对比，方便快速读取整体涨幅。
        ui.add_space(theme::space::XS);
        if let (Some(first), Some(last)) = (values.first(), values.last()) {
            let delta = last - first;
            let color = delta_color(p, delta);

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("首场 {first:.0} → 最新 {last:.0}"))
                        .size(theme::text::CAPTION)
                        .color(p.text_secondary),
                );
                ui.label(
                    RichText::new(format!("累计 {delta:+.0}"))
                        .size(theme::text::CAPTION)
                        .strong()
                        .color(color),
                );
            });
        }
    });
}

/// 参赛活跃度（频率与最长间隔）。
fn draw_activity(ui: &mut Ui, p: &Palette, records: &[ContestRecord]) {
    let Some(per_month) = crate::logic::contest::contests_per_month(records) else {
        return;
    };

    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("参赛活跃度")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.add_space(theme::space::XS);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::XL;

            widgets::metric_label(ui, p, "平均每月参赛", &format!("{per_month:.1} 场"));

            if let Some(gap) = crate::logic::contest::longest_gap_days(records) {
                widgets::metric_label(ui, p, "最长停赛间隔", &format!("{gap:.0} 天"));
            }

            if records.len() >= 2 {
                let first = records.iter().map(|r| r.start_time).min().unwrap_or(0);
                let last = records.iter().map(|r| r.start_time).max().unwrap_or(0);
                let span_days = ((last - first).max(0) as f64) / 86_400.0;
                widgets::metric_label(ui, p, "参赛跨度", &format!("{span_days:.0} 天"));
            }
        });

        ui.add_space(theme::space::XS);
        ui.label(
            RichText::new(
                "周赛通常每周一次。若「平均每月参赛」明显低于 4，\
                 说明投入不连续，进步会明显变慢。",
            )
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );
    });
}

/// 文字总结要点。本页面的核心交付。
fn draw_insights(ui: &mut Ui, p: &Palette, a: &ContestAnalysis) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("分析总结")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new("基于参赛历史自动生成，每条结论均附依据")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        if a.insights.is_empty() {
            ui.label(
                RichText::new("参赛数据过少，暂时无法生成有意义的总结。")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
            return;
        }

        for (i, line) in a.insights.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}.", i + 1))
                        .size(theme::text::CAPTION)
                        .strong()
                        .color(p.accent),
                );
                ui.label(
                    RichText::new(line)
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_secondary),
                );
            });
            ui.add_space(theme::space::XS);
        }
    });
}

/// 逐场明细表（最新在前）。
fn draw_details(ui: &mut Ui, p: &Palette, records: &[ContestRecord]) {
    // 只保留实际参赛的场次，并按时间倒序（最新在前）。
    let mut rows: Vec<&ContestRecord> = records.iter().filter(|r| r.attended).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.start_time));

    if rows.is_empty() {
        return;
    }

    ui.label(
        RichText::new(format!("逐场明细（共 {} 场）", rows.len()))
            .size(theme::text::SUBHEADING)
            .strong()
            .color(p.text_primary),
    );
    ui.label(
        RichText::new("按时间倒序，最新一场在最上方")
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
    );
    ui.add_space(theme::space::XS);

    widgets::card(ui, p, |ui| {
        // 表头。
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::SM;
            header_cell(ui, p, "日期", 92.0);
            header_cell(ui, p, "竞赛", 210.0);
            header_cell(ui, p, "Rating", 66.0);
            header_cell(ui, p, "名次", 100.0);
            header_cell(ui, p, "百分位", 66.0);
            header_cell(ui, p, "解题", 60.0);
        });
        ui.separator();

        let shown = rows.len().min(MAX_DETAIL_ROWS);
        for r in rows.iter().take(shown) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = theme::space::SM;

                body_cell(ui, &format_date(r.start_time), 92.0, p.text_secondary);
                body_cell(ui, &truncate_chars(&r.title, 26), 210.0, p.text_primary);

                // Rating 用主题色区分——这是用户最关心的数值。
                body_cell(ui, &format!("{:.0}", r.rating), 66.0, rating_color(p, r.rating));

                body_cell(
                    ui,
                    &format!(
                        "{} / {}",
                        format_thousands(r.ranking),
                        format_thousands(r.total_participants)
                    ),
                    100.0,
                    p.text_secondary,
                );

                let pct = r
                    .percentile()
                    .map(|v| format!("前 {v:.0}%"))
                    .unwrap_or_else(|| "—".into());
                // 百分位越小越好，用颜色标注优异表现。
                body_cell(ui, &pct, 66.0, percentile_color(p, r.percentile()));

                // 解题进度。全对时高亮。
                let solved_text = format!("{}/{}", r.problems_solved, r.total_problems);
                let solved_color = if r.total_problems > 0 && r.problems_solved >= r.total_problems
                {
                    p.success
                } else {
                    p.text_secondary
                };
                body_cell(ui, &solved_text, 60.0, solved_color);
            });
        }

        if rows.len() > shown {
            ui.add_space(theme::space::XS);
            ui.label(
                RichText::new(format!("另有 {} 场较早记录未显示", rows.len() - shown))
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        }
    });
}

/// 供助理页复用的紧凑摘要。
pub fn compact_summary(ui: &mut Ui, app: &CompassApp) {
    let p = app.palette();

    let Some(a) = &app.contest.analysis else {
        ui.label(
            RichText::new("尚无竞赛分析结果")
                .size(theme::text::BODY_SMALL)
                .color(p.text_tertiary),
        );
        return;
    };

    if a.attended_count == 0 {
        ui.label(
            RichText::new("未查询到参赛记录")
                .size(theme::text::BODY_SMALL)
                .color(p.text_tertiary),
        );
        return;
    }

    ui.label(
        RichText::new(format!(
            "参赛 {} 场｜当前 {:.0}｜平均 {:.0}｜趋势：{}",
            a.attended_count,
            a.latest_rating.unwrap_or(0.0),
            a.average_rating.unwrap_or(0.0),
            a.trend.label_zh()
        ))
        .size(theme::text::BODY_SMALL)
        .color(p.text_secondary),
    );

    if let Some(first) = a.insights.first() {
        ui.label(
            RichText::new(format!("· {first}"))
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
    }
}

// ---------------------------------------------------------------------------
// 语义取色
// ---------------------------------------------------------------------------

/// 趋势颜色：上升绿、持平灰、下滑红。
fn trend_color(p: &Palette, t: Trend) -> Color32 {
    match t {
        Trend::Improving => p.success,
        Trend::Stable => p.neutral,
        Trend::Declining => p.danger,
        Trend::Insufficient => p.text_tertiary,
    }
}

/// 稳定性颜色：越稳定越绿。
fn stability_color(p: &Palette, s: Stability) -> Color32 {
    match s {
        Stability::Consistent => p.success,
        Stability::Moderate => p.warning,
        Stability::Volatile => p.danger,
        Stability::Insufficient => p.text_tertiary,
    }
}

/// 累计变化量颜色：正绿、负红、零中性。
fn delta_color(p: &Palette, delta: f64) -> Color32 {
    if delta > 0.0 {
        p.success
    } else if delta < 0.0 {
        p.danger
    } else {
        p.text_tertiary
    }
}

/// Rating 分档颜色（四档：1900+ / 1600+ / 1400+ / 其余）。
///
/// 阈值来自 LeetCode 竞赛分段惯例；四档映射到"危险→警示→良好→中性"
/// 四组语义色，保证在两套主题下都有足够对比度。
fn rating_color(p: &Palette, rating: f64) -> Color32 {
    if rating >= 1900.0 {
        p.danger
    } else if rating >= 1600.0 {
        p.warning
    } else if rating >= 1400.0 {
        p.success
    } else {
        p.text_secondary
    }
}

/// 百分位颜色：越小越好。
fn percentile_color(p: &Palette, percentile: Option<f64>) -> Color32 {
    match percentile {
        Some(v) if v <= 10.0 => p.success,
        Some(v) if v <= 30.0 => p.info,
        _ => p.text_secondary,
    }
}

// ---------------------------------------------------------------------------
// 绘制辅助
// ---------------------------------------------------------------------------

/// 胶囊标签的前景 / 背景成对配色。
///
/// **必须成对**而不是单给前景色再由背景调 alpha：深色主题下的"低 alpha
/// 前景色叠加"会得到一种不可控的中间色，文字与底色的对比无法保证。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChipColors {
    fg: Color32,
    bg: Color32,
}

impl ChipColors {
    /// 由任意语义色生成胶囊配色。
    ///
    /// 前景直接用该语义色（色板已保证其与背景的对比度），背景由
    /// [`theme::tint_bg`] 派生——该规则属于配色策略，因此定义在
    /// `theme` 而非本文件：散在页面里会让各页胶囊深浅不一，也会触发
    /// `ui::guards` 的"页面不得自行构造颜色"检查。
    fn of(color: Color32) -> Self {
        Self {
            fg: color,
            bg: theme::tint_bg(color),
        }
    }

    fn positive(p: &Palette) -> Self {
        Self::of(p.success)
    }

    fn accent(p: &Palette) -> Self {
        Self::of(p.accent)
    }

    fn neutral(p: &Palette) -> Self {
        Self::of(p.text_tertiary)
    }
}

/// 趋势标签。
fn trend_chip(ui: &mut Ui, p: &Palette, t: Trend, delta: f64) {
    let color = trend_color(p, t);
    let text = if t == Trend::Insufficient {
        format!("{} {}", t.symbol(), t.label_zh())
    } else {
        // 变化量带符号，直观反映幅度。
        format!("{} {}（{:+.0}）", t.symbol(), t.label_zh(), delta)
    };
    chip(ui, p, &text, ChipColors::of(color));
}

/// 稳定性标签。
fn stability_chip(ui: &mut Ui, p: &Palette, s: Stability) {
    let color = stability_color(p, s);
    chip(ui, p, s.label_zh(), ChipColors::of(color));
}

/// 通用胶囊标签。
fn chip(ui: &mut Ui, _p: &Palette, text: &str, colors: ChipColors) {
    ui.label(
        RichText::new(format!(" {text} "))
            .size(theme::text::CAPTION)
            .color(colors.fg)
            .background_color(colors.bg),
    );
}

/// 表头单元格（固定宽度，保证列对齐）。
fn header_cell(ui: &mut Ui, p: &Palette, text: &str, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 16.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                RichText::new(text)
                    .size(theme::text::CAPTION)
                    .strong()
                    .color(p.text_tertiary),
            );
        },
    );
}

/// 表体单元格（固定宽度，保证列对齐）。
fn body_cell(ui: &mut Ui, text: &str, width: f32, color: Color32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                RichText::new(text)
                    .size(theme::text::BODY_SMALL)
                    .color(color),
            );
        },
    );
}

/// 把 Unix 秒格式化为 `YYYY-MM-DD`。
///
/// 手写而非引入 `chrono`：只需要日期部分，且必须本地时区正确。
/// 采用 UTC+8（用户所在时区）偏移后再做民用日期换算。
pub(crate) fn format_date(unix_secs: i64) -> String {
    // 周赛时间戳为 UTC 秒。加 8 小时得到北京时间，再做整除运算。
    let shifted = unix_secs + 8 * 3600;
    let days = shifted.div_euclid(86_400);

    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 由"自 1970-01-01 起的天数"推算公历日期。
///
/// 采用 Howard Hinnant 的 `civil_from_days` 算法（公有领域），
/// 对未来/过去日期均有效，且不含分支猜测。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 千分位格式化。
fn format_thousands(n: u32) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// 按字符数截断（不会切坏 UTF-8）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两套主题，供取色断言遍历。
    fn palettes() -> [Palette; 2] {
        [Palette::light(), Palette::dark()]
    }

    #[test]
    fn date_conversion_matches_known_values() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(format_date(1_704_067_200), "2024-01-01");
        // 1970-01-01 00:00:00 UTC = 0，加 8 小时仍在 1970-01-01
        assert_eq!(format_date(0), "1970-01-01");
        // 2000-02-29 00:00:00 UTC（闰日）= 951782400
        assert_eq!(format_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn date_conversion_handles_pre_epoch() {
        // 1969-12-31 00:00:00 UTC = -86400
        let s = format_date(-86_400);
        assert!(s.starts_with("1969-12-"), "应正确处理负时间戳，实际得到 {s}");
    }

    #[test]
    fn thousands_separator_formats_correctly() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1_000), "1,000");
        assert_eq!(format_thousands(12_345), "12,345");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        // 中文按字符计数，不应 panic 或产出乱码。
        let zh = "第 300 场周赛";
        let t = truncate_chars(zh, 4);
        assert_eq!(t.chars().count(), 4, "截断后字符数应为 4，实际 {t:?}");
    }

    #[test]
    fn trend_colors_are_distinct() {
        for p in palettes() {
            assert_ne!(
                trend_color(&p, Trend::Improving),
                trend_color(&p, Trend::Declining),
                "上升与下滑颜色必须可区分——这是用户最先看的两个状态"
            );
        }
    }

    #[test]
    fn stability_colors_are_distinct() {
        for p in palettes() {
            assert_ne!(
                stability_color(&p, Stability::Consistent),
                stability_color(&p, Stability::Volatile)
            );
        }
    }

    /// **主题化改造的核心断言**：所有语义取色必须随主题变化。
    ///
    /// 改造前这些函数是纯函数、与主题无关，返回的深绿色在深色主题下
    /// 几乎融入背景。现在断言它们在两套主题下取值不同。
    #[test]
    fn all_semantic_colors_follow_the_theme() {
        let l = Palette::light();
        let d = Palette::dark();

        for t in [Trend::Improving, Trend::Stable, Trend::Declining, Trend::Insufficient] {
            assert_ne!(
                trend_color(&l, t),
                trend_color(&d, t),
                "趋势色 {t:?} 未随主题变化"
            );
        }

        for s in [
            Stability::Consistent,
            Stability::Moderate,
            Stability::Volatile,
            Stability::Insufficient,
        ] {
            assert_ne!(
                stability_color(&l, s),
                stability_color(&d, s),
                "稳定性色 {s:?} 未随主题变化"
            );
        }

        for rating in [1350.0_f64, 1500.0, 1700.0, 2100.0] {
            assert_ne!(
                rating_color(&l, rating),
                rating_color(&d, rating),
                "Rating {rating} 的分档色未随主题变化"
            );
        }

        assert_ne!(delta_color(&l, 50.0), delta_color(&d, 50.0));
        assert_ne!(
            percentile_color(&l, Some(5.0)),
            percentile_color(&d, Some(5.0))
        );
    }

    /// 胶囊背景色必须随主题变化，否则深色主题下会变成一块浅色斑点。
    #[test]
    fn chip_backgrounds_follow_the_theme() {
        let l = Palette::light();
        let d = Palette::dark();

        assert_ne!(
            ChipColors::positive(&l).bg,
            ChipColors::positive(&d).bg,
            "正向胶囊底未随主题变化"
        );
        assert_ne!(
            ChipColors::accent(&l).bg,
            ChipColors::accent(&d).bg,
            "强调胶囊底未随主题变化"
        );
        assert_ne!(
            ChipColors::neutral(&l).bg,
            ChipColors::neutral(&d).bg,
            "中性胶囊底未随主题变化"
        );
    }

    /// 胶囊必须真的半透明。
    ///
    /// 若 `ChipColors::of` 误用不透明构造，胶囊会变成实心色块，
    /// 盖住卡片底并让文字失去层次。
    #[test]
    fn chip_background_stays_translucent() {
        for p in palettes() {
            let c = ChipColors::positive(&p);
            assert!(c.bg.a() < 255, "胶囊底必须半透明，实得 alpha={}", c.bg.a());
            assert_eq!(c.fg.a(), 255, "胶囊文字必须完全不透明");
        }
    }

    /// 胶囊前景与背景必须可区分（前景是实色，背景是淡化版）。
    #[test]
    fn chip_foreground_differs_from_its_background() {
        for p in palettes() {
            for colors in [
                ChipColors::positive(&p),
                ChipColors::accent(&p),
                ChipColors::neutral(&p),
            ] {
                assert_ne!(
                    colors.fg, colors.bg,
                    "胶囊前景与背景不得相同，否则文字不可见"
                );
            }
        }
    }

    /// Rating 分档阈值必须与改造前的口径一致（1900 / 1600 / 1400）。
    ///
    /// 阈值是用户认知的一部分，静默改动会让历史数据的颜色突然变化。
    #[test]
    fn rating_tier_boundaries_are_preserved() {
        let p = Palette::dark();

        assert_eq!(rating_color(&p, 1900.0), rating_color(&p, 2200.0));
        assert_eq!(rating_color(&p, 1899.0), rating_color(&p, 1600.0));
        assert_eq!(rating_color(&p, 1599.0), rating_color(&p, 1400.0));
        assert_eq!(rating_color(&p, 1399.0), rating_color(&p, 1200.0));

        // 四档必须两两不同。
        let tiers = [
            rating_color(&p, 2200.0),
            rating_color(&p, 1700.0),
            rating_color(&p, 1500.0),
            rating_color(&p, 1200.0),
        ];
        for i in 0..tiers.len() {
            for j in (i + 1)..tiers.len() {
                assert_ne!(tiers[i], tiers[j], "Rating 第 {i} 档与第 {j} 档撞色");
            }
        }
    }

    /// 百分位阈值必须保留（10% / 30%），且优异档与普通档可区分。
    #[test]
    fn percentile_tiers_are_preserved_and_distinct() {
        let p = Palette::dark();

        assert_eq!(
            percentile_color(&p, Some(10.0)),
            percentile_color(&p, Some(3.0)),
            "10% 应归入优异档"
        );
        assert_eq!(
            percentile_color(&p, Some(30.0)),
            percentile_color(&p, Some(20.0)),
            "30% 应归入良好档"
        );
        assert_eq!(percentile_color(&p, None), percentile_color(&p, Some(80.0)));

        assert_ne!(
            percentile_color(&p, Some(3.0)),
            percentile_color(&p, Some(20.0)),
            "优异档与良好档必须可区分"
        );
        assert_ne!(
            percentile_color(&p, Some(20.0)),
            percentile_color(&p, Some(80.0)),
            "良好档与普通档必须可区分"
        );
    }

    /// 累计变化量的三态方向必须正确（正绿、负红、零中性）。
    #[test]
    fn delta_color_signs_are_not_swapped() {
        for p in palettes() {
            assert_eq!(delta_color(&p, 1.0), p.success, "正增长应为成功色");
            assert_eq!(delta_color(&p, -1.0), p.danger, "负增长应为危险色");
            assert_eq!(delta_color(&p, 0.0), p.text_tertiary, "零变化应为中性色");
        }
    }

    /// 趋势与稳定性必须映射到**同一个** `Trend`/`Stability` 概念色，
    /// 不能一处用 success 一处用 info——否则同一含义出现两种颜色。
    #[test]
    fn improving_and_consistent_share_the_positive_color() {
        for p in palettes() {
            assert_eq!(
                trend_color(&p, Trend::Improving),
                stability_color(&p, Stability::Consistent),
                "上升趋势与稳定表现都应使用同一正向色"
            );
            assert_eq!(
                trend_color(&p, Trend::Declining),
                stability_color(&p, Stability::Volatile),
                "下滑趋势与剧烈波动都应使用同一警示色"
            );
        }
    }
}
