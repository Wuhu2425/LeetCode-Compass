//! 复用 UI 组件。
//!
//! egui 是即时模式 GUI，组件本质上是"绘制函数"。本模块集中那些在多处
//! 重复出现的视觉元素（难度徽章、标签胶囊、状态标记、玻璃容器），
//! 保证样式统一，避免各页面各写一套定义导致视觉不一致。
//!
//! ## 颜色约定（重要）
//!
//! 本模块是**唯一**允许出现 `Color32` 字面量的 UI 模块（另一个是
//! [`super::theme`]）。所有色值一律取自 [`Palette`] 的语义令牌。
//!
//! 页面层不得自行定义颜色——需要什么语义色，就在本模块加一个取色函数。
//! 这条约定由 `src/ui/guards.rs` 的源码扫描测试强制。
//!
//! ## 取色函数的签名约定
//!
//! 语义取色函数一律接收 `&Palette` 而非从 `ui` 取色。理由：`ui.visuals()`
//! 拿到的是 egui 框架的视觉参数，而本项目的语义色板是独立概念
//! （egui 不知道"难度=困难"该是什么颜色）。显式传色板让依赖关系清晰，
//! 也让这些函数可被单元测试直接调用。

use egui::{Color32, CornerRadius, Margin, RichText, Ui};

use crate::models::{Difficulty, SolveStatus, TopicTag};
use crate::ui::theme::{self, BannerColors, GlassSurface, Palette};

/// 难度对应的颜色。
///
/// 采用业界通行语义（绿 / 琥珀 / 红），方便用户形成直觉。
/// 色值来自色板的语义令牌，随主题自动变化。
pub fn difficulty_color(p: &Palette, d: Difficulty) -> Color32 {
    match d {
        Difficulty::Easy => p.success,
        Difficulty::Medium => p.warning,
        Difficulty::Hard => p.danger,
    }
}

/// 完成状态对应的颜色。
pub fn status_color(p: &Palette, s: SolveStatus) -> Color32 {
    match s {
        SolveStatus::Solved => p.success,
        SolveStatus::Attempted => p.warning,
        SolveStatus::Todo => p.neutral,
        SolveStatus::Unknown => p.text_tertiary,
    }
}

/// 难度徽章（可选中形式）。
///
/// `selected` 为真时使用实心强调底 + 反色文字，让"已勾选"一目了然。
///
/// **注意调用顺序**：本函数内部会读取 `selected`，因此调用方**不得**在
/// 传入 `&mut selected` 的同一表达式中再读它（E0503）。需要"先算颜色再
/// 传可变引用"的场景，请显式分两步写。
pub fn difficulty_pill(ui: &mut Ui, p: &Palette, d: Difficulty, selected: bool) -> egui::Response {
    let badge_color = difficulty_color(p, d);
    // 颜色在构造 RichText 前算好：避免与 `&mut selected` 的借用重叠。
    let (bg, fg) = if selected {
        (badge_color, p.text_on_accent)
    } else {
        (p.bg_sunken, p.text_secondary)
    };
    // 未选中态用 `control_stroke`：难度胶囊是可点击的**开关**，
    // 浅色主题下 `border_subtle`（8% 不透明度）叠在浅灰底上等于没有边框，
    // 用户看不出这是可以点的筛选条件。
    let stroke = if selected {
        badge_color
    } else {
        p.control_stroke
    };

    let text = RichText::new(format!(" {} ", d.label_zh()))
        .size(theme::text::CAPTION)
        .color(fg);

    let galley = ui.painter().layout_no_wrap(
        text.text().to_owned(),
        egui::FontId::proportional(theme::text::CAPTION),
        fg,
    );

    let padding = egui::vec2(10.0, 5.0);
    let desired = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    let radius = CornerRadius::same(6);
    ui.painter().rect_filled(rect, radius, bg);
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        fg,
    );

    response
}

/// 绘制难度徽章。
pub fn difficulty_badge(ui: &mut Ui, p: &Palette, d: Difficulty) {
    ui.label(
        RichText::new(d.label_zh())
            .color(difficulty_color(p, d))
            .strong()
            .size(theme::text::BODY_SMALL),
    );
}

