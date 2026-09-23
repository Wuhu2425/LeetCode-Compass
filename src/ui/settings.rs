//! 设置页：外观 + 账号绑定 + 大模型接入。
//!
//! 页面分为四个独立区块，各自可单独工作：
//!
//! 1. **外观**：浅色 / 深色主题切换 + 高级玻璃质感开关
//! 2. **LeetCode 账号**：用户名（必需，公开数据）与可选会话凭据
//!    （`LEETCODE_SESSION` + `csrftoken`，用于读取逐题完成状态）
//! 3. **大模型接入**：API Key 等，用于智能推荐解释与学习助理
//! 4. **本地数据**：缓存规模与渲染条数上限
//!
//! ## 设计要点
//!
//! **凭据安全**是本页面的首要关注点。会话 cookie 与 API Key 默认以
//! 掩码显示（`••••`），需要用户主动点击"显示"才明文呈现，避免旁人
//! 从屏幕上直接读走。凭据通过 `mask_secret()` 处理后仅在用户明示时
//! 展示完整值。
//!
//! 另外，**每个字段都附带获取指引**——用户不知道去哪儿找 cookie 是
//! 最常见的卡点，写清楚比让用户去搜索更有价值。
//!
//! 主题化：本文件**不含任何 `Color32::from_*` 字面量**，全部取色经
//! `&Palette`。这一约束由 `ui::guards` 的源码扫描测试强制。

use egui::{RichText, Ui};

use crate::app::{CompassApp, ToastKind};
use crate::config::LlmProvider;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets;

/// 绘制设置页。
pub fn draw(ui: &mut Ui, app: &mut CompassApp) {
    let p = app.palette();

    // 首次进入时从配置初始化草稿。
    //
    // 用 `try_read` 避免在 UI 线程阻塞；若正忙，说明有后台任务在写配置，
    // 下一帧再试即可，不会造成可见问题。
    //
    // 借用说明：先克隆出配置快照，让读锁 guard 立即释放，再修改
    // `app.settings`。否则 guard 会与 `&mut app` 冲突（E0502）。
    if !app.settings.is_initialized() {
        let snapshot = app.config.try_read().ok().map(|c| c.clone());
        if let Some(cfg) = snapshot {
            app.settings.sync_from(&cfg);
        }
    }

    ui.add_space(theme::space::SM);

    // ---- 布局结构：底部操作栏固定，内容区滚动 ----
    //
    // 实现：给 ScrollArea 的视口高度设上限（可用高度 − 操作栏预留高度），
    // 滚动区之后紧接着画操作栏。这样操作栏永远落在页面底部的固定条带里，
    // 不随长表单滚动——用户填完最后一项凭据后不必再滚回去找按钮。
    //
    // **刻意不用 `Panel::bottom` + `CentralPanel` 嵌套**：本页是在外壳的
    // CentralPanel 内绘制的，实测（egui 0.36.2，见 panel.rs 的
    // `show_inside_dyn`）同层再开面板会通过 `advance_cursor_after_rect`
    // 互相吞掉 available_rect——内层 CentralPanel 拿到近乎零高度，
    // ScrollArea 内容被整体裁掉（真机截图：卡片全部消失）。
    // 显式预留高度的方案不依赖面板状态，行为可预期。
    const ACTION_BAR_HEIGHT: f32 = 88.0; // 分隔线 + 按钮 + 提示行 + 间距
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - ACTION_BAR_HEIGHT).max(100.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new("设置")
                    .size(theme::text::HEADING)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(theme::space::XS);

            // 当前账号状态前置显示：用户在改配置前需要知道"现在是什么状态"。
            {
                let (username, authenticated) = app
                    .config
                    .try_read()
                    .map(|c| (c.username.clone(), c.is_authenticated()))
                    .unwrap_or_default();
                widgets::account_status_bar(ui, &p, &username, authenticated);
            }

            ui.add_space(theme::space::SM);

            draw_appearance_section(ui, &p, app);
            ui.add_space(theme::space::MD);

            draw_account_section(ui, &p, app);
            ui.add_space(theme::space::MD);

            draw_llm_section(ui, &p, app);
            ui.add_space(theme::space::MD);

            draw_data_section(ui, &p, app);
            ui.add_space(theme::space::XL);
        });

    // ---- 底部固定操作栏 ----
    ui.separator();
    ui.add_space(theme::space::XS);
    draw_actions(ui, &p, app);
}

