//! AI 助理页：对话式获取学习建议。
//!
//! 页面结构：
//! 1. 顶部状态区（模型是否可用 + 数据可用性）
//! 2. 账号数据快照（助理"看到"的内容，保证透明度）
//! 3. 对话区（消息气泡）
//! 4. 输入区 + 快捷提问
//!
//! ## 设计要点
//!
//! **透明性是本页面的核心原则**：用户必须能看见助理此刻掌握哪些数据。
//! 如果助理说"你薄弱在动态规划"，但账号数据其实没拉取，那就是幻觉。
//! 因此页面明确列出「助理已知信息」清单，并标出缺失项。
//!
//! 主题化：本文件**不含任何 `Color32::from_*` 字面量**，全部取色经
//! `&Palette`。这一约束由 `ui::guards` 的源码扫描测试强制。
//!
//! ## 气泡配色的关键约束
//!
//! 三种角色（用户 / 助理 / 出错）的气泡必须有**独立的三套配色令牌**，
//! 而不是"一套浅色 + 调明度"。改造前三种气泡分别是浅蓝、浅红、浅灰的
//! 硬编码值，在深色主题下全是刺眼的亮块。现在由 `Palette` 按主题给出
//! 三组不同的底色/描边/文字，并有一项守卫测试断言三者在同一主题下
//! 互不相同、且在两套主题下各自变化。

use egui::{RichText, Ui};
use egui_commonmark::CommonMarkCache;

use crate::app::CompassApp;
use crate::logic::assistant::quick_prompts;
use crate::models::{ChatMessage, ChatRole};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets;

/// 对话区最小高度。
///
/// 太矮会让长回复只能显示一两行，体验极差。
const CHAT_AREA_MIN_HEIGHT: f32 = 220.0;

/// 气泡的三层配色（底 / 描边 / 文字）。
///
/// 单独抽出来是为了让「三态互不相同」这条约束可被测试断言，
/// 而不是散落在绘制函数里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BubbleColors {
    bg: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
}

/// 按消息角色解析气泡配色。
///
/// 角色优先于错误标志：用户消息不可能"出错"（错误只可能来自服务端）。
fn bubble_colors(p: &Palette, msg: &ChatMessage) -> BubbleColors {
    if msg.role == ChatRole::User {
        BubbleColors {
            bg: p.bubble_user_bg,
            border: p.bubble_user_border,
            text: p.text_primary,
        }
    } else if msg.is_error {
        BubbleColors {
            bg: p.bubble_error_bg,
            border: p.bubble_error_border,
            text: p.bubble_error_text,
        }
    } else {
        BubbleColors {
            bg: p.bubble_assistant_bg,
            border: p.bubble_assistant_border,
            text: p.text_primary,
        }
    }
}

/// 绘制助理页。
pub fn draw(ui: &mut Ui, app: &mut CompassApp) {
    let p = app.palette();

    // 顶部：模型状态。
    draw_header(ui, &p, app);
    ui.add_space(theme::space::SM);

    let llm_ready = app.llm.is_configured();

    if !llm_ready {
        widgets::info_banner(
            ui,
            &p,
            "尚未配置大模型 API Key，助理无法回复。\
             请到「设置」页填写 API Key（支持 OpenAI 兼容接口与 Anthropic）。\
             未配置 API Key 不影响本工具的其他功能。",
        );
        ui.add_space(theme::space::SM);
    }

    // 数据可用性：让用户知道助理掌握了什么。
    draw_context_summary(ui, &p, app);
    ui.add_space(theme::space::SM);

    // 对话区 + 输入区。
    draw_conversation(ui, &p, app, llm_ready);
}