/// 绘制完成状态徽章。
///
/// `Unknown` 状态显示为"—"而非"未开始"：在未授权时，"未知"与
/// "确认没做过"是两回事，混为一谈会让用户误判自己的进度。
pub fn status_badge(ui: &mut Ui, p: &Palette, s: SolveStatus) {
    let color = status_color(p, s);
    let text = match s {
        SolveStatus::Solved => "✓ 已通过",
        SolveStatus::Attempted => "◐ 尝试过",
        SolveStatus::Todo => "○ 未开始",
        SolveStatus::Unknown => "— 未知",
    };
    ui.label(RichText::new(text).color(color).size(theme::text::BODY_SMALL));
}

/// 绘制标签胶囊。
///
/// 为控制视觉噪音，最多显示 `max` 个，超出部分折叠为 `+N`。
pub fn tag_pills(ui: &mut Ui, p: &Palette, tags: &[TopicTag], max: usize) {
    if tags.is_empty() {
        ui.label(
            RichText::new("—")
                .color(p.text_tertiary)
                .size(theme::text::BODY_SMALL),
        );
        return;
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::XS;
        for t in tags.iter().take(max) {
            let text = if t.name.is_empty() { &t.slug } else { &t.name };
            ui.label(
                RichText::new(format!(" {} ", text))
                    .size(theme::text::CAPTION)
                    .background_color(p.bg_sunken)
                    .color(p.text_secondary),
            );
        }
        if tags.len() > max {
            ui.label(
                RichText::new(format!("+{}", tags.len() - max))
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        }
    });
}

/// 绘制带标签的数值指标。
pub fn metric_label(ui: &mut Ui, p: &Palette, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .size(theme::text::CAPTION)
                .color(p.text_tertiary),
        );
        ui.label(
            RichText::new(value)
                .size(theme::text::SUBHEADING + 2.0)
                .strong()
                .color(p.text_primary),
        );
    });
}

// ---------------------------------------------------------------------------
// 品牌标识
// ---------------------------------------------------------------------------

/// 侧边栏品牌标识：应用图标磁贴 + 字标。
///
/// ## 为什么是"磁贴 + 字标"而不是两行文字
///
/// 改造前的品牌区是"16px 自绘菱形 + 两行文字（`LeetCode` / `Compass 学习参谋`）"。
/// 两处问题：
///
/// 1. **16px 的标记太小**，在 216px 宽的侧边栏里几乎不成形，读起来更像
///    一个装饰性项目符号，而不像品牌；
/// 2. **两行文字把品牌名与产品定位混在一起**，用户第一眼看到的是
///    "Compass 学习参谋"这串需要额外解读的短语，而不是应用叫什么。
///
/// 新形态只保留单一品牌名，并把它交给一个 34px 的圆角磁贴承载——
/// 磁贴是桌面应用最通用的品牌载体（任务栏、Dock、应用列表里都是它），
/// 用户不需要学习就能认出"这是应用本身"。字标则用 [`text::HEADING`]
/// 档，与顶栏页面名同级，让品牌在层级上高于下方的导航项。
///
/// ## 磁贴里的字形
///
/// 用 [`IconKind::Code`]（尖括号对）而非在 34px 里塞进 "LeetCode" 三个字：
/// 内嵌的 SimHei 只有单一字重，汉字缩到 12px 以下笔画会粘连，而直线构成的
/// 符号不会。字形色取 `text_on_accent`，保证在任何主题下都与磁贴底
/// 形成反色关系。
pub fn brand_mark(ui: &mut Ui, p: &Palette, name: &str) {
    /// 磁贴边长。取 34 而非 32：与下方导航项的 40px 行高形成"略小一档"
    /// 的层级，同时留出圆角与字形的呼吸空间。
    const TILE: f32 = 34.0;
    /// 圆角。比容器的 10 略小、比控件的 8 略大，处于"应用图标"的观感区间。
    const RADIUS: u8 = 10;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::SM;

        let (rect, _) = ui.allocate_exact_size(egui::vec2(TILE, TILE), egui::Sense::hover());
        let radius = CornerRadius::same(RADIUS);

        ui.painter().rect_filled(rect, radius, p.accent);
        // 顶部高光线沿用玻璃容器的那一层：磁贴虽是不透明实心，
        // 但"光从上方打下来"的暗示能让它从平面色块变成有厚度的图标。
        theme::paint_glass_highlight(ui, rect, RADIUS as f32, p);
        // 底板色必须传磁贴自己的填充色（`accent`）——`icon` 的挖空逻辑
        // 依赖它，传 `panel_fill` 会在字形边缘留下色斑（见 `theme::icon`）。
        theme::icon(
            ui,
            rect.center(),
            TILE * 0.54,
            theme::IconKind::Code,
            p.text_on_accent,
            p.accent,
        );

        // 字标与磁贴垂直居中对齐——`ui.horizontal` 默认 `Align::Center`，
        // 因此这里只需保证标签本身不额外占高。
        ui.label(
            RichText::new(name)
                .size(theme::text::HEADING)
                .color(p.text_primary),
        );
    });
}