/// 外观区块：主题与玻璃质感。
///
/// 这两项是本页唯一"改了立刻看得见"的设置，因此放在最前——用户点开设置
/// 若先看到一堆需要查文档才能填的凭据字段，会认为这个页很麻烦。
///
/// 二者都**即时生效**（不需要点「保存设置」）：主题切换是纯视觉状态，
/// 要求用户先保存再预览是不合理的。落盘发生在点「保存设置」时。
fn draw_appearance_section(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("外观")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new("改动即时生效。若希望下次启动保持，请点击底部「保存设置」。")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.add_space(theme::space::MD);

        // ---- 主题 ----
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("主题")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add_space(theme::space::SM);

            // 与顶栏同一枚按钮：图标语义为"点击后会变成什么"。
            let mode = app.ui_theme.mode;
            let icon = mode.toggle_icon();
            if widgets::glass_icon_button(ui, p, 32.0, icon, mode.toggle_hint()).clicked() {
                app.toggle_theme();
            }

            ui.add_space(theme::space::SM);
            ui.label(
                RichText::new(mode.label_zh())
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_primary),
            );
            ui.label(
                RichText::new(mode.toggle_hint())
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        });

        ui.add_space(theme::space::MD);

        // ---- 玻璃质感 ----
        //
        // 借用说明：`checkbox` 需要 `&mut bool`，而当前值来自 `app.ui_theme`。
        // 先复制到局部变量，再把"是否变化"带回处理后调用 `set_glass_effect`，
        // 避免在闭包内同时借用 `app` 的两部分。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("高级玻璃质感")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add_space(theme::space::SM);

            let mut glass = app.ui_theme.glass;
            let changed = ui
                .checkbox(&mut glass, RichText::new("启用").size(theme::text::BODY_SMALL))
                .on_hover_text(
                    "侧边栏、顶栏、按钮与卡片采用半透明 + 高光 + 投影的玻璃材质。\n\
                     关闭后改用不透明底色——在低分辨率屏幕或强烈逆光环境下\n\
                     可获得更高的文字对比度。",
                )
                .changed();

            if changed {
                app.set_glass_effect(glass);
            }
        });
    });
}