/// 顶部标题与操作。
fn draw_header(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::glass_card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::SM;

            ui.label(
                RichText::new("学习助理")
                    .size(theme::text::SUBHEADING)
                    .strong()
                    .color(p.text_primary),
            );

            let llm_ready = app.llm.is_configured();

            if llm_ready {
                let info = app
                    .config
                    .try_read()
                    .map(|c| (c.llm.model.clone(), c.llm.provider.label_zh()))
                    .unwrap_or_default();

                ui.label(
                    RichText::new("● 已接入")
                        .size(theme::text::CAPTION)
                        .color(p.success),
                );
                if !info.0.is_empty() {
                    ui.label(
                        RichText::new(format!("{} / {}", info.1, info.0))
                            .size(theme::text::CAPTION)
                            .color(p.text_tertiary),
                    );
                }
            } else {
                ui.label(
                    RichText::new("○ 未接入")
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }

            if !app.assistant.messages.is_empty() {
                ui.separator();
                if ui
                    .button("清空对话")
                    .on_hover_text("清除当前会话的全部消息（不影响账号与配置）")
                    .clicked()
                {
                    app.assistant.messages.clear();
                }
            }

            if ui
                .button("刷新账号数据")
                .on_hover_text("重新拉取账号数据，让助理掌握最新情况")
                .clicked()
            {
                app.refresh_account();
            }
        });
    });
}

/// 数据可用性清单。
///
/// 这是本页面最有价值的设计之一：显式告诉用户助理掌握/缺失哪些数据，
/// 从而让用户能判断一条回答的可信度。
fn draw_context_summary(ui: &mut Ui, p: &Palette, app: &CompassApp) {
    let has_profile = app.profile.is_some();
    let solved_total = app.profile.as_ref().map(|profile| profile.solved.total()).unwrap_or(0);
    let tag_count = app.tag_stats.len();
    let has_recs = app
        .recommend
        .result
        .as_ref()
        .map(|r| !r.items.is_empty())
        .unwrap_or(false);
    let contest_count = app
        .contest
        .analysis
        .as_ref()
        .map(|a| a.attended_count)
        .unwrap_or(0);
    let submission_count = app.recent_submissions.len();
    let problem_count = app.problems.len();
    let has_account = app
        .config
        .try_read()
        .map(|c| c.has_account())
        .unwrap_or(false);

    egui::CollapsingHeader::new(
        RichText::new("助理已知信息（点击展开）")
            .size(theme::text::BODY)
            .strong()
            .color(p.text_secondary),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(theme::space::XS);
        ui.label(
            RichText::new(
                "助理的回答严格基于以下数据。标为「缺失」的项目它不会凭空推测——\
                 如果你发现它谈论了缺失项，那是幻觉，可以直接指出。",
            )
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );
        ui.add_space(theme::space::XS);

        let rows: Vec<(&str, bool, String)> = vec![
            (
                "LeetCode 账号绑定",
                has_account,
                if has_account { "已绑定".into() } else { "缺失".into() },
            ),
            (
                "解题统计（含难度分布）",
                has_profile,
                if has_profile {
                    format!("已通过 {solved_total} 道")
                } else {
                    "缺失".into()
                },
            ),
            (
                "知识点掌握分布",
                tag_count > 0,
                if tag_count > 0 {
                    format!("{tag_count} 个标签")
                } else {
                    "缺失".into()
                },
            ),
            (
                "本地题库",
                problem_count > 0,
                if problem_count > 0 {
                    format!("{problem_count} 道")
                } else {
                    "缺失".into()
                },
            ),
            (
                "推荐结果",
                has_recs,
                if has_recs { "已生成".into() } else { "缺失".into() },
            ),
            (
                "竞赛记录",
                contest_count > 0,
                if contest_count > 0 {
                    format!("{contest_count} 场")
                } else {
                    "缺失".into()
                },
            ),
            (
                "近期提交记录",
                submission_count > 0,
                if submission_count > 0 {
                    format!("{submission_count} 条")
                } else {
                    "缺失（需授权）".into()
                },
            ),
        ];

        for (label, ok, detail) in rows {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if ok { "✓" } else { "○" })
                        .size(theme::text::CAPTION)
                        .color(if ok { p.success } else { p.text_tertiary }),
                );
                ui.label(
                    RichText::new(label)
                        .size(theme::text::CAPTION)
                        .color(p.text_secondary),
                );
                ui.label(
                    RichText::new(format!("— {detail}"))
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            });
        }
    });
}