// ---------------------------------------------------------------------------
// 容器
// ---------------------------------------------------------------------------

/// 玻璃卡片容器。
///
/// 四层叠加中由 `Frame` 提供三层（半透明底、描边、投影），第四层
/// "顶部高光线"在容器绘制后补画——这是玻璃质感最关键的一层。
pub fn glass_card<R>(ui: &mut Ui, p: &Palette, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = theme::glass_frame(p, GlassSurface::Card, theme::space::MD as i8);
    let inner = frame.show(ui, add_contents);
    theme::paint_glass_highlight(ui, inner.response.rect, 10.0, p);
    inner.inner
}

/// 不透明信息卡片（无玻璃、无投影）。
///
/// 用于表格、代码块这类**内容密集**的区域：半透明底会让滚动内容透出，
/// 损害可读性。这类区域一律使用实心底。
pub fn card<R>(ui: &mut Ui, p: &Palette, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(p.bg_elevated)
        .stroke(egui::Stroke::new(1.0, p.border_subtle))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(theme::space::MD as i8))
        .show(ui, add_contents)
        .inner
}

/// 圆形玻璃按钮（图标按钮）。
///
/// 用于主题切换、刷新等操作。返回 `Response` 供调用方判断点击。
pub fn glass_icon_button(
    ui: &mut Ui,
    p: &Palette,
    size: f32,
    icon: theme::IconKind,
    hint: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());

    let hovered = response.hovered();
    let fill = if hovered {
        p.glass_fill_button_hover
    } else {
        p.glass_fill_button
    };
    let radius = CornerRadius::same((size * 0.5) as u8);
    // 静止态用 `control_stroke` 而非 `glass_stroke`：这是**可点击**的按钮，
    // 必须让用户看出它可以点。`glass_stroke` 是容器轮廓的令牌，
    // 在浅色主题下淡到几乎不存在（见 `Palette::control_stroke` 的说明）。
    let stroke = if hovered {
        egui::Stroke::new(1.0, p.accent)
    } else {
        egui::Stroke::new(1.0, p.control_stroke)
    };

    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    // 顶部高光。
    theme::paint_glass_highlight(ui, rect, size * 0.5, p);

    let icon_color = if hovered { p.accent } else { p.text_secondary };
    // 底板色传**按钮自己刚画的填充色**（而非 `panel_fill`）：月牙靠底色
    // 圆挖空实现，用错颜色会在缺口处留下与按钮底不符的色斑。
    theme::icon(ui, rect.center(), size * 0.5, icon, icon_color, fill);

    if hint.is_empty() {
        response
    } else {
        response.on_hover_text(hint)
    }
}

/// 侧边栏 / 顶栏等贴边玻璃面板。
///
/// 与 [`glass_card`] 的区别：无圆角、无投影（贴边的面板不应有悬浮感），
/// 但有描边与高光线——高光线由 [`theme::paint_glass_highlight`] 在
/// `radius = 0` 下补画，因此它会**贯通整条边**（玻璃卡片则需在圆角处避让）。
///
/// 传入的 `surface` 必须是 [`GlassSurface::Sidebar`] 或
/// [`GlassSurface::Bar`]；其他取值虽有定义但语义不符（卡片应走
/// [`glass_card`]，按钮应走 [`glass_icon_button`]）。
///
/// 当前 `app.rs` 的侧边栏与顶栏直接自绘玻璃底（`Panel::show` 的闭包已
/// 占用内容区，再嵌 `Frame` 会被描边挤掉 1px），因此本函数在产品代码
/// 里暂无调用点。保留它有两个理由：一是 `Panel` 之外仍可能有贴边容器
/// 需要它，二是 [`components_render_in_both_themes`] 用它验证
/// [`GlassSurface::Sidebar`] / [`GlassSurface::Bar`] 两档配色的渲染完整性。
#[allow(dead_code)]
pub fn glass_bar<R>(
    ui: &mut Ui,
    p: &Palette,
    surface: GlassSurface,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    debug_assert!(
        matches!(surface, GlassSurface::Sidebar | GlassSurface::Bar),
        "glass_bar 只用于贴边面板"
    );

    let frame = theme::glass_frame(p, surface, 0)
        .corner_radius(CornerRadius::ZERO)
        .shadow(egui::epaint::Shadow::NONE);
    let inner = frame.show(ui, add_contents);
    theme::paint_glass_highlight(ui, inner.response.rect, 0.0, p);
    inner.inner
}

