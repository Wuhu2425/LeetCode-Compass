//! 源码守卫：把"颜色必须走语义色板"这条约定从注释变成可执行的测试。
//!
//! ## 为什么需要这个模块
//!
//! `src/ui/mod.rs` 里早就写了"颜色一律取自 `theme::Palette`，页面不得
//! 出现 `Color32` 字面量"这条约定。但它只是一句注释，没有任何东西阻止
//! 违反它——改造前实测散落了 **188 处**硬编码颜色，分布在 7 个文件里。
//!
//! 直接后果就是需求三无法达成：主题切换只能覆盖"碰巧用了 `Visuals` 的
//! 组件"，凡是自行取色的地方（气泡、胶囊、图表、状态点）在深色主题下
//! 全是刺眼的亮块或不可读的深灰。
//!
//! 因此本模块用**源码文本扫描**做一次静态检查——这是少见的、真正需要
//! 读文件而不是读运行时的测试场景：要断言的性质是"这段源码里不存在某
//! 种写法"，而不是"某个函数返回了什么"。
//!
//! ## 为什么不用运行时断言
//!
//! 运行时的颜色断言只能覆盖"被调用到的组件"。一个新增页面里的硬编码
//! 颜色要等到有人真正打开那个页面、并且碰巧断言了那个像素才会暴露。
//! 静态扫描在 `cargo test` 时立即失败，且能精确指出文件与行号。

#![cfg(test)]

use std::path::{Path, PathBuf};

/// `src/ui` 目录下允许出现颜色字面量的文件。
///
/// - `theme.rs`：色板定义本身。所有字面量的**唯一**合法归属地。
/// - `widgets.rs`：允许，但只允许在测试代码里（见 [`WIDGETS_ALLOWANCE`]）。
const ALLOWED_TO_DEFINE_COLORS: &[&str] = &["theme.rs"];

/// `widgets.rs` 的例外额度。
///
/// 该文件的测试需要构造任意色值来验证"传入任何颜色都不会 panic"，
/// 属于测试夹具而非产品配色。额度设为 0 之外的确切数字，是为了让
/// **无意中新增**一处产品代码里的硬编码颜色时立即失败——如果这里写成
/// "不限"，守卫就形同虚设。
const WIDGETS_ALLOWANCE: usize = 0;

/// 搜索颜色字面量时匹配的前缀。
///
/// 覆盖 `Color32::from_rgb` / `from_gray` / `from_rgba_unmultiplied` 等
/// 全部构造路径。不匹配 `Color32::NONE` / `Color32::TRANSPARENT` 这类
/// 具名常量——它们是语义化的，不构成硬编码色值。
const FORBIDDEN_PREFIXES: &[&str] = &[
    "Color32::from_rgb",
    "Color32::from_rgba",
    "Color32::from_gray",
    "Color32::from_black_alpha",
    "Color32::from_white_alpha",
];

/// `src/ui` 下的全部页面文件（不含 `theme` / `widgets` / `guards`）。
const PAGE_MODULES: &[&str] = &[
    "home.rs",
    "recommend.rs",
    "contest.rs",
    "assistant.rs",
    "settings.rs",
];

/// 守卫自身的编译期自检：额度必须是 0。
///
/// `WIDGETS_ALLOWANCE` 若为 0，`hits.len() <= WIDGETS_ALLOWANCE` 这类写法
/// 会被 clippy 判为恒真比较。这里用编译期断言把"额度必须是 0"这条设计
/// 意图固定下来，而不是靠一个运行时比较。
const _: () = assert!(WIDGETS_ALLOWANCE == 0);

/// 定位 `src/ui` 目录。
///
/// 用 `CARGO_MANIFEST_DIR` 而非相对路径：测试的工作目录不保证是项目根，
/// 而该环境变量由 cargo 保证指向 `Cargo.toml` 所在目录。
fn ui_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("ui")
}