/// 对话区 + 输入区。
fn draw_conversation(ui: &mut Ui, p: &Palette, app: &mut CompassApp, llm_ready: bool) {
    let available = ui.available_height();
    let chat_height = (available - 140.0).max(CHAT_AREA_MIN_HEIGHT);

    // 对话消息区。
    egui::ScrollArea::vertical()
        .max_height(chat_height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if app.assistant.messages.is_empty() {
                draw_welcome(ui, p, app, llm_ready);
            } else {
                for msg in &app.assistant.messages {
                    // 字段级拆分借用：messages 只读、markdown_cache 可变，
                    // 二者是 `AssistantState` 的不同字段，借检允许。
                    draw_message(ui, p, &mut app.assistant.markdown_cache, msg);
                    ui.add_space(theme::space::SM);
                }
            }

            if app.assistant.thinking {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("助理正在思考…")
                            .size(theme::text::BODY_SMALL)
                            .color(p.text_tertiary),
                    );
                });
            }
        });



    ui.add_space(theme::space::SM);
    ui.separator();
    ui.add_space(theme::space::XS);

    // 历史工具行：左侧说明历史已持久化，右侧"清空对话"。
    if !app.assistant.messages.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("对话历史已自动保存，重启后保留最近 20 条")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("清空对话")
                    .on_hover_text("删除全部对话历史（数据库 + 当前界面），不可恢复")
                    .clicked()
                {
                    app.clear_assistant_history();
                }
            });
        });
        ui.add_space(theme::space::XS);
    }

    // 快捷提问。
    draw_quick_prompts(ui, p, app, llm_ready);

    ui.add_space(theme::space::XS);

    // 输入区。
    draw_input(ui, p, app, llm_ready);
}

/// 空对话时的欢迎与引导。
fn draw_welcome(ui: &mut Ui, p: &Palette, app: &CompassApp, llm_ready: bool) {
    ui.add_space(theme::space::XL);
    ui.vertical_centered(|ui| {
        // 用自绘图标而非 emoji：SimHei 单字重字体不含 emoji 字形。
        let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
        theme::icon(
            ui,
            rect.center(),
            30.0,
            theme::IconKind::Chat,
            p.text_tertiary,
            // 这一枚画在内容区，底色就是面板底色。
            p.bg_panel,
        );

        ui.add_space(theme::space::SM);
        ui.label(
            RichText::new("我是你的 LeetCode 学习助理")
                .size(theme::text::SUBHEADING)
                .color(p.text_secondary),
        );
        ui.add_space(theme::space::XS);
        ui.label(
            RichText::new(if llm_ready {
                "我会基于你的账号数据回答：该练什么、弱在哪里、竞赛表现如何。"
            } else {
                "配置大模型 API Key 后即可对话（见「设置」页）。"
            })
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );

        if let Some(profile) = &app.profile {
            ui.add_space(theme::space::MD);
            widgets::success_banner(
                ui,
                p,
                &format!(
                    "已读取账号数据：已通过 {} 道题（简单 {} / 中等 {} / 困难 {}）",
                    profile.solved.total(),
                    profile.solved.easy,
                    profile.solved.medium,
                    profile.solved.hard
                ),
            );
        }

        // 推荐与竞赛的摘要：让用户知道助理手上有哪些"论据"。
        if app.recommend.result.is_some() || app.contest.analysis.is_some() {
            ui.add_space(theme::space::MD);
            egui::Frame::NONE
                .fill(p.bg_sunken)
                .stroke(egui::Stroke::new(1.0, p.divider))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(theme::space::MD as i8))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("助理可引用的分析结果")
                                .size(theme::text::CAPTION)
                                .strong()
                                .color(p.text_secondary),
                        );
                        ui.add_space(theme::space::XS);
                        crate::ui::recommend::draw_compact_summary(ui, app);
                        ui.add_space(theme::space::XS);
                        crate::ui::contest::compact_summary(ui, app);
                    });
                });
        }
    });
    ui.add_space(theme::space::XL);
}

