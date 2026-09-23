//! 主题与语义色板。
//!
//! ## 为什么需要这一层
//!
//! 改造前，颜色硬编码散落在 7 个文件、共 188 处（`Color32::from_gray(110)`
//! 之类的字面量遍布各页面）。这带来两个问题：
//!
//! 1. **无法主题化**——深色主题下这些浅色字面量全部失效（白底黑字、灰字
//!    不可读）。需求"主题切换覆盖所有页面与组件"因此无法达成。
//! 2. **无法统一调整**——想微调一档次要文字色，要改几十处。
//!
//! 本模块把颜色收敛为**语义令牌**（semantic token）：页面不再关心"什么颜色"，
//! 只关心"什么用途"。`text_secondary` 在浅色下是深灰、深色下是浅灰，
//! 页面代码完全不需要分支。
//!
//! ## 明暗两套如何生效
//!
//! egui 0.36 把 `Style` 按主题存了两份（`dark_style` / `light_style`），
//! 由 `ThemePreference` 决定当前启用哪一份。因此：
//!
//! - **框架组件**（面板底、按钮、输入框、滚动条、工具提示、文本选区、光标）
//!   会**自动**跟随 `ctx.set_theme()` 切换——这部分无需逐组件改动。
//! - **自绘内容**（表格行、图表、聊天气泡、徽章）需要显式读取 [`Palette`]。
//!
//! [`install`] 一次性把明暗两套 `Visuals` 都配置好；此后切换主题只需
//! [`set_theme`]，无需重装样式。
//!
//! ## 玻璃质感的实现边界（重要）
//!
//! egui **没有** backdrop-filter，无法做真正的背景模糊。本模块用四层叠加
//! 近似玻璃质感：半透明底 + 细描边 + **顶部高光线** + 投影。
//! 人眼判断"像不像玻璃"主要依赖边缘高光与通透度，而非背景模糊，
//! 因此这个近似在视觉上是成立的。
//!
//! 高光线（`glass_stroke_top`）是四层中最关键的一层——去掉它，半透明方块
//! 只会像"没画完的色块"；加上它，立刻有"玻璃片"的观感。

use egui::{Color32, Context, CornerRadius, Margin, Shadow, Stroke, Theme, Visuals};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 主题模式
// ---------------------------------------------------------------------------

/// 界面主题。
///
/// **刻意保持二值**，不提供"跟随系统"第三态：需求要求"单个按钮切换 +
/// 图标随主题动态变化（浅色显月亮、深色显太阳）"。第三态会让按钮图标的
/// 语义产生歧义——跟随时该显示月亮还是太阳？二值语义才与单按钮切换自洽。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    /// 深色主题：深底浅字。
    ///
    /// 作为默认值：深色下玻璃质感的表现力更强（高光对比明显），
    /// 且与开发者的常见工作环境一致。
    #[default]
    Dark,
    /// 浅色主题：浅底深字。
    Light,
}

impl ThemeMode {
    /// 界面标签。
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Dark => "深色",
            Self::Light => "浅色",
        }
    }

    /// 映射到 egui 的主题枚举。
    pub fn to_egui(self) -> Theme {
        match self {
            Self::Dark => Theme::Dark,
            Self::Light => Theme::Light,
        }
    }

    /// 是否为深色主题。
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// 切换后的主题。
    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    /// 切换按钮应显示的图标。
    ///
    /// **这是需求明确指定的映射**：按钮上显示的是"点击后会变成什么"，
    /// 即浅色主题下显示月亮（点击转深色），深色主题下显示太阳（点击转浅色）。
    pub fn toggle_icon(self) -> IconKind {
        match self {
            // 当前浅色 → 点它会变深色 → 显示月亮。
            Self::Light => IconKind::Moon,
            // 当前深色 → 点它会变浅色 → 显示太阳。
            Self::Dark => IconKind::Sun,
        }
    }

    /// 切换按钮的悬停提示。
    pub fn toggle_hint(self) -> &'static str {
        match self {
            Self::Light => "切换到深色主题",
            Self::Dark => "切换到浅色主题",
        }
    }
}

// ---------------------------------------------------------------------------
// 自绘图标
// ---------------------------------------------------------------------------

/// 需要自绘的图标种类。
///
/// **为什么不使用 emoji 字符**：
///
/// 1. 项目内嵌的中文字体 SimHei **不含 emoji 字形**（只覆盖 GB2312 汉字集），
///    用 emoji 会渲染成豆腐块；
/// 2. egui 内置的 NotoEmoji 是**彩色** emoji 字体，其鲜艳的色块与"高级玻璃
///    质感"的克制调性严重冲突，且在深色底上显得突兀；
/// 3. 自绘可以精确控制尺寸、线宽、颜色，使其随主题前景色变化。
///
/// 因此所有图标一律用 [`icon`] 以矢量方式绘制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// 月亮——浅色主题下的切换按钮。
    Moon,
    /// 太阳——深色主题下的切换按钮。
    Sun,
    /// 列表——题库页。
    List,
    /// 靶心——推荐页。
    Target,
    /// 柱状图——竞赛复盘页。
    Chart,
    /// 对话气泡——AI 助理页。
    Chat,
    /// 齿轮——设置页。
    Gear,
    /// 尖括号对 `< >`——侧边栏品牌标识。
    ///
    /// 选它作品牌字形的原因：品牌字标是 "LeetCode Compass"，而这么长的
    /// 文案无法在 30px 见方的磁贴里完整呈现（会退化成难以辨认的小字），
    /// 因此磁贴里放一个**语义等价**的图形符号：尖括号对是"代码"最通用的
    /// 视觉约定，且由四条直线构成，在小尺寸下依然清晰——这一点很重要，
    /// 因为内嵌的 SimHei 只有单一字重，缩小后的汉字笔画会粘连，而直线不会。
    Code,
}

// ---------------------------------------------------------------------------
// 字号层级
// ---------------------------------------------------------------------------

/// 字号层级。
///
/// ## 为什么不能靠字重建立层级
///
/// 项目内嵌的 SimHei 只有**单一字重**，`RichText::strong()` 在本项目中
/// 仅改变颜色、不改变笔画的粗细。因此视觉层级**必须**由
/// "字号 + 颜色"两个维度共同建立，不能依赖粗细。
///
/// 这组常量把原先散落在各页面的 `.size(11.0)` / `.size(12.0)` /
/// `.size(13.5)` 等魔法数字收敛为具名档位。
///
/// **不设 `DISPLAY` 档**：一个没有被任何位置使用的字号档位是一种负债——
/// 它会诱导后续开发者在"想强调一下"时随手取用它，从而破坏层级纪律。
/// 最大的实际用字是顶栏的页面名（[`HEADING`]）——顶栏已从"英雄标题区"
/// 降级为上下文档，不再需要更大的字号。
pub mod text {
    /// 区块标题（顶栏页面名、卡片与面板标题）。
    pub const HEADING: f32 = 18.0;
    /// 小节标题。
    pub const SUBHEADING: f32 = 15.0;
    /// 正文。
    pub const BODY: f32 = 14.0;
    /// 次要正文。
    pub const BODY_SMALL: f32 = 12.5;
    /// 说明、表头、刻度。
    pub const CAPTION: f32 = 11.5;
    /// 等宽：题号、精确数字、代码。
    pub const MONO: f32 = 12.5;
}

/// 间距栅格（4 的倍数）。
///
/// 收敛原先散落的 `ui.add_space(6.0)` / `(10.0)` / `(14.0)` 等魔法数字。
pub mod space {
    /// 图标与文字、紧邻元素。
    pub const XS: f32 = 4.0;
    /// 同组元素。
    pub const SM: f32 = 8.0;
    /// 卡片内边距。
    pub const MD: f32 = 12.0;
    /// 区块之间。
    pub const LG: f32 = 18.0;
    /// 页面外边距。
    pub const XL: f32 = 24.0;
}

// ---------------------------------------------------------------------------
// 反馈横幅配色
// ---------------------------------------------------------------------------

/// 一组反馈横幅的四色定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BannerColors {
    /// 底色。
    pub bg: Color32,
    /// 描边。
    pub border: Color32,
    /// 图标颜色。
    pub icon: Color32,
    /// 正文颜色。
    pub text: Color32,
}

// ---------------------------------------------------------------------------
// 语义色板
// ---------------------------------------------------------------------------

