//! 推荐页：分析已完成题目并给出下一步练习建议。
//!
//! 页面结构：
//! 1. 顶部操作区（生成推荐 + 参数调整）
//! 2. 薄弱知识点诊断（解释"为什么推这些"）
//! 3. 推荐题目列表（含理由，可点击跳转）
//!
//! 设计要点：**推荐理由必须可见**。用户理解"为什么推这道题"之后才会
//! 真正去做；一个只有题目编号的列表对学习没有帮助。
//!
//! 主题化：本文件**不含任何 `Color32::from_*` 字面量**，全部取色经
//! `&Palette`。这一约束由 `ui::guards` 的源码扫描测试强制。

use egui::{RichText, Ui};

use crate::app::CompassApp;
use crate::logic::recommend::RecommendMode;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets;

/// 绘制推荐页。
pub fn draw(ui: &mut Ui, app: &mut CompassApp) {
    let p = app.palette();

    draw_header(ui, &p, app);
    ui.add_space(theme::space::MD);

    // 未绑定账号时，说明清楚会发生什么。
    let has_account = app
        .config
        .try_read()
        .map(|c| c.has_account())
        .unwrap_or(false);

    if !has_account {
        widgets::info_banner(
            ui,
            &p,
            "尚未绑定账号。你仍可以获得基于「难度递进」的通用推荐；\
             绑定账号并授权后，推荐会基于你的知识点分布精准定位薄弱环节。",
        );
        ui.add_space(theme::space::SM);
    }

    if app.problems.is_empty() {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "题库未加载，无法生成推荐",
            "请先到「题库」页同步题目数据",
        );
        return;
    }

    // 无推荐结果时给出引导。
    if app.recommend.result.is_none() {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "尚未生成推荐",
            "点击上方「生成推荐」按钮，系统将分析你的练习数据并给出建议",
        );
        ui.vertical_centered(|ui| {
            if ui.button("生成推荐").clicked() {
                app.generate_recommendations();
            }
        });
        return;
    }

    // 克隆结果以避免与 app 的可变借用冲突。
    // `RecommendationSet` 只含推荐条目（默认 12 条），克隆成本可忽略。
    let Some(set) = app.recommend.result.clone() else {
        return;
    };

    draw_mode_banner(ui, &p, set.mode);
    ui.add_space(theme::space::SM);

    if let Some(note) = &set.note {
        widgets::info_banner(ui, &p, note);
        ui.add_space(theme::space::SM);
    }

    // 薄弱知识点诊断。
    if !set.weak_tags.is_empty() {
        draw_weak_tags(ui, &p, &set.weak_tags);
        ui.add_space(theme::space::MD);
    }

    // 知识面覆盖度 + 完全空白领域。
    draw_coverage(ui, &p, app);
    ui.add_space(theme::space::MD);

    // 推荐列表。
    if set.items.is_empty() {
        widgets::empty_state(
            ui,
            &p,
            "◆",
            "暂无需要推荐的题目",
            "可能你的练习已经很全面了，或筛选条件过严。可调整参数后重新生成。",
        );
        return;
    }

    draw_recommendations(ui, &p, app, &set);
}

/// 顶部：标题 + 操作 + 参数。
///
/// 整体包进玻璃卡片，与题库页工具栏保持一致的视觉语汇。
fn draw_header(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::glass_card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::SM;
            ui.label(
                RichText::new("智能推荐")
                    .size(theme::text::SUBHEADING)
                    .strong()
                    .color(p.text_primary),
            );

            if ui
                .button("生成推荐")
                .on_hover_text("基于当前题库与账号数据重新计算")
                .clicked()
            {
                app.generate_recommendations();
            }

            // 生成时间。
            //
            // 推荐结果会持久化、重启后直接恢复，因此**必须**让用户看得见它是
            // 什么时候算的。否则用户会把几天前的推荐当成刚生成的最新结论——
            // 推荐的价值与新鲜度直接相关。
            if let Some(at) = app.recommend.generated_at {
                let now = chrono::Utc::now().timestamp();
                ui.label(
                    RichText::new(format!("生成于 {}", format_generated_at(at, now)))
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                )
                .on_hover_text(
                    "结果已缓存，重启后仍会显示。\n\
                     题库或账号数据更新后，点「生成推荐」即可重算。",
                );
            }

            ui.separator();

            // 推荐数量。
            ui.label(
                RichText::new("数量")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            let mut n = app.recommend.config.top_n;
            if ui
                .add(egui::DragValue::new(&mut n).range(3..=30).speed(1.0))
                .changed()
            {
                app.recommend.config.top_n = n;
            }

            // 会员题开关。
            ui.checkbox(
                &mut app.recommend.config.exclude_paid_only,
                RichText::new("排除会员题").size(theme::text::BODY_SMALL),
            );

            // 已完成题开关（仅在已授权时有意义）。
            let authenticated = app
                .config
                .try_read()
                .map(|c| c.is_authenticated())
                .unwrap_or(false);
            if authenticated {
                ui.checkbox(
                    &mut app.recommend.config.exclude_solved,
                    RichText::new("排除已通过").size(theme::text::BODY_SMALL),
                );
            }
        });
    });
}