// ---------------------------------------------------------------------------
// 图表
// ---------------------------------------------------------------------------

/// 绘制分段进度条（用于展示难度构成等比例数据）。
pub fn segmented_bar(ui: &mut Ui, p: &Palette, segments: &[(f64, Color32)], height: f32) {
    let total: f64 = segments.iter().map(|(v, _)| v.max(0.0)).sum();
    let width = ui.available_width();

    if total <= 0.0 {
        // 无数据时画一条空槽，保持布局稳定。
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, p.bg_sunken);
        return;
    }

    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();

    let mut x = rect.left();
    for (value, color) in segments {
        let v = value.max(0.0);
        if v <= 0.0 {
            continue;
        }
        let w = (v / total) as f32 * width;
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, height));
        painter.rect_filled(seg, 0.0, *color);
        x += w;
    }
}

/// 绘制 Rating 趋势折线图。
///
/// 自绘而非引入图表库：需求简单（单条折线 + 坐标轴），引入依赖
/// 会显著增加编译时间与二进制体积。
pub fn line_chart(
    ui: &mut Ui,
    p: &Palette,
    values: &[f64],
    height: f32,
    color: Color32,
    y_label: &str,
) {
    let width = ui.available_width();
    let desired = egui::vec2(width, height);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());

    // 留出左侧空间标注纵轴数值，底部留出横轴说明空间。
    let chart = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 48.0, rect.top() + 8.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 20.0),
    );

    let painter = ui.painter();

    if values.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "数据不足，无法绘制趋势",
            egui::FontId::proportional(theme::text::BODY_SMALL),
            p.text_tertiary,
        );
        return;
    }

    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // 上下各留 5% 余量，避免折线贴边。
    let pad = ((max - min) * 0.05).max(1.0);
    let lo = min - pad;
    let hi = max + pad;
    let span = (hi - lo).max(f64::EPSILON);

    let to_point = |i: usize, v: f64| -> egui::Pos2 {
        let tx = if values.len() == 1 {
            0.5
        } else {
            i as f32 / (values.len() - 1) as f32
        };
        let ty = ((v - lo) / span) as f32;
        egui::pos2(
            chart.left() + tx * chart.width(),
            // y 轴翻转：数值大在上。
            chart.bottom() - ty * chart.height(),
        )
    };

    // 背景与边框。
    painter.rect_filled(chart, 4.0, p.chart_bg);
    painter.rect_stroke(
        chart,
        4.0,
        egui::Stroke::new(1.0, p.chart_grid),
        egui::StrokeKind::Inside,
    );

    // 参考线（三等分）。
    for frac in [0.25f32, 0.5, 0.75] {
        let y = chart.top() + chart.height() * frac;
        painter.line_segment(
            [egui::pos2(chart.left(), y), egui::pos2(chart.right(), y)],
            egui::Stroke::new(0.5, p.chart_grid),
        );
    }

    // 纵轴刻度与单位。
    let label_font = egui::FontId::proportional(theme::text::CAPTION - 1.5);
    painter.text(
        egui::pos2(chart.left() - 4.0, chart.top()),
        egui::Align2::RIGHT_TOP,
        format!("{max:.0}"),
        label_font.clone(),
        p.text_tertiary,
    );
    painter.text(
        egui::pos2(chart.left() - 4.0, chart.bottom()),
        egui::Align2::RIGHT_BOTTOM,
        format!("{min:.0}"),
        label_font.clone(),
        p.text_tertiary,
    );
    painter.text(
        egui::pos2(rect.left(), chart.top() - 6.0),
        egui::Align2::LEFT_BOTTOM,
        y_label,
        label_font.clone(),
        p.text_tertiary,
    );

    // 折线。
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| to_point(i, *v))
        .collect();
    for w in points.windows(2) {
        painter.line_segment([w[0], w[1]], egui::Stroke::new(1.8, color));
    }

    // 数据点。点多时省略，避免视觉杂乱。
    if points.len() <= 40 {
        for p in &points {
            painter.circle_filled(*p, 2.5, color);
        }
    }

    // 最后一个点高亮（代表当前值）。
    if let Some(last) = points.last() {
        painter.circle_filled(*last, 4.0, color);
        if let Some(last_value) = values.last() {
            painter.text(
                egui::pos2(last.x, last.y - 8.0),
                egui::Align2::CENTER_BOTTOM,
                format!("{last_value:.0}"),
                egui::FontId::proportional(theme::text::CAPTION),
                color,
            );
        }
    }

    // 横轴说明。
    painter.text(
        egui::pos2(chart.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        format!("共 {} 场（由早到晚）", values.len()),
        label_font,
        p.text_tertiary,
    );
}