/// 语义色板。
///
/// 每一档都是"用途"而非"颜色"——`text_secondary` 而非 `gray_110`。
/// 这是主题化能覆盖全部组件的前提：页面只说用途，色值由主题决定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    // ---- 组 1：基底与层级 ----
    /// 窗口最底层背景。
    pub bg_base: Color32,
    /// 内容区背景。
    pub bg_panel: Color32,
    /// 抬升表面（关闭玻璃时容器的实心底色）。
    pub bg_elevated: Color32,
    /// 凹陷表面（输入槽、图表底）。
    pub bg_sunken: Color32,

    // ---- 组 2：玻璃材质 ----
    /// 侧边栏玻璃底。
    pub glass_fill_sidebar: Color32,
    /// 顶栏玻璃底。
    pub glass_fill_bar: Color32,
    /// 卡片玻璃底。
    pub glass_fill_card: Color32,
    /// 按钮玻璃底。
    pub glass_fill_button: Color32,
    /// 按钮玻璃底（悬停态）。
    pub glass_fill_button_hover: Color32,
    /// 顶部高光线——玻璃质感的关键。
    pub glass_stroke_top: Color32,
    /// 主描边。
    pub glass_stroke: Color32,
    /// 投影。
    pub glass_shadow: Color32,

    // ---- 组 3：文字层级 ----
    /// 正文与标题。
    pub text_primary: Color32,
    /// 次要说明。
    pub text_secondary: Color32,
    /// 提示、占位、刻度。
    pub text_tertiary: Color32,
    /// 禁用态。
    pub text_disabled: Color32,
    /// 强调底上的文字。
    pub text_on_accent: Color32,

    // ---- 组 4：强调与语义色 ----
    /// 品牌主色、选中态。
    pub accent: Color32,
    /// 强调底（低透明度）。
    pub accent_subtle: Color32,
    /// 成功 / 已通过 / 简单。
    pub success: Color32,
    /// 警告 / 尝试过 / 中等。
    pub warning: Color32,
    /// 危险 / 错误 / 困难。
    pub danger: Color32,
    /// 中性 / 未开始 / 未知。
    pub neutral: Color32,
    /// 信息。
    pub info: Color32,

    // ---- 组 5：交互态 ----
    //
    // 本组的**反馈类**令牌（悬停底、控件悬停底、选区底）一律取**不透明**色值。
    // 理由见 `hover_overlay` 的文档——半透明在这里不是"更精致"，而是
    // "根本看不见"，或反过来把整块区域冲成纯白。
    /// 列表行悬停底。
    ///
    /// **必须是不透明色。** 这一条由一个真实缺陷反推出来：
    /// 本令牌原先是 `rgba(0x0F172A, 0x0A)`（浅色）与 `rgba(0xFFFFFF, 0x10)`
    /// （深色），两条都在实际渲染中失效——
    ///
    /// | 主题 | 原值 | 实际合成结果 | 用户看到的现象 |
    /// |---|---|---|---|
    /// | 浅色 | 4% 深蓝 | **纯白**（叠在近白行底上被冲掉） | 鼠标移上去**毫无反应** |
    /// | 深色 | 6% 白 | **纯白** | 整行变白，标题（`#E6EAF0`）对比度掉到 **1.08:1，完全看不清** |
    ///
    /// 两条症状看似相反，同源于 `rgba()` 的加性叠加语义（见 `rgba` 的文档）。
    ///
    /// 取值约束（由 `row_hover_is_perceptible_in_both_themes` 与
    /// `dark_hovered_row_keeps_titles_readable` 两条测试锁定）：
    /// 悬停底既要与**行底**和**斑马纹底**都可区分，又要在深色下
    /// 保住标题的可读性——后者是本轮最严重的那条。
    pub hover_overlay: Color32,
    /// 分隔线、卡片描边。
    pub border_subtle: Color32,
    /// 表头下沿。
    pub border_strong: Color32,
    /// 通用分隔线（比 `border_subtle` 更显眼，用于区块之间的水平线）。
    pub divider: Color32,
    /// **交互控件描边**——按钮、输入框、下拉框的静止态边框。
    ///
    /// 与 `glass_stroke` 刻意分开，因为两者的职责不同：
    ///
    /// - `glass_stroke` 是**容器**轮廓。大面积半透明色块靠通透度与顶部高光
    ///   已经能表达边界，轮廓只需极淡，过重反而破坏玻璃的通透感。
    /// - `control_stroke` 是**可交互控件**的轮廓。它的唯一职责是回答
    ///   "这里能点吗 / 这里能输入吗"，因此必须在静止态就清晰可辨。
    ///
    /// 复用 `glass_stroke` 会导致浅色主题下的按钮与输入框完全看不出边界
    /// （该令牌在浅色下只有 9% 不透明度，叠在纯白卡片上几乎与底色相同），
    /// 用户只能靠位置猜测哪里可以输入——这正是本次修复的问题。
    pub control_stroke: Color32,

    // ---- 组 6：反馈横幅 ----
    /// 错误横幅。
    pub banner_error: BannerColors,
    /// 信息横幅。
    pub banner_info: BannerColors,
    /// 成功横幅。
    pub banner_success: BannerColors,
    /// 警告横幅。
    pub banner_warning: BannerColors,

    // ---- 组 7：对话气泡 ----
    //
    // 三种角色必须是**三套独立令牌**，不能靠"一套 + 调明度"推导：
    // 浅色主题下气泡是浅底深字，深色主题下是深底浅字，两者方向相反。
    // 另有一项守卫测试断言三者在同一主题下两两可区分。
    /// 用户消息气泡底。
    pub bubble_user_bg: Color32,
    /// 用户消息气泡描边。
    pub bubble_user_border: Color32,
    /// 助理消息气泡底。
    pub bubble_assistant_bg: Color32,
    /// 助理消息气泡描边。
    pub bubble_assistant_border: Color32,
    /// 错误消息气泡底。
    pub bubble_error_bg: Color32,
    /// 错误消息气泡描边。
    pub bubble_error_border: Color32,
    /// 错误消息气泡文字——错误态需要专门的文字色，不能用 `text_primary`。
    pub bubble_error_text: Color32,

    // ---- 组 8：图表 ----
    /// 图表背景。
    pub chart_bg: Color32,
    /// 图表网格线。
    pub chart_grid: Color32,

    // ---- 组 9：侧边栏导航 ----
    //
    // 侧边栏导航项**不复用** `hover_overlay` / `selected_overlay`。
    //
    // 理由：那两个令牌服务于"列表行悬停"——叠在内容区的大块实心底上，
    // 只需极轻微的明度差即可提示"鼠标在这一行"。而导航项是**独立的
    // 可点击目标**，它必须回答"我现在在哪一页、还能去哪一页"，
    // 对三态（未选中 / 悬停 / 选中）的可辨识度要求高一个量级。
    //
    // 更关键的是深色主题下的可读性：选中态若沿用 16% 的主色叠加，
    // 主色文字（`accent`）与叠出来的亮蓝底几乎同色，标题会"糊"在
    // 背景里。这里为深色主题单独取了一组**高对比**色值，
    // 并由测试锁定对比度下限。
    /// 导航项文字（未选中）。
    pub sidebar_item_text: Color32,
    /// 导航项图标（未选中）。
    pub sidebar_item_icon: Color32,
    /// 导航项悬停底。
    pub sidebar_item_hover: Color32,
    /// 导航项选中底。
    pub sidebar_item_selected_bg: Color32,
    /// 导航项选中态文字与图标。
    pub sidebar_item_selected_text: Color32,
}

/// 从 `0xRRGGBB` 构造不透明颜色。
///
/// 用十六进制字面量书写色值，便于与设计稿逐字对照。
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// 从 `0xRRGGBB` + alpha 构造半透明颜色。
///
/// 直接构造结构体而非调用 `from_rgba_unmultiplied`——后者不是 `const fn`，
/// 无法在 `const` 上下文中使用。`Color32` 的字段就是 `(r, g, b, a)` 四个
/// `u8`，直接构造语义完全等价。
///
/// 注意：`Color32` 内部按**预乘 alpha** 存储（`from_rgba_unmultiplied` 会
/// 做乘法），而这里直接写入未乘的通道值。对本模块的用途（容器底色、
/// 叠加层）这是想要的行为，但**测试不可断言"读回的 RGB 等于传入值"**。
const fn rgba(hex: u32, alpha: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
        alpha,
    )
}

/// 由前景色派生一个"同一色相、极低不透明度"的胶囊背景。
///
/// ## 为什么放在这里而不是调用方
///
/// 这个派生规则原本写在 `contest.rs` 里（`tint()`），但它是一条**配色
/// 策略**而非页面逻辑：任何需要"用语义色做一个淡色标签底"的地方都应
/// 遵循同一套比例。散在页面里会导致各页胶囊深浅不一，而且违反
/// "颜色只由色板决定"的约定（见 `ui::guards`）。
///
/// ## 参数选择
///
/// alpha 取 28/255（约 11%）。这个值在浅色主题下叠在白色卡片上得到
/// 极淡的同色底，在深色主题下叠在深色卡片上得到略带色相的暗底——
/// 两个方向都成立，因此不需要按主题给两套常量。
///
/// **不可断言"读回的 RGB 等于传入值"**：`Color32` 按预乘 alpha 存储，
/// 而这里为了让淡色在视觉上保持色相，显式做了预乘。
pub const fn tint_bg(color: Color32) -> Color32 {
    const ALPHA: u32 = 28;
    // 预乘：把通道值按 alpha 比例缩小，避免叠加时偏白。
    Color32::from_rgba_premultiplied(
        (color.r() as u32 * ALPHA / 255) as u8,
        (color.g() as u32 * ALPHA / 255) as u8,
        (color.b() as u32 * ALPHA / 255) as u8,
        ALPHA as u8,
    )
}

impl Palette {
    /// 浅色主题色板。
    pub const fn light() -> Self {
        Self {
            // 组 1
            bg_base: rgb(0xF6F7F9),
            bg_panel: rgb(0xFFFFFF),
            // `bg_elevated` 必须**比面板底更暗**而不是更亮。
            //
            // 浅色主题下内容区已经是纯白 #FFFFFF，若抬升表面也用纯白，
            // 关闭玻璃时卡片会与背景完全融为一体、轮廓消失。
            // 因此浅色下改为"内容区纯白、抬升表面浅灰"——靠微弱明度差
            // 加描边表达层级边界。深色主题的方向相反（面板更暗、表面提亮），
            // 因为深色下"更暗"会直接掉进纯黑，无法再分层。
            bg_elevated: rgb(0xFAFBFC),
            bg_sunken: rgb(0xEDEFF3),

            // 组 2：浅色玻璃＝白色半透明
            glass_fill_sidebar: rgba(0xFFFFFF, 0xB0), // 69%
            glass_fill_bar: rgba(0xFFFFFF, 0xC2),     // 76%
            glass_fill_card: rgba(0xFFFFFF, 0x9C),    // 61%
            glass_fill_button: rgba(0xFFFFFF, 0x8A),  // 54%
            // 悬停底**刻意做成不透明**，与静止态的半透明不同。
            //
            // 浅色下静止态 `rgba(0xFFFFFF, 0x8A)` 叠在近白卡片上后就是纯白；
            // 悬停底若同样是白色系半透明（`rgba(0xFFFFFF, 0xCC)`），
            // 合成结果**也是纯白**——两个状态逐字节相同，鼠标移上去
            // 完全没有反馈。这是"浅色模式下悬停没有光效"的另一半成因。
            //
            // 反馈类颜色必须先保证"与相邻状态不同"，通透度是次要目标。
            glass_fill_button_hover: rgb(0xD8DEE8),
            glass_stroke_top: rgba(0xFFFFFF, 0xE6), // 上高光
            glass_stroke: rgba(0x0F172A, 0x18),     // 9%
            glass_shadow: rgba(0x0F172A, 0x14),     // 8%

            // 组 3
            text_primary: rgb(0x111827),
            text_secondary: rgb(0x4B5563),
            text_tertiary: rgb(0x8A93A2),
            text_disabled: rgb(0xB6BCC6),
            text_on_accent: rgb(0xFFFFFF),

            // 组 4
            accent: rgb(0x2563EB),
            // 文本选区高亮。同属反馈类，同样取不透明：
            // 原先 12% 主色叠在白底上被冲成纯白，选中的文字看不出被选中。
            accent_subtle: rgb(0xD3E3FD),
            success: rgb(0x059669),
            warning: rgb(0xD97706),
            danger: rgb(0xDC2626),
            neutral: rgb(0x9CA3AF),
            info: rgb(0x2563EB),

            // 组 5
            //
            // 悬停底取"比斑马纹（`#EDEFF3`）再深一档"：斑马纹是本表格里
            // 离悬停底最近的既有底色，悬停若只是"比白稍灰"，落在斑马纹行上
            // 就分不出来。实测 `#CFD7E3` 对行底 1.40:1、对斑马纹 1.26:1。
            hover_overlay: rgb(0xCFD7E3),
            border_subtle: rgba(0x0F172A, 0x14),    // 8%
            border_strong: rgba(0x0F172A, 0x24),    // 14%
            divider: rgba(0x0F172A, 0x1A),          // 10%
            // 控件描边用**不透明**色值而非 `rgba(...)`：边框是 1px 细线，
            // 半透明色在细线上的实际观感高度依赖其下方是什么底色
            // （卡片 vs 面板 vs 输入槽三者都不同），会出现"同一个按钮在
            // 不同位置深浅不一"。固定色值才能保证任何位置都同样清晰。
            //
            // 取值依据：相对纯白背景约 1.9:1 的对比度——足以让用户一眼看出
            // 边界，又不至于抢走内容焦点。这是"有边框"与"边框太重"之间的
            // 平衡点，由 `light_control_borders_are_visible` 测试锁定下限。
            control_stroke: rgb(0xB0BCCB),

            // 组 6
            banner_error: BannerColors {
                bg: rgb(0xFEF2F2),
                border: rgb(0xFECACA),
                icon: rgb(0xB91C1C),
                text: rgb(0x7F1D1D),
            },
            banner_info: BannerColors {
                bg: rgb(0xEFF6FF),
                border: rgb(0xBFDBFE),
                icon: rgb(0x1D4ED8),
                text: rgb(0x1E3A8A),
            },
            banner_success: BannerColors {
                bg: rgb(0xECFDF5),
                border: rgb(0xA7F3D0),
                icon: rgb(0x047857),
                text: rgb(0x065F46),
            },
            banner_warning: BannerColors {
                bg: rgb(0xFFFBEB),
                border: rgb(0xFDE68A),
                icon: rgb(0xB45309),
                text: rgb(0x92400E),
            },

            // 组 7：浅色气泡＝浅底深字
            bubble_user_bg: rgb(0xEFF6FF),
            bubble_user_border: rgb(0xBFDBFE),
            bubble_assistant_bg: rgb(0xF3F4F6),
            bubble_assistant_border: rgb(0xE1E4EA),
            bubble_error_bg: rgb(0xFEF2F2),
            bubble_error_border: rgb(0xFECACA),
            bubble_error_text: rgb(0x991B1B),

            // 组 8
            chart_bg: rgb(0xF8FAFC),
            chart_grid: rgb(0xE2E8F0),

            // 组 9：浅色导航三态
            //
            // 文字用 `#374151`（比 `text_secondary` 更深）：导航项是界面上
            // 最需要"一眼扫到"的元素，不该按次要说明的强度来着色。
            sidebar_item_text: rgb(0x374151),
            sidebar_item_icon: rgb(0x6B7280),
            // 悬停与选中底一律用不透明色值——理由同 `control_stroke`：
            // 导航项横跨整条侧边栏，底下是什么色无从假设。
            //
            // 浅色下侧边栏底本身已接近纯白，因此悬停/选中底需要比"浅灰"
            // 再深一档才看得出反馈（`#EDEFF3` 与白底只有 1.15:1，
            // 实测在屏幕上几乎看不出悬停）。
            sidebar_item_hover: rgb(0xDDE2EA),
            sidebar_item_selected_bg: rgb(0xD8E4FC),
            // 选中文字用比 `accent` 更深的蓝（`#1D4ED8`）：选中底本身已是
            // 浅蓝，若文字也用 `accent`（`#2563EB`），两者明度接近，
            // 在浅色主题下会"糊"成一片。
            sidebar_item_selected_text: rgb(0x1D4ED8),
        }
    }