/// 把推荐结果的生成时刻格式化为便于判断新鲜度的文本。
///
/// 分级策略：
/// - **近期用相对时间**（"5 分钟前"）——用户真正关心的是"这份结果新不新"；
/// - **超过一周改用绝对日期**——"9 天前"远不如具体日期有用，
///   而且天数继续增长后读者已无法换算。
///
/// 纯函数，`now` 由调用方传入，便于测试。
fn format_generated_at(generated_at: i64, now: i64) -> String {
    let elapsed = now - generated_at;

    // 时钟回拨（或系统时间被改）会产生负数。不显示"负 3 分钟前"这种
    // 荒谬文本——按"刚刚"处理是唯一不误导的选择。
    if elapsed < 60 {
        return "刚刚".to_string();
    }

    let minutes = elapsed / 60;
    if minutes < 60 {
        return format!("{minutes} 分钟前");
    }

    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} 小时前");
    }

    let days = hours / 24;
    if days < 7 {
        return format!("{days} 天前");
    }

    chrono::DateTime::from_timestamp(generated_at, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| generated_at.to_string())
}

/// 推荐模式说明条。
///
/// **不用玻璃底**：这条横幅的作用是声明"当前推荐的可信度等级"，
/// 半透明底会让它退到背景里，削弱这个声明的分量。
fn draw_mode_banner(ui: &mut Ui, p: &Palette, mode: RecommendMode) {
    let (text, colors) = match mode {
        RecommendMode::ProgressAware => (
            "进阶模式：已授权会话凭据，可准确识别已完成题目，推荐已排除你做过的题。",
            p.banner_success,
        ),
        RecommendMode::Basic => (
            "基础模式：未授权会话凭据，无法得知逐题完成状态。推荐基于你的知识点分布\
             定位薄弱环节（不会误判哪些题已完成）。",
            p.banner_info,
        ),
    };

    egui::Frame::NONE
        .fill(colors.bg)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(theme::space::MD as i8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("◆").size(theme::text::BODY_SMALL).color(colors.icon));
                ui.label(
                    RichText::new(mode.label_zh())
                        .size(theme::text::BODY_SMALL)
                        .strong()
                        .color(colors.icon),
                );
                ui.label(
                    RichText::new(text)
                        .size(theme::text::CAPTION)
                        .color(colors.text),
                );
            });
        });
}

/// 薄弱知识点诊断面板。
fn draw_weak_tags(ui: &mut Ui, p: &Palette, weak: &[crate::logic::recommend::WeakTag]) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("薄弱知识点诊断")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new("按掌握度升序排列，这些是你练习最少的领域")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        for t in weak.iter().take(8) {
            ui.horizontal(|ui| {
                // 标签名（固定宽度以对齐条形图）。
                ui.allocate_ui_with_layout(
                    egui::vec2(180.0, 18.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(&t.tag_name)
                                .size(theme::text::BODY_SMALL)
                                .color(p.text_primary),
                        );
                    },
                );

                // 掌握度条形图。颜色随掌握度变化：越低越警示。
                let mastery = t.mastery as f32;
                let bar_color = mastery_bar_color(p, mastery);

                let bar_width = 200.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(bar_width, 14.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        widgets::horizontal_bar(ui, p, mastery, bar_color, 10.0);
                    },
                );

                ui.label(
                    RichText::new(format!("{}/{} 题（{:.0}%）", t.solved, t.total, mastery * 100.0))
                        .size(theme::text::CAPTION)
                        .color(p.text_secondary),
                );
            });
        }

        if weak.len() > 8 {
            ui.add_space(theme::space::XS);
            ui.label(
                RichText::new(format!("另有 {} 个薄弱知识点未列出", weak.len() - 8))
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        }
    });
}