/// 读入一个 UI 源文件。文件缺失即测试失败——静默跳过等于守卫失效。
fn read_ui_source(file_name: &str) -> String {
    let path = ui_dir().join(file_name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "无法读取 {}：{e}\n\
             若该文件已被重命名或删除，请同步更新 src/ui/guards.rs 的文件清单——\
             守卫测试静默跳过不存在的文件会让约定失去强制力。",
            path.display()
        )
    })
}

/// 剔除注释行与文档注释，只保留真正的代码。
///
/// **这一步是必需的**：本项目的注释大量讨论颜色（例如"改造前这里用了
/// `Color32::from_rgb(240, 246, 254)`"），若不剔除注释，守卫会对文档
/// 文字误报，进而被开发者当成噪音关掉。
///
/// 剔除规则：
/// - 整行以 `//` 开头（含 `///`、`//!`），或行首（去空白后）以 `//` 开头；
/// - 行内 `//` 之后的部分一并截掉。
///
/// 不处理块注释与字符串字面量里的 `//`——本项目的相关写法都是行注释，
/// 而且误剔除只会让守卫**更宽松**（宁可漏报不可误报，否则守卫会被绕过）。
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            // 整行注释直接丢弃。
            if trimmed.starts_with("//") {
                return String::new();
            }
            // 行内注释截断（不处理字符串里的 "//"，见上文说明）。
            match line.find("//") {
                Some(idx) => line[..idx].to_string(),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 在源码中查找被禁止的颜色构造，返回 `(行号, 行内容)` 列表。
///
/// 行号按**原始文件**计（剔除注释会改变行数，因此这里对照原始行输出）。
fn find_color_literals(source: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // 整行注释不算。
        if trimmed.starts_with("//") {
            continue;
        }
        // 行内注释之后的部分不算。
        let code = match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        };
        if FORBIDDEN_PREFIXES.iter().any(|prefix| code.contains(prefix)) {
            hits.push((idx + 1, line.trim().to_string()));
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// R8：页面不得出现颜色字面量
// ---------------------------------------------------------------------------

/// **本组测试对应需求三的实现保障。**
///
/// "主题切换覆盖所有页面与组件"这一要求，唯一可行的落地方式是**所有**
/// 颜色都来自随主题切换的 `Palette`。只要有一个文件自行取色，那个文件
/// 就会在另一套主题下出错。
///
/// 改造前实测：`contest.rs` 51 处、`widgets.rs` 42 处、`recommend.rs`
/// 29 处、`assistant.rs` 26 处、`home.rs` 16 处、`settings.rs` 14 处、
/// `app.rs` 10 处，合计 188 处。现在由本测试守住归零的结果。
#[test]
fn pages_contain_no_hardcoded_color_literals() {
    let mut failures: Vec<String> = Vec::new();

    for file in PAGE_MODULES {
        let source = read_ui_source(file);
        let hits = find_color_literals(&source);
        if !hits.is_empty() {
            let detail = hits
                .iter()
                .map(|(line_no, text)| format!("      {file}:{line_no}  {text}"))
                .collect::<Vec<_>>()
                .join("\n");
            failures.push(format!(
                "  {file} 出现 {} 处颜色字面量：\n{detail}",
                hits.len()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "\n页面文件不得出现颜色字面量——请改用 `Palette` 中的语义令牌，\n\
         或先在 `theme::Palette` 中补一个具名令牌。\n\
         违反的文件：\n{}\n",
        failures.join("\n")
    );
}

/// `app.rs`（外壳层）同样受约束。
///
/// 侧边栏、顶栏、Toast 都画在这里，它们是最先被用户看到的部分——
/// 若这里硬编码，切换主题时"顶栏变了但侧边栏没变"会立刻被察觉。
#[test]
fn app_shell_contains_no_hardcoded_color_literals() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("app.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {}：{e}", path.display()));

    let hits = find_color_literals(&source);
    assert!(
        hits.is_empty(),
        "\nsrc/app.rs 出现 {} 处颜色字面量：\n{}\n",
        hits.len(),
        hits.iter()
            .map(|(line_no, text)| format!("      app.rs:{line_no}  {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `widgets.rs` 的产品代码段必须零字面量。
///
/// 允许的例外额度见 [`WIDGETS_ALLOWANCE`]。设为 0 是刻意的：该文件是
/// 全部组件原语的所在地，一旦这里开了一个口子，后续新增组件会照着抄，
/// 守卫就会逐步失效。
#[test]
fn widget_primitives_contain_no_hardcoded_color_literals() {
    let source = read_ui_source("widgets.rs");
    let hits = find_color_literals(&source);

    assert_eq!(
        hits.len(),
        WIDGETS_ALLOWANCE,
        "\nsrc/ui/widgets.rs 出现 {} 处颜色字面量（额度 {}）：\n{}\n\
         组件原语必须接收 `&Palette` 而非自行取色——否则所有调用方\n\
         都会被这个硬编码传染。\n",
        hits.len(),
        WIDGETS_ALLOWANCE,
        hits.iter()
            .map(|(line_no, text)| format!("      widgets.rs:{line_no}  {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `theme.rs` 是**唯一**允许定义色值的文件。
///
/// 反向断言：色板定义必须真的存在字面量，否则说明"颜色集中管理"这件事
/// 被搬到了别处——那时上面几条测试虽然仍然通过，约定却已经被绕过。
#[test]
fn theme_is_the_single_place_defining_colors() {
    let source = read_ui_source("theme.rs");
    let hits = find_color_literals(&source);

    assert!(
        !hits.is_empty(),
        "theme.rs 未发现任何颜色字面量——色板定义不应被搬到其他文件。\n\
         若确实做了结构调整，请同步更新 src/ui/guards.rs 的白名单。"
    );

    // 白名单里的其他文件（当前为空）也在此登记，便于未来扩展时集中审阅。
    let extra_allowed = &ALLOWED_TO_DEFINE_COLORS[1..];
    for file in extra_allowed {
        // 这些文件应当**不再**出现在页面清单里，否则同一文件同时被
        // 允许与禁止，产生矛盾。
        assert!(
            !PAGE_MODULES.contains(file),
            "{file} 同时出现在色板白名单与页面清单中，约定自相矛盾"
        );
    }
}

// ---------------------------------------------------------------------------
// 守卫自身的有效性
// ---------------------------------------------------------------------------

/// 验证扫描器真的能发现目标写法。
///
/// 守卫最容易出现的失效方式是"因为某种原因永远扫不到东西"（正则写错、
/// 读错路径、剔除逻辑过度）。这里用一段构造的源码做正向验证。
#[test]
fn the_scanner_actually_detects_forbidden_constructs() {
    let sample = r#"
fn a() {
    let c = Color32::from_rgb(10, 20, 30);
    let d = Color32::from_gray(120);
}
"#;
    let hits = find_color_literals(sample);
    assert_eq!(hits.len(), 2, "扫描器应检出两处，实得 {hits:?}");
}

/// 验证扫描器不会被注释里的示例代码误报。
///
/// 这是本模块最容易被误伤的地方：文档注释里大量出现"改造前用了
/// `Color32::from_rgb(...)`"这样的说明。若守卫对这些误报，开发者会
/// 很快学会无视它的输出。
#[test]
fn the_scanner_ignores_comments() {
    let sample = r#"
//! 改造前这里写的是 Color32::from_rgb(240, 246, 254)。
/// 见 `Color32::from_gray(110)`。
fn a() {
    // let c = Color32::from_rgb(10, 20, 30);
    let real = ThemeMode::Dark; // 对比 Color32::from_gray(200)
}
"#;
    let hits = find_color_literals(sample);
    assert!(
        hits.is_empty(),
        "注释中的颜色示例不得被检出，实得 {hits:?}"
    );
}

/// 验证剔除逻辑与检出逻辑对同一文件的处理一致。
///
/// `strip_comments` 是独立实现（供未来做块级处理），这里断言它与
/// `find_color_literals` 对纯注释输入给出一致结论：都认为"没有颜色"。
#[test]
fn stripping_comments_leaves_no_false_positives() {
    let sample = "// Color32::from_rgb(1, 2, 3)\n/// Color32::from_gray(9)\n";
    let stripped = strip_comments(sample);
    for prefix in FORBIDDEN_PREFIXES {
        assert!(
            !stripped.contains(prefix),
            "剔除注释后不应残留 {prefix}，实得：{stripped:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 接线守卫：定义了令牌 ≠ 用上了令牌
// ---------------------------------------------------------------------------

/// 读入 `src/app.rs`。
fn read_app_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("app.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {}：{e}", path.display()))
}

/// 截取一个顶层 `fn name(` 的函数体（到下一个行首 `}` 为止）。
///
/// 用朴素的行扫描而不是正则：本项目的函数体里没有嵌套的顶层 `}`，
/// 而引入正则依赖（或自己写状态机）对这条守卫的收益不成比例。
/// 取不到即 panic——静默返回空串会让下面的 `contains` 断言全部失效。
fn extract_fn_body(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("源码中找不到 `{signature}`——函数被重命名或删除了？"));
    let rest = &source[start..];
    let end = rest
        .find("\n}")
        .unwrap_or_else(|| panic!("`{signature}` 的函数体未正常闭合"));
    rest[..end].to_string()
}

/// **侧边栏导航项必须使用专用的 `sidebar_item_*` 令牌。**
///
/// 这条守卫守的是"接线"，而不是"取值"——`theme` 模块的对比度测试只能证明
/// 色板里**存在**一组高对比色值，证明不了界面**用了**它。
/// 真实的失效路径恰恰是：色板改好了，`nav_item` 却仍引用
/// `text_secondary` / `selected_overlay` 这些通用令牌，
/// 于是所有色板测试全绿，深色主题下的边栏依旧对比度不足。
///
/// 同时反向禁止通用令牌出现在该函数里：`selected_overlay` 在深色下叠出来
/// 的亮蓝底正是"选中文字糊掉"的成因（见 `Palette` 组 9 的说明）。
#[test]
fn sidebar_navigation_uses_dedicated_tokens() {
    // 必须剔除注释再比对：`nav_item` 的文档注释里正大光明地写着
    // "改造前用的是 `text_secondary` / `selected_overlay`"，
    // 不剔除的话这条守卫会对着自己的说明文字报错，然后被人关掉。
    let source = strip_comments(&read_app_source());
    let nav = extract_fn_body(&source, "fn nav_item(");

    for token in [
        "sidebar_item_text",
        "sidebar_item_icon",
        "sidebar_item_hover",
        "sidebar_item_selected_bg",
        "sidebar_item_selected_text",
    ] {
        assert!(
            nav.contains(token),
            "`nav_item` 未使用 `p.{token}`——色板里定义的高对比色没有被接上，\
             深色主题下边栏依旧是改造前的样子。"
        );
    }

    for forbidden in [
        "hover_overlay",
        "selected_overlay",
        "text_secondary",
        "text_tertiary",
    ] {
        assert!(
            !nav.contains(forbidden),
            "`nav_item` 仍在使用通用令牌 `{forbidden}`——\
             导航项的可辨识度要求高于内容区列表行，必须走 `sidebar_item_*`。"
        );
    }
}

/// 侧边栏品牌区必须走 `widgets::brand_mark`，且不得退回旧的两行文案。
///
/// 品牌区是本轮需求明确要"重新设计"的部分。它没有可自动断言的渲染结果
/// （形状由字体度量决定），因此用源码约定把它固定下来：
/// 一旦有人把 `brand_mark` 换回手写的菱形 + 两行标签，这条会失败。
#[test]
fn sidebar_brand_goes_through_brand_mark() {
    // 同样剔除注释：品牌区的注释里保留了旧形态的文案作为改造记录。
    let source = strip_comments(&read_app_source());

    assert!(
        source.contains("widgets::brand_mark("),
        "侧边栏品牌区未调用 `widgets::brand_mark`——品牌标识的绘制必须收口在该函数里，\
         否则新设计会在下一次改动中被无声地改回去。"
    );
    assert!(
        source.contains("brand_mark(ui, &p, \"LeetCode Compass\")"),
        "品牌字标不是 \"LeetCode Compass\"——完整的品牌名（磁贴 + 字标）\
         是明确的产品决策，不要改回单一 \"LeetCode\" 或其他缩写。"
    );
    assert!(
        !source.contains("Compass 学习参谋"),
        "侧边栏品牌区又出现了「Compass 学习参谋」——\
         副标题会让用户第一眼读到的不是应用名。"
    );
}

/// 页面清单不得遗漏任何实际存在的 `src/ui` 页面文件。
///
/// 否则新增一个页面文件、忘了登记，守卫就对该文件完全失效——这是
/// 白名单式检查最常见的漏洞。
#[test]
fn page_module_list_covers_every_ui_source_file() {
    let dir = ui_dir();
    let mut expected: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("无法列出 {}：{e}", dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".rs"))
        // 三类文件不由本清单管理：色板、色板白名单、守卫自身。
        .filter(|name| !ALLOWED_TO_DEFINE_COLORS.contains(&name.as_str()))
        .filter(|name| name != "guards.rs")
        // `widgets.rs` 单独由 `widget_primitives_contain_no_hardcoded_color_literals` 检查。
        .filter(|name| name != "widgets.rs")
        .filter(|name| name != "mod.rs")
        .collect();
    expected.sort();

    let mut registered: Vec<String> = PAGE_MODULES.iter().map(|s| s.to_string()).collect();
    registered.sort();

    assert_eq!(
        expected, registered,
        "src/ui 下的文件清单与守卫登记不一致。\n\
         新增页面文件时请把它加入 src/ui/guards.rs 的 PAGE_MODULES——\n\
         否则该文件中的硬编码颜色不会被任何测试拦住。"
    );
}

/// **跳转链接必须随登录会话的站点走。**
///
/// `Problem::url(site)` 的站点参数是"跳国内还是跳国际"的唯一来源。
/// 这条守卫守的是接线：UI 里每一次 `open::that` 之前构造的题目 URL，
/// 都必须显式传入 `current_site()`。真实的失效路径是——有人新增一个
/// 点击跳转的入口，顺手写了 `p.url(LeetCodeSite::Com)`（或旧签名），
/// 于是登录国内站的用户点题目被带去国际站，所有单元测试照常全绿。
#[test]
fn problem_url_call_sites_follow_the_session_site() {
    for module in ["src/ui/home.rs", "src/ui/recommend.rs"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(module);
        let source = strip_comments(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("无法读取 {}：{e}", path.display())),
        );
        let occurrences = source.matches(".url(").count();
        assert!(
            occurrences > 0,
            "{module} 中找不到 `.url(` 调用——跳转入口被移动或删除了，\
             请同步更新本守卫的文件清单。"
        );
        assert!(
            source.contains("current_site()"),
            "{module} 中构造题目 URL 时未传入 `current_site()`——\
             跳转站点将不随登录会话（国内站/国际站）变化。"
        );
        // 明确禁止把站点写死：写死哪个站都会让另一侧的登录用户跳错。
        assert!(
            !source.contains("url(LeetCodeSite::"),
                "{module} 中存在写死站点的 `url(LeetCodeSite::...)` 调用——\
                 站点必须取自登录会话（`current_site()`），不得硬编码。",
        );
    }
}