    /// 深色主题色板。
    ///
    /// **注意横幅的四色是独立设计的，不是浅色版加暗**。
    /// 把浅色底 `gamma_multiply(0.2)` 变暗会得到一片几乎无色的灰，
    /// 丢失色相倾向后用户无法一眼分辨"这是错误还是提示"。
    /// 因此深色横幅采用"低明度 + 保留色相"的独立色值。
    pub const fn dark() -> Self {
        Self {
            // 组 1
            bg_base: rgb(0x0E1116),
            bg_panel: rgb(0x12161C),
            bg_elevated: rgb(0x181D25),
            bg_sunken: rgb(0x0A0D11),

            // 组 2：深色玻璃＝浅色叠加（半透明提亮），而非变暗
            glass_fill_sidebar: rgba(0x181D25, 0xD8),
            glass_fill_bar: rgba(0x161B22, 0xDE),
            glass_fill_card: rgba(0x1A2029, 0xC4), // 77%
            glass_fill_button: rgba(0x232A34, 0xD0),
            // 深色下原来的半透明值恰好没被冲白（合成结果 ≈ `#313A47`），
            // 这里直接把该合成值固化下来——同一个颜色，但不再依赖
            // "叠加在什么底色上"这个不可控前提。
            glass_fill_button_hover: rgb(0x363F4E),
            glass_stroke_top: rgba(0xFFFFFF, 0x24), // 14% 上高光
            glass_stroke: rgba(0xFFFFFF, 0x1A),     // 10%
            glass_shadow: rgba(0x000000, 0x52),     // 32%

            // 组 3
            text_primary: rgb(0xE6EAF0),
            text_secondary: rgb(0xA3ADBB),
            text_tertiary: rgb(0x6F7987),
            text_disabled: rgb(0x4A525E),
            text_on_accent: rgb(0x0B0E13),

            // 组 4
            accent: rgb(0x60A5FA),
            accent_subtle: rgb(0x2C4A73),
            success: rgb(0x34D399),
            warning: rgb(0xFBBF24),
            danger: rgb(0xF87171),
            neutral: rgb(0x6B7280),
            info: rgb(0x60A5FA),

            // 组 5
            //
            // 深色悬停底必须是"**提亮但仍是暗色**"。原先的 6% 白叠加被
            // 加性混合冲成纯白，把标题压到 1.08:1。
            //
            // 取值受两侧夹逼，是真正的约束满足问题：
            // - 太暗 → 与行底分不出来（下限：对行底 ≥ 1.2:1）；
            // - 太亮 → 行内最淡的题号（`text_tertiary`）被压掉
            //   （上限：题号 ≥ 3:1）。
            // `#26303C` 落在窗口内：对行底 1.26:1、对斑马纹 1.46:1，
            // 标题 11.0:1、题号 3.0:1。
            hover_overlay: rgb(0x26303C),
            border_subtle: rgba(0xFFFFFF, 0x14),    // 8%
            border_strong: rgba(0xFFFFFF, 0x24),    // 14%
            divider: rgba(0xFFFFFF, 0x1F),          // 12%
            // 深色下的控件描边同样用不透明色值。取"比按钮底亮一档"的
            // 中性蓝灰：深色界面的边框靠**提亮**表达（浅色靠压暗），
            // 这与 `bg_elevated` 在深色下比面板更亮是同一条规律。
            // 相对按钮底约 1.7:1，清晰但不刺眼。
            control_stroke: rgb(0x47525F),

            // 组 6：独立设计的深色横幅
            banner_error: BannerColors {
                bg: rgb(0x2A1416),
                border: rgb(0x7F2A2A),
                icon: rgb(0xFCA5A5),
                text: rgb(0xFECACA),
            },
            banner_info: BannerColors {
                bg: rgb(0x101E33),
                border: rgb(0x1E3A8A),
                icon: rgb(0x93C5FD),
                text: rgb(0xBFDBFE),
            },
            banner_success: BannerColors {
                bg: rgb(0x0D2318),
                border: rgb(0x166534),
                icon: rgb(0x6EE7B7),
                text: rgb(0xA7F3D0),
            },
            banner_warning: BannerColors {
                bg: rgb(0x2A1F0A),
                border: rgb(0x713F12),
                icon: rgb(0xFCD34D),
                text: rgb(0xFDE68A),
            },

            // 组 7：深色气泡＝深底浅字（与浅色主题方向相反）
            //
            // 同样不能用浅色版调暗——浅蓝 `#EFF6FF` 乘 0.2 得到的是一片
            // 近黑灰，丢失色相后用户分不清"这是我的消息还是助理的"。
            bubble_user_bg: rgb(0x16243A),
            bubble_user_border: rgb(0x2C4A73),
            bubble_assistant_bg: rgb(0x1A2029),
            bubble_assistant_border: rgb(0x2A323D),
            bubble_error_bg: rgb(0x2A1416),
            bubble_error_border: rgb(0x7F2A2A),
            bubble_error_text: rgb(0xFECACA),

            // 组 8
            chart_bg: rgb(0x0D1117),
            chart_grid: rgb(0x1F2937),

            // 组 9：深色导航三态——**本组是"边栏对比度"修复的核心**
            //
            // 改造前的三态直接沿用列表行的 `text_secondary` / `text_tertiary`
            // 与 `selected_overlay`，实测三处不足：
            //
            // | 状态   | 改造前                      | 实测对比度 |
            // |--------|-----------------------------|-----------|
            // | 未选中 | `text_tertiary` 图标        | 3.9:1（偏弱）|
            // | 选中   | `accent` 文字叠 16% 主色底  | **1.3:1**（不可读）|
            // | 悬停   | 6% 白叠加                   | 几乎不可见 |
            //
            // 选中态是最严重的一处：底与字同色系、明度接近，标题直接糊掉。
            // 因此这里把三态**整体重取**：文字提到接近正文的亮度，
            // 选中底换成"低明度深蓝"（而不是把主色叠加变亮），
            // 使文字与底形成约 5.7:1 的对比。
            //
            // 数值由 `dark_sidebar_options_meet_contrast_floor` 测试锁定，
            // 任何人把某一档调回去都会立即失败。
            sidebar_item_text: rgb(0xD6DDE6),
            sidebar_item_icon: rgb(0xAAB8C9),
            sidebar_item_hover: rgb(0x2B3542),
            sidebar_item_selected_bg: rgb(0x24405F),
            sidebar_item_selected_text: rgb(0x8FC2FF),
        }
    }

    /// 按主题取色板。
    pub const fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }

    /// 把色板中的玻璃填充替换为不透明实心底色。
    ///
    /// 供"关闭高级玻璃材质"时使用。**保留描边与投影**——关闭玻璃只应
    /// 去掉通透度，不应去掉容器的轮廓与层次，否则界面会退回改造前的
    /// 粗糙状态，用户会觉得"关了很难看"。
    pub const fn without_glass(mut self) -> Self {
        self.glass_fill_sidebar = self.bg_elevated;
        self.glass_fill_bar = self.bg_elevated;
        self.glass_fill_card = self.bg_elevated;
        self.glass_fill_button = self.bg_elevated;
        // 悬停底改用 `hover_overlay` 而不是 `bg_sunken`：后者是"输入槽"色，
        // 与实心的 `bg_elevated` 只差 1.13:1，关闭玻璃后按钮的悬停反馈
        // 会弱到几乎看不出来。反馈强度不该随玻璃开关而变。
        self.glass_fill_button_hover = self.hover_overlay;
        // 高光线在实心底上仍保留——它同样用于区分层次边界。
        self.glass_stroke_top = self.border_subtle;
        self
    }
}

// ---------------------------------------------------------------------------
// 应用级主题状态
// ---------------------------------------------------------------------------

/// 应用当前的视觉状态：主题模式 + 是否启用玻璃 + 对应色板。
///
/// 三者绑定成一个值，避免三处状态各自漂移（例如色板已改而 `glass` 标志未改）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiTheme {
    /// 当前主题模式。
    pub mode: ThemeMode,
    /// 是否启用高级玻璃质感。
    pub glass: bool,
    /// 当前色板。`glass == false` 时其玻璃填充已退化为不透明色。
    pub palette: Palette,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self::new(ThemeMode::default(), true)
    }
}

impl UiTheme {
    /// 构造。
    pub fn new(mode: ThemeMode, glass: bool) -> Self {
        let palette = Palette::for_mode(mode);
        let palette = if glass { palette } else { palette.without_glass() };
        Self {
            mode,
            glass,
            palette,
        }
    }

    /// 切换主题，返回新状态。
    pub fn toggled_mode(&self) -> Self {
        Self::new(self.mode.toggled(), self.glass)
    }

    /// 设置玻璃开关，返回新状态。
    pub fn with_glass(&self, glass: bool) -> Self {
        Self::new(self.mode, glass)
    }

    /// 取色板的只读引用。
    ///
    /// 产品代码目前统一走 `CompassApp::palette()`（它按值返回，让调用方
    /// 立刻脱离对 `self` 的借用，避免后续 `&mut app` 操作与借用冲突）。
    /// 本方法保留给**不需要 `CompassApp`** 的场景——例如离屏渲染测试
    /// 只持有 `UiTheme`；以及未来可能出现的、把主题独立于应用状态传入的
    /// 组件。
    #[allow(dead_code)]
    pub fn palette(&self) -> &Palette {
        &self.palette
    }
}

// ---------------------------------------------------------------------------
// 写入 egui 全局样式
// ---------------------------------------------------------------------------

/// 一次性装置明暗两套 `Visuals` 与间距体系。
///
/// 只需在启动时调用一次，以及**主题或玻璃开关变化时**重调。
/// 此后切换主题走 [`set_theme`] 即可，无需重装样式。
///
/// 同时配置两套的意义：用户点切换按钮时，目标主题的 `Visuals` 已经就绪，
/// 切换是纯指针操作，绝无可能显示"半新半旧"的中间态。
pub fn install(ctx: &Context, theme: &UiTheme) {
    // 间距与字号对两套都生效。
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(space::MD, space::SM);
        style.spacing.button_padding = egui::vec2(space::MD, 6.0);
        // 中文段落行距：默认 0 在汉字下显得拥挤。
        style.spacing.extra_text_line_spacing = 2.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 2.0;

        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(text::HEADING, Proportional)),
            (TextStyle::Body, FontId::new(text::BODY, Proportional)),
            (TextStyle::Monospace, FontId::new(text::MONO, egui::FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(text::BODY, Proportional)),
            (TextStyle::Small, FontId::new(text::CAPTION, Proportional)),
        ]
        .into();
    });

    // 明暗两套各自装色板。玻璃开关对两套同时生效。
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let pal = if theme.glass {
            Palette::for_mode(mode)
        } else {
            Palette::for_mode(mode).without_glass()
        };
        ctx.style_mut_of(mode.to_egui(), |style| {
            configure_visuals(&mut style.visuals, &pal, mode);
        });
    }
}