/// 单条消息气泡。
///
/// 气泡**不用玻璃底**：玻璃的半透明会让文字与下层内容混合，削弱可读性。
/// 这里用实心语义底（`bubble_*` 三组令牌），并保持描边以区分边界。
///
/// 内容按角色分流：助理回复按 **Markdown** 渲染（LLM 输出普遍带标题、
/// 列表与代码块）；用户输入与错误提示保持纯文本——用户敲的 `*`、`#`
/// 不应被解释为格式，错误信息也以原样展示更可靠。
fn draw_message(ui: &mut Ui, p: &Palette, cache: &mut CommonMarkCache, msg: &ChatMessage) {
    let is_user = msg.role == ChatRole::User;
    let colors = bubble_colors(p, msg);

    let speaker = if is_user {
        "我"
    } else if msg.is_error {
        "助理（出错了）"
    } else {
        "助理"
    };

    egui::Frame::NONE
        .fill(colors.bg)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(theme::space::MD as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.label(
                RichText::new(speaker)
                    .size(theme::text::CAPTION)
                    .strong()
                    .color(p.text_tertiary),
            );
            ui.add_space(theme::space::XS);

            // 消息内容：助理回复走 Markdown，其余纯文本。
            if is_user || msg.is_error {
                ui.label(
                    RichText::new(&msg.content)
                        .size(theme::text::MONO)
                        .color(colors.text),
                );
            } else {
                // **局部作用域覆盖 `active.fg_stroke`**：
                // egui 的 `RichText::strong()`（Markdown 的标题/加粗/列表
                // 圆点与序号都走它）取色来自
                // `visuals.strong_text_color()` == `widgets.active.fg_stroke`，
                // 而 `override_text_color` **不覆盖** strong/weak 路径
                // （egui 0.36 widget_text.rs `get_text_color`：strong 分支
                // 直接返回 active 态色，无视 override）。
                // 本项目把 active.fg_stroke 设为 `text_on_accent`
                // （深色主题 = 近黑、浅色主题 = 纯白），落在气泡底上
                // 对比度不足 1.2:1——这就是用户报告的"黑字看不清"。
                // 在气泡子树内把它改成气泡正文色，按钮按压态（全局）
                // 不受影响。
                ui.style_mut().visuals.widgets.active.fg_stroke =
                    egui::Stroke::new(1.0, colors.text);
                egui_commonmark::CommonMarkViewer::new().show(ui, cache, &msg.content);
            }

            // 助理消息提供复制按钮。
            if !is_user {
                ui.add_space(theme::space::XS);
                if ui
                    .small_button("复制")
                    .on_hover_text("复制全文到剪贴板")
                    .clicked()
                {
                    ui.ctx().copy_text(msg.content.clone());
                }
            }
        });
}

/// 快捷提问按钮。
fn draw_quick_prompts(ui: &mut Ui, p: &Palette, app: &mut CompassApp, llm_ready: bool) {
    let enabled = llm_ready && !app.assistant.thinking;
    let mut selected: Option<&'static str> = None;

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("快捷提问：")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.spacing_mut().item_spacing.x = theme::space::XS;

        for (label, prompt) in quick_prompts() {
            let btn = ui.add_enabled(
                enabled,
                egui::Button::new(RichText::new(label).size(theme::text::CAPTION)),
            );
            if btn.clicked() {
                selected = Some(prompt);
            }
        }
    });

    if let Some(prompt) = selected {
        app.assistant.input = prompt.to_string();
        app.send_assistant_message();
    }
}