/// LeetCode 账号区块。
fn draw_account_section(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("LeetCode 账号")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new(
                "填写用户名即可浏览题库与查看公开数据。\
                 若需读取每道题的完成状态、提交记录与日历，还需提供会话凭据。",
            )
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        // 用户名。
        //
        // 提示文案必须**分站点**：中国站的查询标识是 ASCII slug，与显示昵称
        // 可能是两回事。统一写"个人主页 URL 中 /u/ 之后的字符串"会诱导用户
        // 去填中文昵称，结果查不到又不知道为什么。
        let site = crate::config::LeetCodeSite::all()
            .get(app.settings.site_idx)
            .copied()
            .unwrap_or_default();
        let (hint, hover) = if site.prefers_chinese_titles() {
            (
                "账号标识，例如：wuhu",
                "力扣中国站按「账号标识（slug）」查询，不是显示昵称。\n\
                 标识就是你个人主页 URL 中 /u/ 之后的那一段：\n\
                 https://leetcode.cn/u/wuhu/ → 填 wuhu\n\n\
                 若你的昵称是中文（比如「梧糊」），标识通常仍是注册时的\n\
                 英文或拼音名。填了会话凭据后点击「校验凭据」，\n\
                 应用会自动识别并回填正确的标识。",
            )
        } else {
            (
                "例如：leetcode_user",
                "LeetCode 个人主页 URL 中 /u/ 之后的字符串：\n\
                 https://leetcode.com/u/leetcode_user/ → 填 leetcode_user",
            )
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("用户名")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add_sized(
                egui::vec2(220.0, 24.0),
                egui::TextEdit::singleline(&mut app.settings.username).hint_text(hint),
            )
            .on_hover_text(hover);
        });

        ui.add_space(theme::space::SM);

        // 会话凭据说明。
        egui::CollapsingHeader::new(
            RichText::new("会话凭据（可选，用于读取完成状态）").size(theme::text::BODY_SMALL),
        )
        .default_open(app.settings.site_idx == 1)
        .show(ui, |ui| {
            // ---- 站点选择 ----
            //
            // 这是"Cookie 总无效"的首要成因：两站账号体系独立，
            // 用中国站的 Cookie 请求国际站，服务端只会当作未登录。
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("站点")
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_secondary),
                );
                let current = crate::config::LeetCodeSite::all()
                    .get(app.settings.site_idx)
                    .copied()
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("site_picker")
                    .selected_text(current.label_zh())
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        for (i, site) in crate::config::LeetCodeSite::all().iter().enumerate() {
                            ui.selectable_value(&mut app.settings.site_idx, i, site.label_zh());
                        }
                    });
            });
            ui.label(
                RichText::new("凭据来自哪个站就选哪个。选错会表现为「Cookie 总是无效」。")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );

            // 站点能力差异说明。
            //
            // 这是关键的预期管理：两站不只有域名不同，GraphQL schema 也
            // 完全不同，导致部分功能在中国站无法实现。提前讲清楚，
            // 用户才不会把"功能不可用"误判为"程序有 bug"。
            let current_site = crate::config::LeetCodeSite::all()
                .get(app.settings.site_idx)
                .copied()
                .unwrap_or_default();
            ui.label(
                RichText::new(current_site.capability_note())
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
            ui.add_space(theme::space::SM);

            widgets::info_banner(
                ui,
                p,
                "获取方式：在浏览器登录 LeetCode 后打开开发者工具（F12）→\
                 Network → 筛选 Fetch/XHR → 刷新页面 → 点任一 graphql 请求 →\
                 Request Headers → 复制 Cookie 整行的值，粘贴到下方「整段 Cookie」框，\
                 点「自动提取」即可。凭据仅保存在本机数据库，不会上传到任何第三方。",
            );
            ui.add_space(theme::space::SM);

            // ---- 整段 Cookie 智能提取 ----
            //
            // 用户从开发者工具复制的天然形态是整段 Cookie 字符串，
            // 要求手工抠出两个值既繁琐又易错。这里提供一个粘贴框 + 提取按钮。
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("整段 Cookie")
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_secondary),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.cookie_blob)
                        .desired_width(320.0)
                        .hint_text("LEETCODE_SESSION=...; csrftoken=..."),
                );
                let has_content = !app.settings.cookie_blob.trim().is_empty();
                if ui
                    .add_enabled(has_content, egui::Button::new("自动提取"))
                    .clicked()
                {
                    let (s, c) = crate::config::parse_cookie_blob(&app.settings.cookie_blob);
                    let mut got = Vec::new();
                    if let Some(s) = s {
                        app.settings.session_cookie = s;
                        got.push("LEETCODE_SESSION");
                    }
                    if let Some(c) = c {
                        app.settings.csrf_token = c;
                        got.push("csrftoken");
                    }
                    if got.is_empty() {
                        app.show_toast(
                            ToastKind::Error,
                            "未能从这段内容中识别出凭据，请确认复制的是 Cookie 整行",
                        );
                    } else {
                        app.settings.cookie_blob.clear();
                        app.show_toast(
                            ToastKind::Success,
                            format!("已提取：{}", got.join("、")),
                        );
                    }
                }
                // 提示识别结果，避免用户不确定是否成功。
                if !has_content {
                    ui.label(
                        RichText::new("粘贴后可自动分离出两个值")
                            .size(theme::text::CAPTION)
                            .color(p.text_tertiary),
                    );
                }
            });

            ui.add_space(theme::space::XS);

            // LEETCODE_SESSION。
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("LEETCODE_SESSION")
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_secondary),
                );
                secret_input(
                    ui,
                    p,
                    &mut app.settings.session_cookie,
                    &mut app.settings.reveal_session,
                    "粘贴 LEETCODE_SESSION 的值",
                );
            });

            ui.add_space(theme::space::XS);

            // csrftoken。
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("csrftoken")
                        .size(theme::text::BODY_SMALL)
                        .color(p.text_secondary),
                );
                secret_input(
                    ui,
                    p,
                    &mut app.settings.csrf_token,
                    &mut app.settings.reveal_csrf,
                    "粘贴 csrftoken 的值（32 位）",
                );
            });

            ui.add_space(theme::space::SM);

            // 当前授权状态。
            let complete = !app.settings.session_cookie.trim().is_empty()
                && !app.settings.csrf_token.trim().is_empty();

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if complete {
                        "● 凭据已填写（保存后可校验有效性）"
                    } else {
                        "○ 未填写完整凭据，将仅使用公开数据"
                    })
                    .size(theme::text::CAPTION)
                    .color(if complete { p.success } else { p.text_tertiary }),
                );

                if complete && ui.small_button("校验凭据").clicked() {
                    // 先保存再校验——校验读的是已落盘的配置。
                    app.save_settings();
                    app.verify_session(crate::app::CheckTrigger::Manual);
                }
            });

            if let Some(identity) = &app.verified {
                ui.label(
                    RichText::new(format!("✓ 凭据有效，已登录：{}", identity.display_label()))
                        .size(theme::text::CAPTION)
                        .color(p.success),
                );
                // 说明该结论会被记住，用户不必每次打开都重校验。
                ui.label(
                    RichText::new("该结论已保存，下次启动无需重新校验。")
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }
        });
    });
}