/// 切换当前生效的主题。
///
/// 这是"主题切换覆盖所有页面与组件"的核心杠杆：egui 的框架级组件
/// （面板、按钮、输入框、滚动条、工具提示、文本选区、光标）全部读取
/// 当前 `Style`，因此一次调用即可全局生效，不存在遗漏某个组件的可能。
pub fn set_theme(ctx: &Context, mode: ThemeMode) {
    ctx.set_theme(mode.to_egui());
}

/// 把一个色板写入 `Visuals`。
fn configure_visuals(v: &mut Visuals, p: &Palette, mode: ThemeMode) {
    let dark = mode.is_dark();

    v.dark_mode = dark;
    v.panel_fill = p.bg_panel;
    v.window_fill = p.bg_elevated;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: p.glass_shadow,
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: p.glass_shadow,
    };
    v.menu_corner_radius = CornerRadius::same(8);

    v.extreme_bg_color = p.bg_sunken;
    v.text_edit_bg_color = Some(p.bg_sunken);
    v.code_bg_color = p.bg_sunken;
    v.faint_bg_color = p.hover_overlay;

    v.override_text_color = Some(p.text_primary);
    v.weak_text_color = Some(p.text_secondary);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;

    v.selection.bg_fill = p.accent_subtle;
    v.selection.stroke = Stroke::new(1.0, p.text_primary);

    v.button_frame = true;
    v.striped = false;
    v.slider_trailing_fill = true;

    // ---- 控件状态 ----
    // 圆角 6：比玻璃容器的 10 明显更方，形成"容器更圆、控件更方"的层级差，
    // 避免所有元素圆角相同导致的空间关系模糊（按用户反馈从 8 调小）。
    let radius = CornerRadius::same(6);

    v.widgets.noninteractive.bg_fill = p.bg_panel;
    v.widgets.noninteractive.weak_bg_fill = p.bg_panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border_subtle);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.noninteractive.expansion = 0.0;

    // 按钮静止态：玻璃底 + **可见的**描边。
    //
    // `bg_stroke` 是本项目里"按钮边框"与"输入框边框"的**唯一**取色点：
    // `egui::Button` 经 `Style::interact` 取它，而 `egui::TextEdit` 在
    // 未获焦点时同样取 `visuals.widgets.inactive.bg_stroke` 作为外框描边
    // （见 `egui-0.36.2/src/widgets/text_edit/builder.rs` 的
    // `allocated.frame ... .stroke(visuals.bg_stroke)`）。
    //
    // 因此这一处改用 `control_stroke` 即可同时修好两类控件——不需要、
    // 也不应该在每个页面里手工给输入框套 `Frame`。
    v.widgets.inactive.bg_fill = p.glass_fill_button;
    v.widgets.inactive.weak_bg_fill = p.glass_fill_button;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.control_stroke);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.inactive.corner_radius = radius;
    v.widgets.inactive.expansion = 0.0;

    // 悬停态：加亮 + 主色描边。
    v.widgets.hovered.bg_fill = p.glass_fill_button_hover;
    v.widgets.hovered.weak_bg_fill = p.glass_fill_button_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.hovered.corner_radius = radius;
    v.widgets.hovered.expansion = 0.0;

    // 按下态：主色底 + 反色文字，反馈明确。
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.weak_bg_fill = p.accent;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text_on_accent);
    v.widgets.active.corner_radius = radius;
    v.widgets.active.expansion = 0.0;

    // 展开态（如已打开的下拉）。
    v.widgets.open.bg_fill = p.glass_fill_button_hover;
    v.widgets.open.weak_bg_fill = p.glass_fill_button_hover;
    v.widgets.open.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.text_primary);
    v.widgets.open.corner_radius = radius;
    v.widgets.open.expansion = 0.0;

    v.disabled_alpha = 0.45;
}

// ---------------------------------------------------------------------------
// 玻璃容器绘制
// ---------------------------------------------------------------------------

/// 玻璃表面的用途，决定使用哪一档玻璃底。
///
/// 五个取值并非都会被产品代码构造：侧边栏与顶栏在 `app.rs` 里直接自绘
/// 玻璃底（`Panel::show` 的闭包已占用内容区，再嵌 `Frame` 会被描边挤掉
/// 1px），因此 `Sidebar` / `Bar` 只由 [`crate::ui::widgets::glass_bar`]
/// 与渲染测试构造；按钮的两档由 `glass_icon_button` 内部用条件表达式
/// 直接取色，不经本枚举。
///
/// 保留完整枚举而不是删掉"没人构造"的取值，是因为这张表表达的是
/// **玻璃材质的语义分类**：配色（`Palette` 里的五档 `glass_fill_*`）
/// 与圆角规则都按它组织。删掉取值会让配色表失去可对照的语义索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum GlassSurface {
    /// 侧边栏。
    Sidebar,
    /// 顶栏。
    Bar,
    /// 内容卡片。
    Card,
    /// 按钮。
    Button,
    /// 按钮（悬停）。
    ButtonHover,
}

impl GlassSurface {
    /// 取该用途的填充色。
    fn fill(self, p: &Palette) -> Color32 {
        match self {
            Self::Sidebar => p.glass_fill_sidebar,
            Self::Bar => p.glass_fill_bar,
            Self::Card => p.glass_fill_card,
            Self::Button => p.glass_fill_button,
            Self::ButtonHover => p.glass_fill_button_hover,
        }
    }

    /// 取该用途的圆角。
    fn radius(self) -> CornerRadius {
        match self {
            Self::Card => CornerRadius::same(10),
            Self::Button => CornerRadius::same(18),
            Self::ButtonHover => CornerRadius::same(18),
            // 侧边栏与顶栏贴边，不需要圆角。
            Self::Sidebar => CornerRadius::ZERO,
            Self::Bar => CornerRadius::ZERO,
        }
    }
}

/// 装配一个玻璃容器的 `Frame`。
///
/// 四层叠加中的三层（底、描边、投影）由此提供；第四层"顶部高光线"
/// 无法用 `Frame` 表达，需在容器绘制后由 [`paint_glass_highlight`] 补画。
pub fn glass_frame(p: &Palette, surface: GlassSurface, inner_margin: i8) -> egui::Frame {
    egui::Frame::NONE
        .fill(surface.fill(p))
        .stroke(Stroke::new(1.0, p.glass_stroke))
        .corner_radius(surface.radius())
        .inner_margin(Margin::same(inner_margin))
        .shadow(Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: p.glass_shadow,
        })
}

/// 在已绘制容器的顶部补一条高光线。
///
/// **这是玻璃质感最关键的一层。** 只有半透明底 + 描边的方块看起来像
/// "没画完的色块"；加上顶部亮线后，才有"光从上方打在玻璃边缘"的观感，
/// 玻璃片的感觉才成立。
///
/// 高光线**不覆盖整个顶边**，而是左右各留出圆角半径的宽度——玻璃的
/// 高光不会延伸到圆角处，这是物理上的常识，也是视觉上"真"与"假"的分界。
pub fn paint_glass_highlight(ui: &egui::Ui, rect: egui::Rect, radius: f32, p: &Palette) {
    // 圆角容器的高光线需避开圆角弧段；直角容器（侧边栏/顶栏）不避让。
    let inset = if radius > 0.0 { radius } else { 0.0 };
    let left = rect.left() + inset;
    let right = rect.right() - inset;
    if right <= left {
        return;
    }
    let y = rect.top() + 0.5;
    ui.painter()
        .line_segment([egui::pos2(left, y), egui::pos2(right, y)], Stroke::new(1.0, p.glass_stroke_top));
}

// ---------------------------------------------------------------------------
// 自绘图标
// ---------------------------------------------------------------------------

/// 以矢量方式绘制图标。
///
/// `size` 为图标的逻辑边长，绘制在以 `center` 为中心、`size × size` 的
/// 正方形区域内。`color` 为线条/填充色。
///
/// `backdrop` 是图标**正下方**的实际填充色，仅 [`IconKind::Moon`] 使用——
/// 月牙靠"画一个背景色的圆盖住外圆一部分"实现（egui 无路径裁剪，
/// 这是唯一可行手段）。若这个颜色与真实底色不符，月牙的缺口会变成一块
/// 突兀的色斑。因此调用方必须传入自己刚画的那层填充色，**不能猜**。
///
/// **已核验的约束**：不依赖任何 emoji 字形（内嵌 SimHei 无 emoji 覆盖，
/// egui 自带 NotoEmoji 为彩色，与克制的界面调性冲突）。
pub fn icon(
    ui: &egui::Ui,
    center: egui::Pos2,
    size: f32,
    kind: IconKind,
    color: Color32,
    backdrop: Color32,
) {
    use egui::{pos2, Stroke};

    let painter = ui.painter();
    let half = size * 0.5;
    let stroke = Stroke::new((size / 9.0).max(1.0), color);

    match kind {
        // ---- 月亮：外圆填充 + 偏移圆挖空 ----
        // 用"底色圆覆盖"而非布尔差集——egui 无路径裁剪，这是唯一的
        // 可用手段。因两个圆都是实心，视觉上等价于月牙。
        IconKind::Moon => {
            painter.circle_filled(center, half * 0.92, color);
            // 偏移方向右上，形成经典月牙开口。
            let cut = pos2(center.x + half * 0.55, center.y - half * 0.45);
            // 必须用底板实际颜色：用透明色只是叠加（挖不空），
            // 用错颜色会留下可见色斑。
            painter.circle_filled(cut, half * 0.80, backdrop);
        }

        // ---- 太阳：中心实心圆 + 八条放射线 ----
        IconKind::Sun => {
            painter.circle_filled(center, half * 0.40, color);
            for i in 0..8 {
                let angle = std::f32::consts::TAU * (i as f32) / 8.0;
                let (s, c) = angle.sin_cos();
                let inner = pos2(center.x + c * half * 0.60, center.y + s * half * 0.60);
                let outer = pos2(center.x + c * half * 0.98, center.y + s * half * 0.98);
                painter.line_segment([inner, outer], stroke);
            }
        }

        // ---- 列表：三条横线 + 左侧圆点 ----
        IconKind::List => {
            for i in 0..3 {
                let y = center.y + (i as f32 - 1.0) * half * 0.50;
                painter.circle_filled(pos2(center.x - half * 0.72, y), half * 0.11, color);
                painter.line_segment(
                    [
                        pos2(center.x - half * 0.42, y),
                        pos2(center.x + half * 0.82, y),
                    ],
                    stroke,
                );
            }
        }

        // ---- 靶心：两个同心圆 + 中心点 ----
        IconKind::Target => {
            painter.circle_stroke(center, half * 0.90, stroke);
            painter.circle_stroke(center, half * 0.52, stroke);
            painter.circle_filled(center, half * 0.18, color);
        }

        // ---- 柱状图：三根不同高度的竖条 ----
        IconKind::Chart => {
            let base = center.y + half * 0.85;
            for (i, h) in [0.45f32, 0.85, 0.62].iter().enumerate() {
                let x = center.x + (i as f32 - 1.0) * half * 0.55;
                let top = base - size * h;
                painter.line_segment(
                    [pos2(x, base), pos2(x, top)],
                    Stroke::new(stroke.width * 1.9, color),
                );
            }
        }

        // ---- 对话气泡：圆角矩形 + 左下角尖角 ----
        IconKind::Chat => {
            let w = size * 0.92;
            let h = size * 0.72;
            let rect = egui::Rect::from_center_size(
                pos2(center.x, center.y - size * 0.06),
                egui::vec2(w, h),
            );
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Inside,
            );
            // 尾部尖角。
            let tail_x = rect.left() + w * 0.22;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(tail_x, rect.bottom()),
                    pos2(tail_x + w * 0.16, rect.bottom()),
                    pos2(tail_x - w * 0.06, rect.bottom() + h * 0.34),
                ],
                color,
                Stroke::NONE,
            ));
        }

        // ---- 齿轮：中心孔 + 八个齿 ----
        IconKind::Gear => {
            painter.circle_stroke(center, half * 0.46, stroke);
            painter.circle_stroke(center, half * 0.86, stroke);
            for i in 0..8 {
                let angle = std::f32::consts::TAU * (i as f32) / 8.0;
                let (s, c) = angle.sin_cos();
                let inner = pos2(center.x + c * half * 0.86, center.y + s * half * 0.86);
                let outer = pos2(center.x + c * half * 1.04, center.y + s * half * 1.04);
                painter.line_segment([inner, outer], Stroke::new(stroke.width * 1.7, color));
            }
        }

        // ---- 尖括号对：左右各两条折线 ----
        //
        // 两条折线各自用 `line_segment` 分两段画，而不是 `PathShape`：
        // 折线只有两个拐点，两段线段足以表达，且线段图元更容易在
        // 离屏测试里按"线宽 + 颜色"断言（见 `code_icon_draws_two_chevrons`）。
        IconKind::Code => {
            // 线宽比通用档略粗：品牌标识是界面上唯一"图形即标题"的位置，
            // 需要比列表图标更结实的分量才压得住右侧 18px 的字标。
            let brand_stroke = Stroke::new(stroke.width * 1.25, color);
            // 每个尖括号的横向跨度与顶点位置分开给：跨度决定"斜边的斜度"，
            // 顶点位置决定两个括号之间的**间隙**。若两者用同一个量
            // （顶点取 ±span、斜边回折 span），两个括号会在中心处首尾相接，
            // 退化成 `<>` 一个菱形，丢失"成对括号"的语义。
            let arm = half * 0.52;
            let dy = half * 0.62;
            for dir in [-1.0f32, 1.0] {
                let tip = pos2(center.x + dir * half * 0.72, center.y);
                let back_x = tip.x - dir * arm;
                painter.line_segment([pos2(back_x, tip.y - dy), tip], brand_stroke);
                painter.line_segment([tip, pos2(back_x, tip.y + dy)], brand_stroke);
            }
        }
    }
}