/// 输入区。
fn draw_input(ui: &mut Ui, p: &Palette, app: &mut CompassApp, llm_ready: bool) {
    let enabled = llm_ready && !app.assistant.thinking;

    ui.horizontal(|ui| {
        // 输入框占据大部分宽度，右侧留给发送按钮。
        let button_width = 80.0;
        let input_width = (ui.available_width() - button_width).max(200.0);

        let hint = if enabled {
            "问点什么…（Enter 发送）"
        } else if llm_ready {
            "助理正在回复，请稍候…"
        } else {
            "请先在设置页配置大模型 API Key"
        };

        let resp = ui.add_sized(
            egui::vec2(input_width, 26.0),
            egui::TextEdit::singleline(&mut app.assistant.input).hint_text(hint),
        );

        // Enter 发送。
        let submitted_by_enter =
            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        let send_clicked = ui
            .add_enabled(
                enabled && !app.assistant.input.trim().is_empty(),
                egui::Button::new(RichText::new("发送").size(theme::text::BODY_SMALL)),
            )
            .clicked();

        if (submitted_by_enter || send_clicked) && enabled {
            app.send_assistant_message();
            // 让输入框重新获得焦点，方便连续追问。
            resp.request_focus();
        }
    });

    // 未接入模型时，给出直接的跳转入口。
    if !llm_ready {
        ui.add_space(theme::space::XS);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("提示：")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
            if ui.small_button("前往设置").clicked() {
                app.page = crate::app::Page::Settings;
            }
            ui.label(
                RichText::new("填写 API Key 后即可使用助理")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条测试消息。
    fn msg(role: ChatRole, is_error: bool) -> ChatMessage {
        ChatMessage {
            role,
            content: "内容".into(),
            is_error,
        }
    }

    #[test]
    fn chat_area_min_height_is_usable() {
        // 对话区太矮会让长回复只能显示一两行，体验极差。
        // 这里对派生值断言，避免 clippy 的 `assertions_on_constants` 误判为无效测试。
        let min_height = CHAT_AREA_MIN_HEIGHT;
        let required = 200.0_f32;
        assert!(
            min_height >= required,
            "对话区最小高度应足够显示一段完整回复"
        );
    }

    #[test]
    fn quick_prompts_are_available_and_distinct() {
        let prompts = quick_prompts();
        assert!(!prompts.is_empty(), "必须提供快捷提问，降低使用门槛");

        // 标签不重复，否则用户会看到两个一样的按钮。
        let mut labels: Vec<&str> = prompts.iter().map(|(l, _)| *l).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len(), "快捷提问标签不得重复");

        // 每个提示词都要有实质内容。
        for (label, prompt) in prompts {
            assert!(!label.trim().is_empty(), "标签不得为空");
            assert!(
                prompt.chars().count() >= 10,
                "提示词「{label}」过短，不足以让模型给出有用回答"
            );
        }
    }

    /// **本组测试对应一个真实的深色主题缺陷。**
    ///
    /// 改造前三种气泡分别是浅蓝、浅红、浅灰的硬编码值，在深色主题下
    /// 成了三块刺眼的亮色矩形。现在断言：三种气泡在**任何一套主题**下
    /// 都必须两两可区分。
    #[test]
    fn the_three_bubble_kinds_are_mutually_distinct_in_both_themes() {
        for p in [Palette::light(), Palette::dark()] {
            let user = bubble_colors(&p, &msg(ChatRole::User, false));
            let normal = bubble_colors(&p, &msg(ChatRole::Assistant, false));
            let error = bubble_colors(&p, &msg(ChatRole::Assistant, true));

            assert_ne!(user.bg, normal.bg, "用户气泡与助理气泡底色必须可区分");
            assert_ne!(normal.bg, error.bg, "助理气泡与错误气泡底色必须可区分");
            assert_ne!(user.bg, error.bg, "用户气泡与错误气泡底色必须可区分");
        }
    }

    /// 三种气泡都必须随主题变化（亮色主题下是亮底，深色主题下是暗底）。
    ///
    /// 这是"深色主题不可读"这一缺陷的直接回归测试：若某个气泡在两套
    /// 主题下取到同一底色，说明它是硬编码的，必然在其中一套主题下失败。
    #[test]
    fn every_bubble_kind_changes_between_themes() {
        let l = Palette::light();
        let d = Palette::dark();

        for (name, role, is_error) in [
            ("用户", ChatRole::User, false),
            ("助理", ChatRole::Assistant, false),
            ("错误", ChatRole::Assistant, true),
        ] {
            let lc = bubble_colors(&l, &msg(role, is_error));
            let dc = bubble_colors(&d, &msg(role, is_error));

            assert_ne!(lc.bg, dc.bg, "{name}气泡底色未随主题变化");
            assert_ne!(lc.border, dc.border, "{name}气泡描边未随主题变化");
            assert_ne!(lc.text, dc.text, "{name}气泡文字色未随主题变化");
        }
    }

    /// 深色主题下气泡底色必须**暗于**其中的文字——否则文字不可读。
    ///
    /// 用相对亮度比较而非具体色值，这样调整配色时测试仍然有效。
    #[test]
    fn bubble_text_is_lighter_than_the_bubble_in_dark_theme() {
        let p = Palette::dark();

        for (name, role, is_error) in [
            ("用户", ChatRole::User, false),
            ("助理", ChatRole::Assistant, false),
            ("错误", ChatRole::Assistant, true),
        ] {
            let c = bubble_colors(&p, &msg(role, is_error));
            let text_luma = luminance(c.text);
            let bg_luma = luminance(c.bg);
            assert!(
                text_luma > bg_luma + 40.0,
                "{name}气泡在深色主题下文字亮度 {text_luma:.0} 相对底色 {bg_luma:.0} 对比不足"
            );
        }
    }

    /// 浅色主题下则必须反过来：底亮字暗。
    #[test]
    fn bubble_text_is_darker_than_the_bubble_in_light_theme() {
        let p = Palette::light();

        for (name, role, is_error) in [
            ("用户", ChatRole::User, false),
            ("助理", ChatRole::Assistant, false),
            ("错误", ChatRole::Assistant, true),
        ] {
            let c = bubble_colors(&p, &msg(role, is_error));
            let text_luma = luminance(c.text);
            let bg_luma = luminance(c.bg);
            assert!(
                bg_luma > text_luma + 40.0,
                "{name}气泡在浅色主题下底色亮度 {bg_luma:.0} 相对文字 {text_luma:.0} 对比不足"
            );
        }
    }

    /// 角色优先于错误标志：用户消息永远走用户配色。
    ///
    /// 若反过来，一条被标记错误的用户消息会显示成红色错误气泡，
    /// 让人以为是用户自己出错。
    #[test]
    fn user_role_takes_precedence_over_the_error_flag() {
        let p = Palette::dark();
        let plain_user = bubble_colors(&p, &msg(ChatRole::User, false));
        let flagged_user = bubble_colors(&p, &msg(ChatRole::User, true));
        assert_eq!(plain_user, flagged_user, "用户消息不应受错误标志影响");
    }

    /// Markdown 渲染必须能在无头 Context 下完整跑通一段"LLM 典型输出"
    /// （标题 / 列表 / 加粗 / 行内代码 / 围栏代码块），且气泡内的
    /// Markdown 路径与纯文本路径都能渲染不 panic。
    #[test]
    fn assistant_markdown_renders_headless() {
        let ctx = egui::Context::default();
        let mut cache = CommonMarkCache::default();
        let markdown = "# 学习建议\n\n1. 先看**通过率**与难度\n2. 用 `双指针` 思路\n\n```rust\nfn two_sum() {}\n```\n";

        // egui 0.36 的无头驱动入口是 `run_ui`（闭包直接拿到根 `Ui`，
        // 与 eframe 的 `App::ui` 同一形态）。
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // 助理消息路径：Markdown。
            egui_commonmark::CommonMarkViewer::new().show(ui, &mut cache, markdown);
            // 用户消息路径：纯文本（含 Markdown 元字符也不解释）。
            ui.label(RichText::new("输入含 * 号与 # 号"));
        });
        assert!(
            !output.shapes.is_empty(),
            "Markdown 渲染没有产生任何绘制指令"
        );
        // 无头测试没有后端去消费纹理增量（首轮的字体纹理更新），
        // 按 epaint 的约定在丢弃前显式 `clear`。
        output.textures_delta.clear();
    }

    /// **Markdown 的强调色必须可读。**
    ///
    /// egui 的 `RichText::strong()`（Markdown 标题/加粗/列表符号）取色
    /// 来自 `widgets.active.fg_stroke`，且 `override_text_color` 不覆盖
    /// 该路径（egui 0.36 `widget_text.rs::get_text_color`：strong 分支
    /// 直接返回 active 态色）。本项目把 active 态设为 `text_on_accent`
    /// ——深色主题下是近黑（0x0B0E13）、浅色主题下是纯白，落在气泡底上
    /// 对比度不足 1.2:1，即用户报告的"黑字看不清"。
    ///
    /// 回归：渲染一条含标题/加粗/列表/代码块的助理消息，统计**不透明**
    /// 顶点颜色，禁止出现 `text_on_accent`（气泡内已在 `draw_message`
    /// 局部覆盖为气泡正文色）。alpha < 255 的顶点（阴影、8% 白色装饰）
    /// 与 `bg_sunken` 等合法背景色不参与判定。
    #[test]
    fn assistant_markdown_strong_text_never_uses_text_on_accent() {
        use crate::models::{ChatMessage, ChatRole};
        use crate::ui::theme::{ThemeMode, UiTheme};

        let markdown = "# 标题\n\n正文 **加粗** 与 *斜体* 与 `行内代码`\n\n- 列表一\n1. 列表二\n\n> 引用\n\n```rust\nfn two_sum() {}\n```\n\n[链接](https://example.com)\n";

        for (mode, forbidden) in [
            (ThemeMode::Dark, (0x0B, 0x0E, 0x13)), // 深色 text_on_accent
            (ThemeMode::Light, (0xFF, 0xFF, 0xFF)), // 浅色 text_on_accent
        ] {
            let ctx = egui::Context::default();
            let ui_theme = UiTheme::new(mode, true);
            crate::ui::theme::install(&ctx, &ui_theme);
            let p = ui_theme.palette;
            let mut cache = CommonMarkCache::default();
            let msg = ChatMessage {
                role: ChatRole::Assistant,
                content: markdown.into(),
                is_error: false,
            };

            // 有界视口：默认 RawInput 视口近无限大，markdown 查看器的
            // 屏幕外测量图元（x≈10000）会把统计搅浑。
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                draw_message(ui, &p, &mut cache, &msg);
            });
            // 无头测试没有后端消费纹理增量，先按 epaint 约定清理，
            // 避免断言失败时 unwind 触发第二重 panic（abort）。
            output.textures_delta.clear();

            let prims = ctx.tessellate(output.shapes, 1.0);
            let mut forbidden_hits = 0;
            for prim in &prims {
                let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else { continue };
                for vtx in &mesh.vertices {
                    let c = vtx.color;
                    if c.a() == 255
                        && (c.r(), c.g(), c.b()) == forbidden
                        && prim.clip_rect.contains(vtx.pos)
                    {
                        forbidden_hits += 1;
                    }
                }
            }
            assert!(
                forbidden_hits == 0,
                "{mode:?} 主题下 Markdown 渲染出现了 {forbidden_hits} 个 \
                 text_on_accent {forbidden:?} 顶点——标题/加粗/列表符号在气泡底上不可读\
                 （见 draw_message 的局部覆盖）"
            );
        }
    }

    /// 简单相对亮度（0.299R + 0.587G + 0.114B），够用于方向性断言。
    fn luminance(c: egui::Color32) -> f32 {
        0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
    }
}