/// 大模型接入区块。
fn draw_llm_section(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("大模型接入（可选）")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new(
                "用于「智能推荐解释」与「学习助理」。不配置时，\
                 本地推荐算法与竞赛分析仍可正常使用。",
            )
            .size(theme::text::CAPTION)
            .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        // 服务商。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("服务商")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            let providers = LlmProvider::all();
            let current = providers
                .get(app.settings.provider_idx)
                .copied()
                .unwrap_or(LlmProvider::OpenAiCompatible);

            egui::ComboBox::from_id_salt("llm_provider")
                .selected_text(current.label_zh())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for (i, provider) in providers.iter().enumerate() {
                        if ui
                            .selectable_label(app.settings.provider_idx == i, provider.label_zh())
                            .clicked()
                        {
                            app.settings.provider_idx = i;
                            // 切换服务商时补全默认 Base URL 与示例模型名，
                            // 减少用户需要自己查文档的情况。
                            if app.settings.base_url.trim().is_empty()
                                || providers
                                    .iter()
                                    .any(|q| q.default_base_url() == app.settings.base_url.trim())
                            {
                                app.settings.base_url = provider.default_base_url().to_string();
                            }
                        }
                    }
                });
        });

        ui.add_space(theme::space::XS);

        // Base URL。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Base URL")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add_sized(
                egui::vec2(420.0, 24.0),
                egui::TextEdit::singleline(&mut app.settings.base_url)
                    .hint_text("https://api.openai.com/v1"),
            )
            .on_hover_text(
                "不含 /chat/completions。填本地地址（如 http://localhost:11434/v1）\
                 可使用 Ollama 等本地模型。",
            );
            if ui.small_button("用默认值").clicked() {
                let provider = LlmProvider::all()
                    .get(app.settings.provider_idx)
                    .copied()
                    .unwrap_or(LlmProvider::OpenAiCompatible);
                app.settings.base_url = provider.default_base_url().to_string();
            }
        });

        ui.add_space(theme::space::XS);

        // 模型名。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("模型")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add_sized(
                egui::vec2(220.0, 24.0),
                egui::TextEdit::singleline(&mut app.settings.model).hint_text("gpt-4o-mini"),
            );
        });

        ui.add_space(theme::space::XS);

        // API Key。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("API Key")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            secret_input(
                ui,
                p,
                &mut app.settings.api_key,
                &mut app.settings.reveal_api_key,
                "sk-…",
            );
        });

        ui.add_space(theme::space::XS);

        // 温度。
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("温度")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            ui.add(
                egui::Slider::new(&mut app.settings.temperature, 0.0..=1.5)
                    .fixed_decimals(1)
                    .text(""),
            )
            .on_hover_text("越低越稳定保守，越高越发散。学习建议场景建议 0.2～0.5。");
        });

        ui.add_space(theme::space::SM);

        // 连通性测试。
        ui.horizontal(|ui| {
            let configured =
                !app.settings.api_key.trim().is_empty() && !app.settings.base_url.trim().is_empty();

            if ui
                .add_enabled(configured, egui::Button::new("保存并测试连通性"))
                .on_hover_text("先保存配置，再向服务端发送一条最小请求验证可用性")
                .clicked()
            {
                app.save_settings();
                app.test_llm();
            }

            if !configured {
                ui.label(
                    RichText::new("需先填写 Base URL 与 API Key")
                        .size(theme::text::CAPTION)
                        .color(p.text_tertiary),
                );
            }

            let doc_url = match LlmProvider::all()
                .get(app.settings.provider_idx)
                .copied()
                .unwrap_or(LlmProvider::OpenAiCompatible)
            {
                LlmProvider::OpenAiCompatible => None,
                LlmProvider::Anthropic => Some("https://console.anthropic.com/settings/keys"),
            };
            if let Some(url) = doc_url {
                ui.hyperlink_to("获取 API Key", url);
            }
        });
    });
}