/// 绘制水平条形图（用于展示标签掌握度等）。
pub fn horizontal_bar(ui: &mut Ui, p: &Palette, ratio: f32, color: Color32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();

    // 底槽。
    painter.rect_filled(rect, 3.0, p.bg_sunken);

    // 填充部分。
    let r = ratio.clamp(0.0, 1.0);
    if r > 0.0 {
        let filled =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * r, rect.height()));
        painter.rect_filled(filled, 3.0, color);
    }
}

// ---------------------------------------------------------------------------
// 状态提示
// ---------------------------------------------------------------------------

/// 空状态提示。
///
/// 统一的空状态视觉能显著提升可用性——用户看到"该做什么"而不是空白页。
pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(
            RichText::new(icon)
                .size(36.0)
                .color(p.text_disabled),
        );
        ui.add_space(theme::space::SM);
        ui.label(
            RichText::new(title)
                .size(theme::text::SUBHEADING)
                .color(p.text_secondary),
        );
        ui.add_space(theme::space::XS);
        ui.label(
            RichText::new(hint)
                .size(theme::text::BODY_SMALL)
                .color(p.text_tertiary),
        );
        ui.add_space(40.0);
    });
}

/// 加载中提示。
pub fn loading_state(ui: &mut Ui, p: &Palette, message: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.spinner();
        ui.add_space(10.0);
        ui.label(RichText::new(message).color(p.text_secondary));
    });
}

/// 错误提示条。
pub fn error_banner(ui: &mut Ui, p: &Palette, message: &str) {
    banner(ui, "⚠", message, p.banner_error);
}

/// 信息提示条。
pub fn info_banner(ui: &mut Ui, p: &Palette, message: &str) {
    banner(ui, "ℹ", message, p.banner_info);
}

/// 成功提示条。
pub fn success_banner(ui: &mut Ui, p: &Palette, message: &str) {
    banner(ui, "✓", message, p.banner_success);
}

/// 警告提示条。
///
/// 当前五个页面都没有"警告"级别的提示（错误走 `error_banner`，能力边界
/// 走 `info_banner`），因此产品代码里暂无调用点。保留它是因为
/// [`components_render_in_both_themes`] 需要覆盖四种横幅的配色完整性——
/// 删掉它会让 `banner_warning` 这组令牌失去唯一的渲染验证。
#[allow(dead_code)]
pub fn warning_banner(ui: &mut Ui, p: &Palette, message: &str) {
    banner(ui, "!", message, p.banner_warning);
}

/// 提示条的公共绘制逻辑。
fn banner(ui: &mut Ui, icon: &str, message: &str, colors: BannerColors) {
    // 提示条用实心底而非玻璃底：它的作用是"让人注意"，半透明会削弱强调感。
    egui::Frame::NONE
        .fill(colors.bg)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon).color(colors.icon));
                ui.label(
                    RichText::new(message)
                        .size(theme::text::BODY_SMALL)
                        .color(colors.text),
                );
            });
        });
}