/// 掌握度 → 条形图颜色（三档）。
///
/// 阈值取自推荐引擎的同类判定（0.15 / 0.40），三档分别映射到危险、警示、
/// 良好三组语义色——这样在深色主题下也能保证对比度。
fn mastery_bar_color(p: &Palette, mastery: f32) -> egui::Color32 {
    if mastery < 0.15 {
        p.danger
    } else if mastery < 0.4 {
        p.warning
    } else {
        p.success
    }
}

/// 推荐题目列表。
fn draw_recommendations(
    ui: &mut Ui,
    p: &Palette,
    app: &mut CompassApp,
    set: &crate::logic::recommend::RecommendationSet,
) {
    ui.label(
        RichText::new(format!("推荐练习（{} 道）", set.items.len()))
            .size(theme::text::SUBHEADING)
            .strong()
            .color(p.text_primary),
    );
    ui.label(
        RichText::new("按推荐优先级排序，点击任意条目在浏览器中打开")
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
    );
    ui.add_space(theme::space::XS);

    let mut clicked: Option<String> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (rank, rec) in set.items.iter().enumerate() {
                let resp = draw_recommendation_card(ui, p, rank + 1, rec);
                if resp.clicked() {
                    clicked = Some(rec.problem.title_slug.clone());
                }
                ui.add_space(theme::space::SM);
            }
        });

    if let Some(slug) = clicked {
        if let Some(prob) = app.problems.iter().find(|prob| prob.title_slug == slug) {
            // 站点随登录会话：登的是国内站跳 leetcode.cn，登的是国际站跳 leetcode.com。
            if let Some(url) = prob.url(app.current_site()) {
                match open::that(&url) {
                    Ok(()) => app.show_toast(
                        crate::app::ToastKind::Info,
                        format!("已在浏览器打开：{}", prob.title),
                    ),
                    Err(e) => app.show_toast(
                        crate::app::ToastKind::Error,
                        format!("无法打开浏览器：{e}"),
                    ),
                }
            }
        }
    }
}

/// 知识面覆盖度与完全空白领域。
///
/// "薄弱"与"空白"是两个不同的概念：薄弱是练过但少，空白是**完全没碰过**。
/// 空白领域往往是最容易快速提分的地方，因此单独列出。
///
/// 数据依赖：本卡片需要"用户技能标签统计"。**中国站不提供该接口**
/// （无 `matchedUser.tagProblemCounts`），此时不是"没有数据"而是
/// "该站点不提供"——必须说清楚，否则用户会以为是自己练得太少。
fn draw_coverage(ui: &mut Ui, p: &Palette, app: &CompassApp) {
    if app.tag_stats.is_empty() {
        // 中国站：明确告知能力边界，并说明推荐算法已降级。
        if !app.current_site().supports_tag_stats() {
            widgets::card(ui, p, |ui| {
                ui.label(
                    RichText::new("知识面覆盖")
                        .size(theme::text::SUBHEADING)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(theme::space::XS);
                widgets::info_banner(
                    ui,
                    p,
                    "当前力扣中国站不提供「技能标签统计」接口，因此无法计算知识面\
                     覆盖度。推荐已自动降级为基于题库元数据（难度、通过率、标签分布），\
                     仍可正常使用。若需要按知识点精准推荐，请在设置页改用国际站账号。",
                );
            });
        }
        return;
    }

    let (covered, total_tags) = crate::logic::recommend::knowledge_coverage(&app.tag_stats);
    // 与推荐引擎使用同一阈值，避免两处口径不一致。
    let untouched = crate::logic::recommend::untouched_tags(&app.tag_stats, 8);

    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("知识面覆盖")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.add_space(theme::space::XS);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::XL;
            widgets::metric_label(ui, p, "已涉及知识点", &format!("{covered} / {total_tags}"));
            widgets::metric_label(ui, p, "完全空白", &format!("{}", untouched.len()));
        });

        if total_tags > 0 {
            ui.add_space(theme::space::XS);
            let ratio = covered as f32 / total_tags as f32;
            widgets::horizontal_bar(ui, p, ratio, p.accent, 8.0);
        }

        if !untouched.is_empty() {
            ui.add_space(theme::space::SM);
            ui.label(
                RichText::new("完全没做过的领域（按题量降序，这些通常是提分最快的地方）")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
            ui.add_space(theme::space::XS);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::space::XS;
                for t in untouched.iter().take(12) {
                    ui.label(
                        RichText::new(format!(" {} ({}) ", t.tag_name, t.total))
                            .size(theme::text::CAPTION)
                            .background_color(p.banner_warning.bg)
                            .color(p.banner_warning.text),
                    );
                }
            });

            if untouched.len() > 12 {
                ui.label(
                    RichText::new(format!("另有 {} 个未列出", untouched.len() - 12))
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }
        }
    });
}