/// 图标所需的推荐尺寸（供布局预留空间）。
pub const ICON_SIZE: f32 = 18.0;

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // 主题二值切换
    // -----------------------------------------------------------------------

    /// 需求指定：浅色显示月亮（点击转深色），深色显示太阳（点击转浅色）。
    #[test]
    fn toggle_icon_matches_the_requested_convention() {
        assert_eq!(
            ThemeMode::Light.toggle_icon(),
            IconKind::Moon,
            "浅色主题下应显示月亮"
        );
        assert_eq!(
            ThemeMode::Dark.toggle_icon(),
            IconKind::Sun,
            "深色主题下应显示太阳"
        );
    }

    /// 切换两次必须回到原状态——单按钮循环的基本正确性。
    #[test]
    fn toggling_twice_returns_to_the_original_mode() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            assert_eq!(mode.toggled().toggled(), mode);
        }
    }

    /// 切换按钮的图标必须随主题而**变化**，否则用户无法判断当前状态。
    #[test]
    fn toggle_icon_differs_between_themes() {
        assert_ne!(
            ThemeMode::Light.toggle_icon(),
            ThemeMode::Dark.toggle_icon(),
            "两种主题下的图标必须不同"
        );
    }

    #[test]
    fn default_theme_is_dark() {
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
    }

    // -----------------------------------------------------------------------
    // 色板完整性
    // -----------------------------------------------------------------------

    /// 两套色板的文字色必须与各自底色**明暗方向相反**。
    ///
    /// 这是最基本的可读性前提：浅色主题下文字必须比背景暗，
    /// 深色主题下必须比背景亮。若哪天有人改错一个值，这条会立刻失败。
    #[test]
    fn text_contrasts_with_background_in_both_themes() {
        let luma = |c: Color32| c.r() as i32 + c.g() as i32 + c.b() as i32;

        let light = Palette::light();
        assert!(
            luma(light.text_primary) < luma(light.bg_panel),
            "浅色主题下正文色必须比背景暗"
        );

        let dark = Palette::dark();
        assert!(
            luma(dark.text_primary) > luma(dark.bg_panel),
            "深色主题下正文色必须比背景亮"
        );
    }

    /// 文字三档必须逐级变淡，否则层级无法体现。
    #[test]
    fn text_levels_are_monotonic_in_both_themes() {
        let luma = |c: Color32| c.r() as i32 + c.g() as i32 + c.b() as i32;

        // 浅色主题：越次要越浅（luma 越大）。
        let l = Palette::light();
        assert!(luma(l.text_primary) < luma(l.text_secondary));
        assert!(luma(l.text_secondary) < luma(l.text_tertiary));

        // 深色主题：越次要越暗（luma 越小）。
        let d = Palette::dark();
        assert!(luma(d.text_primary) > luma(d.text_secondary));
        assert!(luma(d.text_secondary) > luma(d.text_tertiary));
    }

    /// 语义色必须互相可区分——用户要靠它们分辨难度与状态。
    #[test]
    fn semantic_colors_are_mutually_distinct() {
        for p in [Palette::light(), Palette::dark()] {
            let colors = [p.success, p.warning, p.danger, p.neutral];
            for i in 0..colors.len() {
                for j in (i + 1)..colors.len() {
                    assert_ne!(
                        colors[i], colors[j],
                        "语义色必须可区分，否则用户无法快速识别"
                    );
                }
            }
        }
    }

    /// 深色横幅必须是**独立设计**的，而非浅色版压暗。
    ///
    /// 若直接把浅色底乘一个系数变暗，会得到几乎无色的灰，用户无法一眼
    /// 分辨错误与提示。这条断言深色横幅底保留了可辨识的色相差异。
    #[test]
    fn dark_banners_are_independently_designed_not_dimmed_light_ones() {
        let l = Palette::light();
        let d = Palette::dark();

        for (name, lb, db) in [
            ("error", l.banner_error, d.banner_error),
            ("info", l.banner_info, d.banner_info),
            ("success", l.banner_success, d.banner_success),
            ("warning", l.banner_warning, d.banner_warning),
        ] {
            assert_ne!(lb.bg, db.bg, "{name} 横幅的明暗底色不应相同");

            // 深色横幅底必须"暗"：三通道之和远小于浅色版。
            let luma = |c: Color32| c.r() as i32 + c.g() as i32 + c.b() as i32;
            assert!(
                luma(db.bg) * 3 < luma(lb.bg),
                "{name} 横幅的深色底应显著更暗（{luma:?}）",
                luma = luma(db.bg)
            );

            // 且底色不应退化为中性灰——保留色相才有辨识度。
            let spread = |c: Color32| {
                let (r, g, b) = (c.r() as i32, c.g() as i32, c.b() as i32);
                (r - g).abs() + (g - b).abs() + (r - b).abs()
            };
            assert!(
                spread(db.bg) > 4,
                "{name} 横幅的深色底丢失了色相（接近中性灰）"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 对比度：把"看得清"从主观判断变成可执行断言
    // -----------------------------------------------------------------------
    //
    // 下面三条测试对应本次三个修复诉求中的两个（边框可见、边栏对比度）。
    //
    // 为什么必须量化：这两个问题的共同特征是**编译、测试、clippy 全通过，
    // 只有真机打开界面才暴露**。"边框看不见"不会让任何断言失败，
    // "选中项文字糊在背景里"也不会——它们只是"难看"。因此这里把
    // 设计意图写成对比度下限，让回归在 `cargo test` 阶段就被拦下。

    /// 按 egui 的实际混合式把前景合成到背景上。
    ///
    /// **不能用 `Color32` 的通道值直接当"源色"**：`Color32` 按预乘 alpha
    /// 存储，而本模块的 `rgba()` 刻意写入未预乘的通道值（见其文档）。
    /// 因此这里显式按 GPU 的 `ONE / ONE_MINUS_SRC_ALPHA` 混合式计算，
    /// 得到的结果才是屏幕上真正显示的颜色。
    fn over_rgb(fg: Color32, bg: (f32, f32, f32)) -> (f32, f32, f32) {
        let a = fg.a() as f32 / 255.0;
        (
            fg.r() as f32 + bg.0 * (1.0 - a),
            fg.g() as f32 + bg.1 * (1.0 - a),
            fg.b() as f32 + bg.2 * (1.0 - a),
        )
    }

    /// 合成到**不透明**背景上的便捷写法。
    ///
    /// 背景本身半透明时不能用它（例如侧边栏底），要先自己用 [`over_rgb`]
    /// 把它合成到窗口底上，再把结果当作这里的"已合成背景"。
    fn over(fg: Color32, bg: Color32) -> (f32, f32, f32) {
        over_rgb(fg, (bg.r() as f32, bg.g() as f32, bg.b() as f32))
    }

    /// sRGB 相对亮度（WCAG 定义）。
    fn rel_luma(rgb: (f32, f32, f32)) -> f32 {
        fn channel(v: f32) -> f32 {
            let v = (v / 255.0).clamp(0.0, 1.0);
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2)
    }

    /// WCAG 对比度，取值 1.0（同色）～21.0（黑白）。
    fn contrast(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
        let (la, lb) = (rel_luma(a), rel_luma(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 浅色主题下，**按钮与输入框的描边必须看得见**。
    ///
    /// 改造前这两类控件复用容器的 `glass_stroke`（浅色下仅 9% 不透明度），
    /// 叠在纯白卡片上后与底色几乎完全一致——界面上出现了一片"没有边界的
    /// 文字"，用户只能靠位置猜测哪里可以点、哪里可以输入。
    ///
    /// 断言下限取 1.6:1。这个值刻意低于 WCAG 对"必要图形边界"要求的 3:1，
    /// 因为控件同时还有独立的底色（`glass_fill_button` / `bg_sunken`），
    /// 边框只是**强化**而非**唯一**的边界线索；但它必须显著高于
    /// 改造前的 1.05:1，否则"加了边框"在屏幕上等于没加。
    #[test]
    fn light_control_borders_are_visible() {
        let l = Palette::light();

        for (name, surface) in [("按钮底（纯白）", l.bg_panel), ("输入槽底", l.bg_sunken)] {
            let border = over(l.control_stroke, surface);
            let ratio = contrast(border, over(surface, surface));
            assert!(
                ratio >= 1.6,
                "浅色主题下 {name} 的控件描边对比度仅 {ratio:.2}:1（应 ≥ 1.6:1），\
                 用户在屏幕上分辨不出控件边界"
            );
        }

        // 反向断言：控件描边必须**显著强于**容器轮廓。
        // 若哪天有人把 `control_stroke` 改回 `glass_stroke` 的值，这条会失败
        // ——它守的是"两个令牌职责不同"这件事本身，而不只是某个具体色值。
        let control = rel_luma(over(l.control_stroke, l.bg_panel));
        let glass = rel_luma(over(l.glass_stroke, l.bg_panel));
        assert_ne!(l.control_stroke, l.glass_stroke);
        assert!(
            control < glass * 0.75,
            "控件描边必须明显比容器轮廓更实（control luma {control:.3} vs glass {glass:.3}）"
        );
    }

    /// 浅色主题下，控件描边必须真的**接线**到 egui 的控件样式。
    ///
    /// 只断言"色板里有 `control_stroke`"是不够的：真正的失效方式是
    /// **令牌定义了却没接上**（`widgets.inactive.bg_stroke` 仍指向
    /// `glass_stroke`）。那时色板测试全绿，界面依旧没有边框。
    ///
    /// `widgets.inactive.bg_stroke` 是本项目"按钮边框 + 输入框边框"的
    /// 唯一取色点：`egui::Button` 经 `Style::interact` 取它，
    /// `egui::TextEdit` 未获焦点时同样以它作外框描边。
    #[test]
    fn light_widget_stroke_is_wired_to_control_stroke() {
        let ctx = Context::default();
        install(&ctx, &UiTheme::new(ThemeMode::Light, true));

        let style = ctx.style_of(egui::Theme::Light);
        let stroke = style.visuals.widgets.inactive.bg_stroke;

        assert_eq!(
            stroke.color,
            Palette::light().control_stroke,
            "浅色主题下按钮/输入框的描边没有接到 `control_stroke`"
        );
        assert_ne!(
            stroke.color,
            Palette::light().glass_stroke,
            "控件描边仍等于容器轮廓色——浅色下会淡到看不见"
        );
        assert!(stroke.width > 0.0, "边框宽度必须为正，否则等于没有边框");
    }

    /// **深色主题下侧边栏导航项的三态对比度下限。**
    ///
    /// 这是本次"边栏对比度"修复的判定依据。改造前的实测值：
    ///
    /// | 状态 | 改造前 | 对比度 |
    /// |---|---|---|
    /// | 未选中文字 | `text_secondary` | 7.6:1（尚可） |
    /// | 未选中图标 | `text_tertiary` | 3.9:1（偏弱） |
    /// | **选中文字** | `accent` 叠 16% 主色底 | **1.3:1（不可读）** |
    ///
    /// 选中态那一栏是必须修掉的：底与字同色系、明度接近，当前页标题
    /// 直接糊在背景里。改造后三态全部提到 4.5:1 以上。
    #[test]
    fn dark_sidebar_options_meet_contrast_floor() {
        let p = Palette::dark();
        // 侧边栏底是半透明的，必须先合成到窗口底上才是屏幕上的颜色。
        let base = over(p.glass_fill_sidebar, p.bg_base);

        // 文字下限取 10:1 而非 WCAG 的正文线 7:1。
        //
        // 依据是**实测值必须被拦下**：改造前该位置用的是 `text_secondary`
        // （`#A3ADBB`），实测 7.6:1——按 AA 标准"合格"，但它让主导航
        // 读起来像"次要说明"，这正是用户反馈"边栏对比度不足"的来源。
        // 阈值若停在 7.0，把颜色改回旧值测试照样通过，等于没守。
        let text = contrast(over_rgb(p.sidebar_item_text, base), base);
        assert!(
            text >= 10.0,
            "深色侧边栏未选中文字对比度仅 {text:.2}:1（应 ≥ 10:1）——\
             导航项是主导航，不能按次要说明的强度着色"
        );

        let icon = contrast(over_rgb(p.sidebar_item_icon, base), base);
        assert!(
            icon >= 4.5,
            "深色侧边栏未选中图标对比度仅 {icon:.2}:1（应 ≥ 4.5:1）"
        );

        // 选中项的文字落在**选中底**上，而不是侧边栏底上——
        // 这正是改造前算错的地方：底变亮了，却仍按侧边栏底评估可读性。
        let sel_base = over_rgb(p.sidebar_item_selected_bg, base);
        let sel = contrast(over_rgb(p.sidebar_item_selected_text, sel_base), sel_base);
        assert!(
            sel >= 4.5,
            "深色侧边栏选中文字对比度仅 {sel:.2}:1（应 ≥ 4.5:1）——\
             文字与选中底同色系时会糊成一片"
        );

        // 悬停与选中底必须**与静止底可区分**，否则三态形同虚设。
        let hover = contrast(over_rgb(p.sidebar_item_hover, base), base);
        assert!(
            hover >= 1.2,
            "深色侧边栏悬停底与静止底仅差 {hover:.2}:1，用户看不出鼠标在哪一项"
        );
        let sel_vs_base = contrast(sel_base, base);
        assert!(
            sel_vs_base >= 1.4,
            "深色侧边栏选中底与静止底仅差 {sel_vs_base:.2}:1，当前页不够醒目"
        );
    }

    /// 浅色主题的侧边栏三态同样要过对比度下限。
    ///
    /// 与深色版**分开写**而不是循环两套主题：两套的阈值不同
    /// （浅色底接近纯白，悬停/选中底的可用对比度天然更低），
    /// 而且失败信息必须能直接指出是哪一套主题的哪一态不达标。
    #[test]
    fn light_sidebar_options_meet_contrast_floor() {
        let p = Palette::light();
        let base = over(p.glass_fill_sidebar, p.bg_base);

        // 浅色下限 9:1，同样按"旧值必须被拦下"定：旧值 `text_secondary`
        // （`#4B5563`）在白底上是 7.6:1。
        let text = contrast(over_rgb(p.sidebar_item_text, base), base);
        assert!(
            text >= 9.0,
            "浅色侧边栏未选中文字对比度仅 {text:.2}:1（应 ≥ 9:1）"
        );

        let icon = contrast(over_rgb(p.sidebar_item_icon, base), base);
        assert!(
            icon >= 4.5,
            "浅色侧边栏未选中图标对比度仅 {icon:.2}:1（应 ≥ 4.5:1）"
        );

        let sel_base = over_rgb(p.sidebar_item_selected_bg, base);
        let sel = contrast(over_rgb(p.sidebar_item_selected_text, sel_base), sel_base);
        assert!(
            sel >= 4.5,
            "浅色侧边栏选中文字对比度仅 {sel:.2}:1（应 ≥ 4.5:1）"
        );

        let hover = contrast(over_rgb(p.sidebar_item_hover, base), base);
        assert!(
            hover >= 1.2,
            "浅色侧边栏悬停底与静止底仅差 {hover:.2}:1"
        );
        let sel_vs_base = contrast(sel_base, base);
        assert!(
            sel_vs_base >= 1.2,
            "浅色侧边栏选中底与静止底仅差 {sel_vs_base:.2}:1"
        );
    }

    /// **悬停反馈必须在两套主题下都看得见。**
    ///
    /// 本条对应用户报告的第一个缺陷："浅色模式下，鼠标移动到选项上没有光效"。
    ///
    /// 根因是 `hover_overlay` 原为 4% 的深色叠加，按 egui 的加性混合
    /// 叠在近白行底上被冲成**纯白**，与行底逐字节相同——不是"太淡"，
    /// 而是**完全没有差别**。
    ///
    /// 验收对象有两个，缺一不可：行底（`bg_elevated`）与斑马纹底
    /// （`bg_sunken`）。只测前者会漏掉"悬停落在斑马纹行上看不出来"。
    #[test]
    fn row_hover_is_perceptible_in_both_themes() {
        for (name, p) in [("浅色", Palette::light()), ("深色", Palette::dark())] {
            let hover = over(p.hover_overlay, p.bg_elevated);

            let vs_row = contrast(hover, over(p.bg_elevated, p.bg_elevated));
            assert!(
                vs_row >= 1.2,
                "{name}主题下悬停底与行底仅差 {vs_row:.2}:1，鼠标移上去看不出变化"
            );

            let vs_zebra = contrast(hover, over(p.bg_sunken, p.bg_sunken));
            assert!(
                vs_zebra >= 1.2,
                "{name}主题下悬停底与斑马纹底仅差 {vs_zebra:.2}:1，\
                 悬停落在奇数行上会看不出来"
            );
        }
    }

    /// **深色下悬停的行必须仍然读得出标题——本轮最严重的一条。**
    ///
    /// 对应用户报告的第二个缺陷："深色模式下，鼠标移到题库选项上看不清题目名称"。
    ///
    /// 根因：深色 `hover_overlay` 原为 6% 白叠加，加性混合后把整行冲成
    /// **纯白**；标题用 `text_primary`（`#E6EAF0`），落在纯白上对比度
    /// 只有 **1.08:1**——不是"有点淡"，是彻底看不见。
    ///
    /// 评估对象必须是**悬停后的底色**，而不是行底：这正是原实现算错的地方。
    #[test]
    fn dark_hovered_row_keeps_titles_readable() {
        let p = Palette::dark();
        let hovered = over(p.hover_overlay, p.bg_elevated);

        // 标题用 `text_primary`（见 `home::draw_row` 的 TITLE 列）。
        let title = contrast(over_rgb(p.text_primary, hovered), hovered);
        assert!(
            title >= 7.0,
            "深色下悬停行的标题对比度仅 {title:.2}:1（应 ≥ 7:1）——\
             悬停底把整行冲白时标题会完全消失"
        );

        // 题号用 `text_tertiary`，是同一行里最淡的文字，也要保住。
        let id = contrast(over_rgb(p.text_tertiary, hovered), hovered);
        assert!(
            id >= 3.0,
            "深色下悬停行的题号对比度仅 {id:.2}:1（应 ≥ 3:1）"
        );

        // 反向断言：悬停底不得是（接近）白色——那正是把文字压掉的原因。
        assert!(
            rel_luma(hovered) < 0.2,
            "深色下悬停底太亮（luma {:.3}），会把浅色文字压掉",
            rel_luma(hovered)
        );
    }

    /// 按钮的悬停态必须与静止态**可区分**。
    ///
    /// 这是"浅色下悬停没有光效"的另一半成因：静止态与悬停态都是白色系
    /// 半透明，叠在近白卡片上后**合成结果逐字节相同**。
    /// 该测试覆盖的是这两个状态的实际合成值之差，而不是色板里的原始值
    /// 之差——后者即使不同，也可能因为合成而变得相同。
    #[test]
    fn button_hover_differs_from_rest_state() {
        for (name, p) in [("浅色", Palette::light()), ("深色", Palette::dark())] {
            let rest = over(p.glass_fill_button, p.bg_elevated);
            let hover = over(p.glass_fill_button_hover, p.bg_elevated);

            let ratio = contrast(hover, rest);
            assert!(
                ratio >= 1.2,
                "{name}主题下按钮悬停底与静止底仅差 {ratio:.2}:1——\
                 两个状态合成后可能相同，鼠标移上去没有反馈"
            );
        }
    }

    /// 关闭玻璃质感后，按钮的悬停反馈**不得**跟着消失。
    ///
    /// `without_glass()` 会把按钮底换成不透明的 `bg_elevated`；若悬停底
    /// 顺手换成"同样是实心底但只差一点"的 `bg_sunken`，浅色下两者只差
    /// 1.11:1——用户在设置里关掉玻璃后会发现"按钮点上去没反应了"。
    /// 反馈强度不该随一个视觉风格开关而变。
    #[test]
    fn button_hover_survives_disabling_glass() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let p = Palette::for_mode(mode).without_glass();
            let rest = over(p.glass_fill_button, p.bg_elevated);
            let hover = over(p.glass_fill_button_hover, p.bg_elevated);

            let ratio = contrast(hover, rest);
            assert!(
                ratio >= 1.15,
                "关闭玻璃后 {mode:?} 主题下按钮悬停仅差 {ratio:.2}:1"
            );
        }
    }

    /// 文本选区高亮必须看得见。
    ///
    /// 与上面两条同源（`accent_subtle` 是 12%/20% 的半透明叠加），
    /// 只是它影响的是"选中的文字有没有高亮"，而不是悬停。
    #[test]
    fn text_selection_highlight_is_visible() {
        for (name, p) in [("浅色", Palette::light()), ("深色", Palette::dark())] {
            let sel = over(p.accent_subtle, p.bg_panel);
            let ratio = contrast(sel, over(p.bg_panel, p.bg_panel));
            assert!(
                ratio >= 1.2,
                "{name}主题下文本选区高亮与底仅差 {ratio:.2}:1，\
                 选中文字看不出被选中"
            );

            // 选中态下的文字仍要可读。
            let text = contrast(over_rgb(p.text_primary, sel), sel);
            assert!(
                text >= 4.5,
                "{name}主题下选区中的文字对比度仅 {text:.2}:1（应 ≥ 4.5:1）"
            );
        }
    }

    /// 导航三态令牌必须随主题变化。
    ///
    /// 与 `semantic_colors_follow_the_theme` 同理：某个令牌若在两套主题下
    /// 取值相同，说明它没跟着主题走，必然在其中一套下不可读。
    #[test]
    fn sidebar_tokens_follow_the_theme() {
        let l = Palette::light();
        let d = Palette::dark();

        for (name, lv, dv) in [
            ("文字", l.sidebar_item_text, d.sidebar_item_text),
            ("图标", l.sidebar_item_icon, d.sidebar_item_icon),
            ("悬停底", l.sidebar_item_hover, d.sidebar_item_hover),
            (
                "选中底",
                l.sidebar_item_selected_bg,
                d.sidebar_item_selected_bg,
            ),
            (
                "选中文字",
                l.sidebar_item_selected_text,
                d.sidebar_item_selected_text,
            ),
        ] {
            assert_ne!(lv, dv, "侧边栏导航「{name}」色未随主题变化");
        }
    }

    // -----------------------------------------------------------------------
    // 玻璃开关
    // -----------------------------------------------------------------------

    /// 关闭玻璃后，容器底必须**完全不透明**——否则等于没关。
    #[test]
    fn disabling_glass_makes_surfaces_opaque() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let t = UiTheme::new(mode, false);
            let p = t.palette();

            for (name, c) in [
                ("sidebar", p.glass_fill_sidebar),
                ("bar", p.glass_fill_bar),
                ("card", p.glass_fill_card),
                ("button", p.glass_fill_button),
            ] {
                assert_eq!(c.a(), 255, "{name} 在关闭玻璃后必须完全不透明");
            }
        }
    }

    /// 关闭玻璃**不得移除描边**。
    ///
    /// 这是刻意的设计决定：关闭玻璃只去掉通透度，保留轮廓与层次。
    /// 若连描边一起去掉，界面会退回改造前的粗糙状态。
    #[test]
    fn disabling_glass_keeps_borders_and_layering() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let with = Palette::for_mode(mode);
            let without = Palette::for_mode(mode).without_glass();

            assert_eq!(
                with.glass_stroke, without.glass_stroke,
                "关闭玻璃不应改变描边色，容器仍需有清晰边界"
            );
            // 实心底必须与内容区背景可区分，否则容器会"消失"。
            assert_ne!(
                without.glass_fill_card, without.bg_panel,
                "关闭玻璃后卡片底仍须与背景可区分"
            );
            assert_ne!(
                without.glass_fill_sidebar, without.bg_panel,
                "关闭玻璃后侧边栏底仍须与背景可区分"
            );
        }
    }

    /// 开启玻璃时容器底必须**半透明**，否则玻璃效果无从谈起。
    #[test]
    fn enabling_glass_makes_surfaces_translucent() {
        let p = Palette::dark();
        assert!(
            p.glass_fill_sidebar.a() < 255,
            "启用玻璃时侧边栏底必须半透明"
        );
        assert!(p.glass_fill_card.a() < 255, "启用玻璃时卡片底必须半透明");
        // 但也不能过透——过度透明会让文字落在不可控的背景上。
        assert!(
            p.glass_fill_sidebar.a() > 100,
            "玻璃底不能过透，否则文字可读性无法保证"
        );
    }

    // -----------------------------------------------------------------------
    // 状态构造
    // -----------------------------------------------------------------------

    /// 两个维度（主题、玻璃）相互正交，四种组合都能构造。
    #[test]
    fn theme_and_glass_are_independent_dimensions() {
        let combos = [
            (ThemeMode::Dark, true),
            (ThemeMode::Dark, false),
            (ThemeMode::Light, true),
            (ThemeMode::Light, false),
        ];
        for (mode, glass) in combos {
            let t = UiTheme::new(mode, glass);
            assert_eq!(t.mode, mode);
            assert_eq!(t.glass, glass);
        }

        // 切换主题不改变玻璃开关。
        let t = UiTheme::new(ThemeMode::Dark, false);
        assert!(!t.toggled_mode().glass, "切主题不应影响玻璃开关");

        // 切玻璃开关不改变主题。
        let t = UiTheme::new(ThemeMode::Light, true);
        assert_eq!(t.with_glass(false).mode, ThemeMode::Light);
    }

    /// 切换主题后色板必须真正换成另一套。
    #[test]
    fn toggling_mode_switches_the_palette() {
        let dark = UiTheme::new(ThemeMode::Dark, true);
        let light = dark.toggled_mode();
        assert_ne!(
            dark.palette().text_primary, light.palette().text_primary,
            "切换主题必须换用另一套色值"
        );
        assert_eq!(light.palette().text_primary, Palette::light().text_primary);
    }

    // -----------------------------------------------------------------------
    // 图标
    // -----------------------------------------------------------------------

    /// 每个图标路径都必须可绘制且不 panic。
    ///
    /// 自绘图标的坐标全是算术推导，一处符号写反就可能画出屏幕外的形状
    /// 或触发断言。这条用真实的离屏 Context 走一遍全部图标。
    #[test]
    fn all_icons_render_without_panicking() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::Area::new(egui::Id::new("icon_test")).show(ctx, |ui| {
                for kind in [
                    IconKind::Moon,
                    IconKind::Sun,
                    IconKind::List,
                    IconKind::Target,
                    IconKind::Chart,
                    IconKind::Chat,
                    IconKind::Gear,
                    IconKind::Code,
                ] {
                    // 两套底板色各画一遍：月亮对底板色敏感，用错会露色斑。
                    for backdrop in [Palette::dark().bg_panel, Palette::light().glass_fill_sidebar] {
                        icon(
                            ui,
                            egui::pos2(20.0, 20.0),
                            ICON_SIZE,
                            kind,
                            Color32::WHITE,
                            backdrop,
                        );
                    }
                }
            });
        });
        // 纹理增量必须消费掉，否则 Context 析构时 panic。
        out.textures_delta.clear();
    }

    /// 在离屏 Context 中绘制一个图标，返回帧内全部**实心圆**的 `(填充色, 半径)`。
    ///
    /// 直接检查图元数据，而不是比较序列化字符串：`Color32` 的 `Debug` 输出
    /// 不含具体通道值（只显示类似 `Color32(18, 22, 28, 255)` 的元组），也难以
    /// 做稳定比较。遍历 `epaint::ClippedShape` 逐一取出 `CircleShape::fill`
    /// 才是可靠的判定方式。
    fn filled_circles_of(kind: IconKind, color: Color32, backdrop: Color32) -> Vec<(Color32, f32)> {
        use egui::Shape;

        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            icon(ui, egui::pos2(40.0, 40.0), 32.0, kind, color, backdrop);
        });
        // 纹理增量必须消费掉，否则 Context 析构时 panic。
        out.textures_delta.clear();

        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Circle(c) => Some((c.fill, c.radius)),
                _ => None,
            })
            .collect()
    }

    /// **本组测试对应一个真实的视觉缺陷。**
    ///
    /// 月牙的缺口是靠"画一个底板色的圆盖住外圆"实现的（egui 无路径裁剪）。
    /// 改造期间该函数曾自行取 `ui.visuals().panel_fill` 作底板色，但主题
    /// 切换按钮位于**玻璃面板**上——玻璃底色与 `panel_fill` 不同色，于是
    /// 月牙缺口处露出一块突兀的色斑。
    ///
    /// 修复方式是让 [`icon`] 接收显式 `backdrop`，迫使调用方传入自己刚画的
    /// 那层填充色。这条测试守住"底板色确实被用上了"：挖空圆的填充色必须
    /// **逐字节等于**调用方传入的 `backdrop`，换成另一套底板色就必须跟着变。
    #[test]
    fn moon_cutout_uses_the_supplied_backdrop() {
        // 月亮色必须与两套底板色都不同，否则"哪个圆是挖空"无法区分——
        // 浅色主题的 `bg_panel` 就是纯白 #FFFFFF，用白色画月亮会让
        // 本体圆与浅色底板色撞在一起。
        let moon = Palette::dark().danger;
        let dark_backdrop = Palette::dark().bg_panel;
        let light_backdrop = Palette::light().bg_panel;

        assert_ne!(moon, dark_backdrop);
        assert_ne!(moon, light_backdrop);
        assert_ne!(
            dark_backdrop, light_backdrop,
            "两套底板色必须不同，否则这条测试失去判别力"
        );

        let on_dark = filled_circles_of(IconKind::Moon, moon, dark_backdrop);
        let on_light = filled_circles_of(IconKind::Moon, moon, light_backdrop);

        // 月亮固定由两个实心圆组成：本体 + 挖空。
        assert_eq!(
            on_dark.len(),
            2,
            "月亮必须由\"本体圆 + 挖空圆\"两个实心圆构成，实际 {} 个",
            on_dark.len()
        );

        let has_fill = |circles: &[(Color32, f32)], target: Color32| {
            circles.iter().any(|(fill, _)| *fill == target)
        };

        assert!(
            has_fill(&on_dark, dark_backdrop),
            "深色底板下不存在填充色等于 backdrop 的圆——\
             挖空圆没画，或用了硬编码的底板色，月亮会在玻璃面板上露出色斑"
        );
        assert!(
            has_fill(&on_light, light_backdrop),
            "浅色底板下不存在填充色等于 backdrop 的圆——同上"
        );
        // 反向检查：底板色必须**跟着参数变**。若 `backdrop` 被忽略而
        // 固定用某一套主题的底色，这两条会有一条失败。
        assert!(
            !has_fill(&on_dark, light_backdrop),
            "深色底板下的挖空圆用了浅色底板色——`backdrop` 参数被忽略"
        );
        assert!(
            !has_fill(&on_light, dark_backdrop),
            "浅色底板下的挖空圆用了深色底板色——`backdrop` 参数被忽略"
        );
    }

    /// 月牙的缺口不得与月亮本体同色，否则画出来是一个实心圆。
    ///
    /// 与上一条互补：上一条验证"用的是传入的色"，这条验证"确实存在一个
    /// 与本体异色的圆"。两条一起才能排除"两个圆都画成月亮色"这种退化。
    #[test]
    fn moon_is_not_a_solid_circle() {
        let moon = Palette::dark().danger;
        let backdrop = Palette::dark().bg_panel;
        assert_ne!(moon, backdrop);

        let circles = filled_circles_of(IconKind::Moon, moon, backdrop);

        assert_eq!(
            circles.len(),
            2,
            "月亮必须由两个实心圆构成；只有一个说明挖空圆没画，\
             渲染结果是实心圆而非月牙"
        );

        let body = circles
            .iter()
            .find(|(fill, _)| *fill == moon)
            .expect("必须存在填充色为月亮色的本体圆");
        let cutout = circles
            .iter()
            .find(|(fill, _)| *fill == backdrop)
            .expect("必须存在填充色为底板色的挖空圆");

        assert_ne!(body.0, cutout.0, "挖空圆与本体圆同色——视觉上仍是实心圆");
        assert!(
            cutout.1 > 0.0 && cutout.1 < body.1,
            "挖空圆半径 {} 必须介于 0 与本体半径 {} 之间\
             （过大吃掉整个月亮，过小则缺口不可见）",
            cutout.1,
            body.1
        );
    }

    /// 品牌字形必须真的画出**成对且分离**的尖括号。
    ///
    /// 这条测试存在的理由：`IconKind::Code` 的全部坐标都是算术推导，
    /// 把"顶点位置"与"斜边跨度"写成同一个量时，两个尖括号会在中心处
    /// 首尾相接，退化成一个 `<>` 菱形——**画得出来、不 panic、
    /// 肉眼看只是"有点怪"**，正是 §8.3 说的那类测试盲区。
    /// 因此这里直接断言图元：4 条线段 + 中心留有间隙。
    #[test]
    fn code_icon_draws_two_separated_chevrons() {
        use egui::Shape;

        let color = Palette::dark().accent;
        let center_x = 40.0f32;

        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            icon(
                ui,
                egui::pos2(center_x, 40.0),
                32.0,
                IconKind::Code,
                color,
                Palette::dark().bg_panel,
            );
        });
        out.textures_delta.clear();

        let segments: Vec<[egui::Pos2; 2]> = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::LineSegment { points, stroke } => {
                    // 正向断言：字形必须用调用方传入的颜色。
                    assert_eq!(stroke.color, color, "字形未使用传入的颜色");
                    Some(*points)
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            segments.len(),
            4,
            "尖括号对必须由 4 条线段构成（2 个括号 × 2 段），实得 {}",
            segments.len()
        );

        // ---- 按左右分组 ----
        //
        // **不能只比较"两侧最靠近中线的点"**：退化的画法（顶点位置与斜边
        // 跨度取同一个量）会让两个括号的**回折点**都落在中线上，
        // 而那些点恰好被 `x < center` / `x > center` 的严格比较排除掉，
        // 断言于是失效——这一条正是被变异验证抓出来的。
        // 改成按"该线段是否触及某一侧"分组，再比较两组的**完整横向跨度**。
        let mut left: Vec<[egui::Pos2; 2]> = Vec::new();
        let mut right: Vec<[egui::Pos2; 2]> = Vec::new();
        for seg in &segments {
            if seg.iter().any(|p| p.x < center_x) {
                left.push(*seg);
            } else if seg.iter().any(|p| p.x > center_x) {
                right.push(*seg);
            }
        }
        assert_eq!(left.len(), 2, "左侧尖括号应由 2 条线段构成");
        assert_eq!(right.len(), 2, "右侧尖括号应由 2 条线段构成");

        let span = |segs: &[[egui::Pos2; 2]]| -> (f32, f32) {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for p in segs.iter().flatten() {
                lo = lo.min(p.x);
                hi = hi.max(p.x);
            }
            (lo, hi)
        };
        let (l_min, l_max) = span(&left);
        let (r_min, r_max) = span(&right);

        // 1. 两个括号的横向范围**不得重叠**——重叠即在中心连成一体。
        assert!(
            l_max < r_min,
            "两个尖括号的横向范围重叠（左止于 {l_max:.1}，右起于 {r_min:.1}），\
             视觉上会连成一个菱形而不是一对括号"
        );
        assert!(
            r_min - l_max > 3.0,
            "两括号间隙仅 {:.1}px，过小",
            r_min - l_max
        );

        // 2. 每个括号的两条线段必须**共用顶点**，且顶点在该括号最外侧。
        //    少了这一条，"两条平行竖线"也能通过上面的跨度断言。
        let tip = |segs: &[[egui::Pos2; 2]]| -> egui::Pos2 {
            segs[0]
                .iter()
                .find(|a| segs[1].iter().any(|b| b.distance(**a) < 0.01))
                .copied()
                .expect("同一括号的两条线段不共顶点——画出来是两条平行线，不是尖括号")
        };

        let l_tip = tip(&left);
        let r_tip = tip(&right);
        assert!(
            (l_tip.x - l_min).abs() < 0.01,
            "左侧尖括号的顶点不在最外侧（顶点 {:.1}，最外侧 {l_min:.1}）——括号朝向画反了",
            l_tip.x
        );
        assert!(
            (r_tip.x - r_max).abs() < 0.01,
            "右侧尖括号的顶点不在最外侧（顶点 {:.1}，最外侧 {r_max:.1}）——括号朝向画反了",
            r_tip.x
        );

        // 3. 反向断言：两个顶点必须分居中线的**两侧**，
        //    防止把两个括号都画到同一侧（那样跨度断言也可能通过）。
        assert!(l_tip.x < center_x, "左侧尖括号的顶点跑到了中线右侧");
        assert!(r_tip.x > center_x, "右侧尖括号的顶点跑到了中线左侧");
    }

    /// 图标尺寸必须为正，否则布局预留的空间会失效。
    #[test]
    fn icon_size_is_usable() {
        // 写成对派生值的断言：图标尺寸是一个需要与字号（CAPTION 11.5 —
        // HEADING 18）协调的参数，过大或过小都会破坏侧边栏的行高节奏。
        // 直接断言常量会被 clippy 判为 `assertions_on_constants`，而这个
        // 约束本身是有意义的（改 ICON_SIZE 时应当被拦下），因此保留断言
        // 而把它表达为"相对字号的比例关系"。
        let ratio_when_small = ICON_SIZE / text::CAPTION;
        let ratio_when_large = ICON_SIZE / text::HEADING;

        assert!(
            (1.0..=1.6).contains(&ratio_when_small),
            "图标相对 Caption 字号的比例应在 1.0～1.6，实得 {ratio_when_small:.2}"
        );
        assert!(
            (0.8..=1.4).contains(&ratio_when_large),
            "图标相对 Heading 字号的比例应在 0.8～1.4，实得 {ratio_when_large:.2}"
        );
    }

    // -----------------------------------------------------------------------
    // 需求三：主题切换必须覆盖所有框架级组件
    // -----------------------------------------------------------------------

    /// **本组测试对应需求三的核心机制。**
    ///
    /// "主题切换即时生效并覆盖所有页面与组件"这一要求，靠的不是逐个组件
    /// 改色，而是 `install()` 把色板写进 egui 的**两套**按主题存储的
    /// `Visuals`，再由 `set_theme()` 切换指针。
    ///
    /// 框架级组件（面板、按钮、输入框、滚动条、工具提示、文本选区、光标）
    /// 全部读当前 `Style`，因此安装正确 = 覆盖完整。这条测试断言安装后
    /// 两套 `Visuals` 确实不同——若某次重构让 `style_mut_of` 只作用于
    /// 一套，这里会立即失败。
    #[test]
    fn install_writes_distinct_visuals_into_both_themes() {
        let ctx = Context::default();
        install(&ctx, &UiTheme::new(ThemeMode::Dark, true));

        let dark = ctx.style_of(egui::Theme::Dark);
        let light = ctx.style_of(egui::Theme::Light);

        // 面板底是两个主题最直观的差异载体。
        assert_ne!(
            dark.visuals.panel_fill, light.visuals.panel_fill,
            "两套主题的面板底色必须不同——否则 set_theme 切换后界面不变"
        );
        assert_ne!(
            dark.visuals.override_text_color, light.visuals.override_text_color,
            "两套主题的默认文字色必须不同"
        );
        // `dark_mode` 标志影响 egui 内部的若干默认推导（如阴影强度）。
        assert!(dark.visuals.dark_mode, "深色样式必须标记 dark_mode");
        assert!(!light.visuals.dark_mode, "浅色样式不得标记 dark_mode");
    }

    /// 切换主题后，框架级组件的取色源必须真的换了。
    ///
    /// 这是"即时生效"的可观测证据：`set_theme` 只改一个枚举，但
    /// `ctx.style_of(ctx.theme())` 随之返回另一套 `Style`，所有框架组件
    /// 下一帧即用新色。
    ///
    /// 注意 `Context` **没有** `style()`——egui 0.36 起样式按主题存储，
    /// 必须先问 `ctx.theme()` 当前是哪套，再用 `style_of()` 取。
    #[test]
    fn set_theme_switches_the_active_style_source() {
        let ctx = Context::default();
        install(&ctx, &UiTheme::new(ThemeMode::Dark, true));

        set_theme(&ctx, ThemeMode::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark, "set_theme 后 ctx.theme() 应同步");
        let dark_fill = ctx.style_of(ctx.theme()).visuals.panel_fill;

        set_theme(&ctx, ThemeMode::Light);
        assert_eq!(
            ctx.theme(),
            egui::Theme::Light,
            "set_theme 后 ctx.theme() 应同步"
        );
        let light_fill = ctx.style_of(ctx.theme()).visuals.panel_fill;

        assert_ne!(
            dark_fill, light_fill,
            "set_theme 之后取到的样式必须换套——否则框架组件不会跟着变"
        );
    }

    /// 玻璃开关必须同时作用于明暗两套样式。
    ///
    /// 一个容易犯的错误是只对"当前主题"应用玻璃开关：那样用户在深色下
    /// 关掉玻璃，再切到浅色又会看到玻璃回来了。这里断言开关对两套都生效。
    ///
    /// 断言对象的选择有讲究：`window_fill` 取 `bg_elevated`，而
    /// `without_glass()` **刻意不动** `bg_elevated`（它已经是不透明实心底，
    /// 关闭玻璃不应改变它），所以不能用它判断开关是否生效。真正会变的是
    /// **玻璃填充色**——它体现在控件的 `weak_bg_fill` 上（按钮底）。
    #[test]
    fn glass_toggle_applies_to_both_stored_themes() {
        let on_ctx = Context::default();
        install(&on_ctx, &UiTheme::new(ThemeMode::Dark, true));

        let off_ctx = Context::default();
        install(&off_ctx, &UiTheme::new(ThemeMode::Dark, false));

        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let on = &on_ctx.style_of(theme).visuals;
            let off = &off_ctx.style_of(theme).visuals;

            // 开启玻璃时控件底是半透明的，关闭后变为不透明实心。
            assert!(
                on.widgets.inactive.weak_bg_fill.a() < 255,
                "{theme:?} 主题下开启玻璃时控件底应为半透明"
            );
            assert_eq!(
                off.widgets.inactive.weak_bg_fill.a(),
                255,
                "{theme:?} 主题下关闭玻璃后控件底应不透明——\
                 若仍半透明，说明开关没有作用到这套样式"
            );
            assert_ne!(
                on.widgets.inactive.weak_bg_fill, off.widgets.inactive.weak_bg_fill,
                "{theme:?} 主题下玻璃开关未改变控件底色"
            );
        }
    }

    /// `sync_theme` 依赖的幂等性：同一 `(mode, glass)` 组合重复安装必须稳定。
    ///
    /// `app.rs` 的 `sync_theme` 用 `(mode, glass)` 作为"是否需要重装"的
    /// 判据。若安装结果不稳定（例如依赖随机或残留状态），这个判据就会
    /// 导致样式抖动。
    #[test]
    fn reinstalling_the_same_configuration_is_stable() {
        let ctx_a = Context::default();
        install(&ctx_a, &UiTheme::new(ThemeMode::Light, true));

        let ctx_b = Context::default();
        install(&ctx_b, &UiTheme::new(ThemeMode::Light, true));
        // 第二次安装（同一 Context）——模拟 `sync_theme` 的重复调用。
        install(&ctx_b, &UiTheme::new(ThemeMode::Light, true));

        let a = &ctx_a.style_of(egui::Theme::Light).visuals;
        let b = &ctx_b.style_of(egui::Theme::Light).visuals;

        assert_eq!(
            a.panel_fill, b.panel_fill,
            "重复安装同一配置应得到相同的面板底"
        );
        assert_eq!(a.window_fill, b.window_fill, "重复安装应得到相同的窗口底");
        assert_eq!(
            a.override_text_color, b.override_text_color,
            "重复安装应得到相同的默认文字色"
        );
        assert_eq!(
            a.widgets.inactive.weak_bg_fill, b.widgets.inactive.weak_bg_fill,
            "重复安装应得到相同的控件底色"
        );
    }
}