/// 绘制账号状态指示条。
///
/// 把"当前是否已授权"这个关键状态前置到界面顶部，因为它在很大程度上
/// 决定了用户能看到哪些数据。
pub fn account_status_bar(ui: &mut Ui, p: &Palette, username: &str, authenticated: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("账号：")
                .size(theme::text::BODY_SMALL)
                .color(p.text_tertiary),
        );
        if username.is_empty() {
            ui.label(
                RichText::new("未绑定")
                    .size(theme::text::BODY_SMALL)
                    .color(p.text_tertiary),
            );
        } else {
            ui.label(
                RichText::new(username)
                    .size(theme::text::BODY_SMALL)
                    .strong()
                    .color(p.text_primary),
            );
        }

        ui.separator();

        if authenticated {
            ui.label(
                RichText::new("● 已授权（可读取完成状态）")
                    .size(theme::text::CAPTION)
                    .color(p.success),
            );
        } else {
            ui.label(
                RichText::new("○ 未授权（仅公开数据）")
                    .size(theme::text::CAPTION)
                    .color(p.text_tertiary),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palettes() -> [Palette; 2] {
        [Palette::light(), Palette::dark()]
    }

    /// 未选中的难度胶囊必须带**可见**边框。
    ///
    /// 胶囊是可点击的筛选开关。改造前未选中态用 `border_subtle`
    /// （浅色下 8% 不透明度），叠在浅灰底上等于没有边框——用户看不出
    /// 这是一排可以点的筛选条件，只当它是文字标签。
    ///
    /// 断言落在**图元**上而不是"没 panic"：描边取错颜色不会让任何东西崩溃，
    /// 只有检查 `RectShape::stroke` 才能发现。
    #[test]
    fn difficulty_pill_unselected_uses_control_stroke() {
        use egui::Shape;

        for (name, p) in [("浅色", Palette::light()), ("深色", Palette::dark())] {
            let ctx = egui::Context::default();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                difficulty_pill(ui, &p, Difficulty::Easy, false);
            });
            out.textures_delta.clear();

            let strokes: Vec<egui::Stroke> = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    Shape::Rect(r) => Some(r.stroke),
                    _ => None,
                })
                .collect();

            assert!(
                strokes
                    .iter()
                    .any(|s| s.color == p.control_stroke && s.width > 0.0),
                "{name}主题下未选中的难度胶囊没有使用 `control_stroke` 描边"
            );
            // 反向断言：不得退回那个几乎不可见的容器轮廓色。
            assert!(
                !strokes.iter().any(|s| s.color == p.border_subtle),
                "{name}主题下难度胶囊仍在使用 `border_subtle`"
            );
        }
    }

    #[test]
    fn difficulty_colors_are_distinct() {
        for p in palettes() {
            let colors: Vec<Color32> = Difficulty::all()
                .iter()
                .map(|d| difficulty_color(&p, *d))
                .collect();
            for i in 0..colors.len() {
                for j in (i + 1)..colors.len() {
                    assert_ne!(
                        colors[i], colors[j],
                        "不同难度的颜色必须可区分，否则用户无法快速识别"
                    );
                }
            }
        }
    }

    #[test]
    fn status_colors_distinguish_solved_from_unknown() {
        for p in palettes() {
            // 这几个状态在语义上差别最大，视觉上必须能分开。
            assert_ne!(
                status_color(&p, SolveStatus::Solved),
                status_color(&p, SolveStatus::Unknown)
            );
            assert_ne!(
                status_color(&p, SolveStatus::Solved),
                status_color(&p, SolveStatus::Todo)
            );
            assert_ne!(
                status_color(&p, SolveStatus::Attempted),
                status_color(&p, SolveStatus::Todo)
            );
        }
    }

    #[test]
    fn all_statuses_map_to_a_color() {
        for p in palettes() {
            for s in [
                SolveStatus::Solved,
                SolveStatus::Attempted,
                SolveStatus::Todo,
                SolveStatus::Unknown,
            ] {
                let c = status_color(&p, s);
                assert!(c.a() > 0, "颜色必须完全不透明");
            }
        }
    }

    /// **主题化改造的核心断言**：同一语义在两套主题下必须给出**不同**色值。
    ///
    /// 若某个语义色在两套主题下相同，说明它没有跟着主题走——那样的颜色
    /// 必然在其中一套主题下不可读（例如深色主题里仍用深灰文字）。
    #[test]
    fn semantic_colors_follow_the_theme() {
        let l = Palette::light();
        let d = Palette::dark();

        // 文字色必须随主题反转明暗。
        assert_ne!(l.text_primary, d.text_primary);
        assert_ne!(l.text_secondary, d.text_secondary);
        assert_ne!(l.text_tertiary, d.text_tertiary);

        // 容器与图表底也必须变化。
        assert_ne!(l.bg_panel, d.bg_panel);
        assert_ne!(l.chart_bg, d.chart_bg);
        assert_ne!(l.chart_grid, d.chart_grid);

        // 难度色随主题调整明度（深色下用更亮的色阶）。
        for diff in Difficulty::all() {
            assert_ne!(
                difficulty_color(&l, diff),
                difficulty_color(&d, diff),
                "{diff:?} 的难度色未随主题变化"
            );
        }
    }

    /// 玻璃容器与实心卡片必须视觉可分。
    #[test]
    fn glass_card_and_opaque_card_differ() {
        for p in palettes() {
            // 玻璃卡片底是半透明的，实心卡片底不透明。
            let glass = Palette::for_mode(if p == Palette::light() {
                crate::ui::theme::ThemeMode::Light
            } else {
                crate::ui::theme::ThemeMode::Dark
            });
            assert!(
                glass.glass_fill_card.a() < 255,
                "玻璃卡片底应为半透明"
            );
            assert_eq!(glass.bg_elevated.a(), 255, "实心卡片底应不透明");
        }
    }

    /// 玻璃开关必须真正改变卡片底的透明度。
    #[test]
    fn glass_toggle_changes_card_opacity() {
        use crate::ui::theme::{ThemeMode, UiTheme};

        let on = UiTheme::new(ThemeMode::Dark, true);
        let off = UiTheme::new(ThemeMode::Dark, false);

        assert!(on.palette().glass_fill_card.a() < 255);
        assert_eq!(off.palette().glass_fill_card.a(), 255);
    }

    /// 所有组件都必须在两套主题下可绘制而不 panic。
    ///
    /// 自绘组件里的坐标全是算术推导，一处符号错误就可能画出屏幕外的形状
    /// 或触发 egui 断言。这条用真实离屏 Context 走一遍主要组件。
    #[test]
    fn components_render_in_both_themes() {
        use crate::ui::theme::{ThemeMode, UiTheme};

        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            for glass in [true, false] {
                let theme = UiTheme::new(mode, glass);
                let p = *theme.palette();

                let ctx = egui::Context::default();
                let mut out = ctx.run_ui(egui::RawInput::default(), |ctx| {
                    egui::Area::new(egui::Id::new("widget_test")).show(ctx, |ui| {
                        ui.set_width(600.0);

                        glass_card(ui, &p, |ui| {
                            ui.label("卡片内容");
                        });
                        card(ui, &p, |ui| {
                            ui.label("实心卡片");
                        });
                        glass_bar(ui, &p, GlassSurface::Sidebar, |ui| {
                            ui.label("侧边栏");
                        });
                        glass_bar(ui, &p, GlassSurface::Bar, |ui| {
                            ui.label("顶栏");
                        });

                        for icon in [theme::IconKind::Moon, theme::IconKind::Sun] {
                            glass_icon_button(ui, &p, 36.0, icon, "提示");
                        }

                        difficulty_badge(ui, &p, Difficulty::Hard);
                        status_badge(ui, &p, SolveStatus::Solved);
                        metric_label(ui, &p, "指标", "42");
                        segmented_bar(ui, &p, &[(1.0, p.success), (2.0, p.danger)], 8.0);
                        line_chart(ui, &p, &[1.0, 3.0, 2.0], 120.0, p.accent, "Rating");
                        horizontal_bar(ui, &p, 0.6, p.success, 8.0);
                        empty_state(ui, &p, "◆", "标题", "提示");
                        loading_state(ui, &p, "加载中");

                        error_banner(ui, &p, "错误");
                        info_banner(ui, &p, "信息");
                        success_banner(ui, &p, "成功");
                        warning_banner(ui, &p, "警告");

                        account_status_bar(ui, &p, "wuhu", true);
                    });
                });
                // 纹理增量必须消费掉，否则 Context 析构时 panic。
                out.textures_delta.clear();
            }
        }
    }
}