/// 单条推荐卡片。
///
/// 用玻璃卡片：推荐列表是本页主体，多条并列时玻璃材质的层次感最明显。
fn draw_recommendation_card(
    ui: &mut Ui,
    p: &Palette,
    rank: usize,
    rec: &crate::models::Recommendation,
) -> egui::Response {
    let prob = &rec.problem;

    let resp = ui
        .scope(|ui| {
            widgets::glass_card(ui, p, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    // 排名徽章。
                    ui.label(
                        RichText::new(format!("#{rank}"))
                            .size(theme::text::BODY_SMALL)
                            .strong()
                            .color(p.text_tertiary),
                    );

                    ui.label(
                        RichText::new(&prob.frontend_id)
                            .size(theme::text::BODY_SMALL)
                            .color(p.text_tertiary),
                    );
                    ui.label(
                        RichText::new(&prob.title)
                            .size(theme::text::BODY)
                            .strong()
                            .color(p.text_primary),
                    );

                    widgets::difficulty_badge(ui, p, prob.difficulty);

                    ui.label(
                        RichText::new(format!("通过率 {:.1}%", prob.ac_rate))
                            .size(theme::text::CAPTION)
                            .color(p.text_secondary),
                    );

                    if prob.is_paid_only {
                        ui.label(
                            RichText::new("会员")
                                .size(theme::text::CAPTION)
                                .color(p.banner_warning.text),
                        );
                    }

                    // 尝试过的题给出特殊标记。
                    if prob.status == crate::models::SolveStatus::Attempted {
                        ui.label(
                            RichText::new("重做")
                                .size(theme::text::CAPTION)
                                .background_color(p.banner_warning.bg)
                                .color(p.banner_warning.text),
                        );
                    }
                });

                ui.add_space(theme::space::XS);

                // 标签行。
                ui.horizontal_wrapped(|ui| {
                    widgets::tag_pills(ui, p, &prob.tags, 5);
                });

                // 推荐理由。这是本页面的核心信息。
                ui.add_space(theme::space::XS);
                for reason in &rec.reasons {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("·")
                                .size(theme::text::CAPTION)
                                .color(p.accent),
                        );
                        ui.label(
                            RichText::new(reason)
                                .size(theme::text::CAPTION)
                                .color(p.text_secondary),
                        );
                    });
                }
            });
        })
        .response;

    // 整卡可点击。
    resp.interact(egui::Sense::click())
        .on_hover_text("点击在浏览器中打开此题目")
}