/// 本地数据区块。
fn draw_data_section(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    widgets::card(ui, p, |ui| {
        ui.label(
            RichText::new("本地数据")
                .size(theme::text::SUBHEADING)
                .strong()
                .color(p.text_primary),
        );
        ui.label(
            RichText::new("题库与账号数据缓存在本机 SQLite 数据库中，离线可用。")
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.add_space(theme::space::SM);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::space::XL;
            widgets::metric_label(ui, p, "已缓存题目", &format!("{}", app.problems.len()));
            widgets::metric_label(ui, p, "知识点标签", &format!("{}", app.tag_stats.len()));
            widgets::metric_label(ui, p, "竞赛记录", &format!("{}", app.contest_records.len()));
        });

        ui.add_space(theme::space::SM);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("每题显示条数")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_secondary),
            );
            let mut size = app.settings.page_size;
            if ui
                .add(egui::DragValue::new(&mut size).range(20..=500).speed(5.0))
                .changed()
            {
                app.settings.page_size = size;
            }
            ui.label(
                RichText::new("（影响题库页每次渲染的行数上限）")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        });
    });
}

/// 底部操作栏（固定在页面底部，不随内容滚动——布局见 `draw`）。
fn draw_actions(ui: &mut Ui, p: &Palette, app: &mut CompassApp) {
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new("保存设置").size(theme::text::BODY_SMALL))
            .on_hover_text("写入本机数据库")
            .clicked()
        {
            app.save_settings();
        }

        if ui
            .button("重新加载")
            .on_hover_text("放弃当前修改，从已保存的配置重新载入")
            .clicked()
        {
            // 先取出快照释放读锁，再修改 draft 并弹提示（避免 E0502）。
            let snapshot = app.config.try_read().ok().map(|c| c.clone());
            match snapshot {
                Some(cfg) => {
                    app.settings.sync_from(&cfg);
                    app.show_toast(ToastKind::Info, "已重新载入已保存的配置");
                }
                None => app.show_toast(ToastKind::Info, "配置正忙，请稍后重试"),
            }
        }

        ui.separator();

        // 清空输入区（常用于粘贴错误后想重来）。
        if ui
            .button("清空凭据")
            .on_hover_text("仅清空本页输入框，需再点「保存设置」才生效")
            .clicked()
        {
            app.settings.session_cookie.clear();
            app.settings.csrf_token.clear();
            app.settings.api_key.clear();
            app.show_toast(ToastKind::Info, "输入框已清空，点击「保存设置」后生效");
        }
    });

    ui.add_space(theme::space::XS);
    ui.label(
        RichText::new(
            "提示：配置保存在本机数据库中。凭据字段在界面上默认掩码显示，\
             但仍属明文存储——请勿在共享电脑上保存他人账号的凭据。",
        )
        .size(theme::text::CAPTION)
        .color(p.text_tertiary),
    );
}