/// 供其他模块复用的推荐页面入口（用于"从助理跳转到推荐"等场景）。
pub fn draw_compact_summary(ui: &mut Ui, app: &CompassApp) {
    let p = app.palette();

    let Some(set) = &app.recommend.result else {
        ui.label(
            RichText::new("尚无推荐结果")
                .size(theme::text::BODY_SMALL)
                .color(p.text_tertiary),
        );
        return;
    };

    ui.label(
        RichText::new(format!("当前推荐 {} 道题", set.items.len()))
            .size(theme::text::BODY_SMALL)
            .color(p.text_primary),
    );
    for rec in set.items.iter().take(3) {
        ui.label(
            RichText::new(format!(
                "  · {}. {}",
                rec.problem.frontend_id, rec.problem.title
            ))
            .size(theme::text::CAPTION)
            .color(p.text_secondary),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 推荐结果原先只存于内存，重启即丢失，用户每次打开都要重新点一次
    /// 「生成推荐」。改为持久化后，界面必须标明生成时间——否则用户会把
    /// 几天前的推荐当成刚算出来的，而推荐的价值与新鲜度直接相关。
    #[test]
    fn generated_at_uses_relative_text_within_a_day() {
        let now = 1_700_000_000_i64;
        assert_eq!(format_generated_at(now, now), "刚刚");
        assert_eq!(format_generated_at(now - 30, now), "刚刚");
        assert_eq!(format_generated_at(now - 60, now), "1 分钟前");
        assert_eq!(format_generated_at(now - 45 * 60, now), "45 分钟前");
        assert_eq!(format_generated_at(now - 3600, now), "1 小时前");
        assert_eq!(format_generated_at(now - 23 * 3600, now), "23 小时前");
    }

    /// 一周内仍用相对时间，超过一周改用绝对日期。
    ///
    /// 边界值必须明确：`days < 7` 意味着第 7 天整点就切到日期，
    /// 否则会出现"365 天前"这类读者无法换算的文本。
    #[test]
    fn generated_at_switches_to_absolute_date_after_a_week() {
        let now = 1_700_000_000_i64;
        assert_eq!(format_generated_at(now - 24 * 3600, now), "1 天前");
        assert_eq!(format_generated_at(now - 6 * 24 * 3600, now), "6 天前");

        // 第 7 天起改为日期，格式为 YYYY-MM-DD。
        let text = format_generated_at(now - 7 * 24 * 3600, now);
        assert!(
            text.len() == 10 && text.chars().filter(|c| *c == '-').count() == 2,
            "超过一周应输出日期，实得: {text}"
        );
    }

    /// 时钟回拨（或系统时间被改）不得产出"负 N 分钟前"这种荒谬文本。
    ///
    /// 用 `now - generated_at` 直接计算时，未来时间戳会得到负数；
    /// 若不特判，`elapsed / 60` 会得到负数并原样拼进文案。
    #[test]
    fn generated_at_handles_clock_skew() {
        let now = 1_700_000_000_i64;
        assert_eq!(format_generated_at(now + 10_000, now), "刚刚");
        assert_eq!(format_generated_at(now + 86_400, now), "刚刚");
    }

    /// 掌握度三档配色必须随主题变化。
    ///
    /// 若某档在两套主题下取到同一色值，说明它没跟着主题走——那样的颜色
    /// 必然在其中一套主题下与背景撞色。
    #[test]
    fn mastery_colors_follow_the_theme() {
        let l = Palette::light();
        let d = Palette::dark();

        for mastery in [0.0_f32, 0.3, 0.9] {
            assert_ne!(
                mastery_bar_color(&l, mastery),
                mastery_bar_color(&d, mastery),
                "掌握度 {mastery} 的配色未随主题变化"
            );
        }
    }

    /// 三档掌握度必须映射到三个彼此不同的语义色。
    ///
    /// 这是本图的语义基础：条形图靠颜色区分"很弱 / 偏弱 / 尚可"，
    /// 两档撞色则信息丢失。
    #[test]
    fn mastery_tiers_are_mutually_distinct() {
        let p = Palette::dark();
        let low = mastery_bar_color(&p, 0.0);
        let mid = mastery_bar_color(&p, 0.3);
        let high = mastery_bar_color(&p, 0.9);

        assert_ne!(low, mid, "低档与中档必须可区分");
        assert_ne!(mid, high, "中档与高档必须可区分");
        assert_ne!(low, high, "低档与高档必须可区分");
    }

    /// 档位边界必须与推荐引擎的阈值一致（0.15 / 0.40）。
    ///
    /// 两处口径不一致会让"诊断面板说很弱、条形图显示尚可"这类矛盾出现。
    #[test]
    fn mastery_tier_boundaries_match_the_recommend_engine() {
        let p = Palette::dark();

        // 恰好 0.15 应进入中档，而非低档。
        assert_eq!(
            mastery_bar_color(&p, 0.15),
            mastery_bar_color(&p, 0.16),
            "0.15 应归属中档"
        );
        // 恰好 0.40 应进入高档。
        assert_eq!(
            mastery_bar_color(&p, 0.40),
            mastery_bar_color(&p, 0.50),
            "0.40 应归属高档"
        );
        // 0.1499 仍在低档。
        assert_eq!(
            mastery_bar_color(&p, 0.1499),
            mastery_bar_color(&p, 0.0),
            "0.1499 仍在低档"
        );
    }
}