/// 掩码输入框：默认隐藏内容，可点击眼睛按钮切换明文。
fn secret_input(
    ui: &mut Ui,
    p: &Palette,
    value: &mut String,
    reveal: &mut bool,
    hint: &str,
) {
    ui.add_sized(
        egui::vec2(360.0, 24.0),
        egui::TextEdit::singleline(value)
            .password(!*reveal)
            .hint_text(hint),
    );

    let label = if *reveal { "隐藏" } else { "显示" };
    if ui
        .small_button(label)
        .on_hover_text(if *reveal {
            "切换为掩码显示"
        } else {
            "切换为明文显示（注意旁人视线）"
        })
        .clicked()
    {
        *reveal = !*reveal;
    }

    // 附带掩码预览，让用户在掩码状态下也能确认已填入内容。
    if !*reveal && !value.trim().is_empty() {
        ui.label(
            RichText::new(crate::config::mask_secret(value))
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_default_urls_are_non_empty() {
        for provider in LlmProvider::all() {
            assert!(
                !provider.default_base_url().trim().is_empty(),
                "{} 必须有默认 Base URL，否则用户需要自己查文档",
                provider.label_zh()
            );
        }
    }

    #[test]
    fn mask_secret_hides_most_of_the_value() {
        // 掩码后必须无法从字符串还原原值。
        let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
        let masked = crate::config::mask_secret(secret);
        assert_ne!(masked, secret, "掩码结果不得与原值相同");
        assert!(
            !masked.contains("abcdefghij"),
            "掩码不得泄露中段内容，实际得到 {masked}"
        );
    }

    /// **本组测试对应需求二的一条隐性约束。**
    ///
    /// 设置页的开关与顶栏按钮必须**读同一份状态**，否则会出现
    /// "顶栏切了深色、设置页仍显示浅色"的自相矛盾。
    ///
    /// 这里断言 `UiTheme` 的两个维度由同一结构承载、且切换是幂等的：
    /// 连续切换两次必回到原状态。
    #[test]
    fn appearance_toggles_are_idempotent_and_share_one_state_source() {
        use crate::ui::theme::{ThemeMode, UiTheme};

        let base = UiTheme::new(ThemeMode::Light, true);

        // 主题：连切两次回到起点（顶栏与设置页点的是同一个方法）。
        let twice = base.toggled_mode().toggled_mode();
        assert_eq!(twice.mode, base.mode, "主题连切两次必须回到原状态");

        // 玻璃：显式设回原值。
        let restored = base.with_glass(false).with_glass(true);
        assert!(restored.glass, "玻璃开关设回 true 后必须为 true");
    }
}
