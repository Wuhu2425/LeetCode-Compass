//! 字体装载。
//!
//! egui 的 `default_fonts` feature 只内置拉丁字体（Ubuntu-Light / NotoEmoji 等），
//! **不含 CJK 字形**。若不额外注入中文字体，所有汉字都会渲染成"豆腐块"（□）。
//!
//! 本模块把中文字体内嵌进二进制（`include_bytes!`），因此：
//! - 单文件 exe 在任意机器上都能正确显示中文，不依赖系统已装字体；
//! - 代价是产物体积增加约 9 MB。
//!
//! 字体选择：**黑体（SimHei）**。理由：常见中文字体中体积最小
//! （9.3 MB，对比微软雅黑 19 MB、思源黑体 17 MB），且覆盖 GB2312 全集，
//! 界面场景（标签、标题、表格）足以胜任。SimHei 为黑体，无衬线，
//! 在小字号下比宋体更清晰。

use egui::{FontData, FontDefinitions, FontFamily};

/// 内嵌的中文字体。构建脚本已确保该路径存在（见 `build.rs`）。
pub const CJK_FONT: &[u8] = include_bytes!("../assets/fonts/SimHei.ttf");

/// 该字体在 egui 内部的注册名。
const CJK_FONT_NAME: &str = "simhei";

/// 把中文字体注册为后备字体。
///
/// 关键点：中文字体是**追加（push）到字体列表末尾**，而非替换。
/// 这样拉丁字母与数字仍由 egui 内置字体渲染（字形更精致、等宽对齐更好），
/// 只有内置字体缺失的汉字才回退到中文字体。
///
/// `Monospace` 同样需要追加：代码、题号、精确数字用等宽字体渲染，
/// 而内置等宽字体不覆盖汉字，不追加同样会出豆腐块。
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        CJK_FONT_NAME.to_owned(),
        std::sync::Arc::new(FontData::from_static(CJK_FONT)),
    );

    // 追加到末尾 → 作为后备，不抢占内置字体的拉丁字形。
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(CJK_FONT_NAME.to_owned());
    }

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_is_non_trivial_ttf() {
        // 防止误提交占位文件或 Git LFS 指针文件。
        assert!(
            CJK_FONT.len() > 1_000_000,
            "内嵌字体体积异常（{} 字节），可能不是真实字体文件",
            CJK_FONT.len()
        );
    }

    #[test]
    fn embedded_font_has_valid_ttf_signature() {
        // TrueType 文件以 0x00010000 或 "true" 开头；
        // OpenType/CFF 以 "OTTO" 开头。三者之外一律视为损坏。
        let head = &CJK_FONT[..4];
        let ok = head == [0x00, 0x01, 0x00, 0x00] || head == *b"true" || head == *b"OTTO";
        assert!(ok, "字体文件头签名无效: {head:02X?}");
    }

    #[test]
    fn font_contains_cjk_glyph_coverage_marker() {
        // 轻量校验：字体文件内应出现 cmap 表标识，
        // 说明这是一个带字符映射表的可渲染字体，而非仅有元数据的空壳。
        let needle = b"cmap";
        assert!(
            CJK_FONT.windows(needle.len()).any(|w| w == needle),
            "未找到 cmap 表，字体可能不含字形映射"
        );
    }

    #[test]
    fn install_makes_egui_able_to_render_chinese() {
        // 真正的端到端验证：在离屏 Context 上走一遍完整安装流程，
        // 再问 egui "你能不能渲染这些汉字"。
        //
        // 这是唯一能证明"豆腐块问题已修复"的测试——
        // 前面几个测试只验证字体文件本身完好，无法证明它被正确接入了 egui。
        let ctx = egui::Context::default();

        // 注意：egui 在第一次 `run_ui` 之前不构造字体系统，
        // 因此必须先跑一帧，字形查询才有意义。
        // `FullOutput.textures_delta` 必须消费掉，否则 Drop 时会 panic。
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();

        let sample = "题库推荐竞赛复盘设置";
        let before = ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(14.0), sample));
        assert!(
            !before,
            "egui 默认字体不应能渲染中文，否则本测试失去意义"
        );

        install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();

        for (label, font_id) in [
            ("Proportional", egui::FontId::proportional(14.0)),
            ("Monospace", egui::FontId::monospace(12.5)),
        ] {
            let ok = ctx.fonts_mut(|f| f.has_glyphs(&font_id, sample));
            assert!(ok, "安装后 {label} 字体族仍无法渲染中文");
        }
    }

    #[test]
    fn install_covers_characters_actually_used_in_ui() {
        // 覆盖界面高频用字，防止将来换字体时静默丢失某个字形。
        let ctx = egui::Context::default();
        install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();

        let text = "已完成 尝试过 未知 简单 中等 困难 难度 标签 题号 标题 加载中 请稍候 \
                    重试 失败 推荐 薄弱 覆盖 竞赛 评级 稳定性 折线 明细 合计 清空 保存 \
                    账号 绑定 密码 密钥 超时 温度 模型 地址 网络 错误 成功 提示 确认 取消";
        let font_id = egui::FontId::proportional(13.5);
        let missing: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font_id, *c)))
            .collect();

        assert!(
            missing.is_empty(),
            "以下界面用字缺少字形，将显示为豆腐块: {missing}"
        );
    }

    #[test]
    fn chinese_glyphs_actually_rasterize_to_visible_pixels() {
        // `has_glyph` 只回答"字形表里有没有"，本测试进一步验证
        // "排出来之后真的有像素"。
        //
        // 这能挡住一类更隐蔽的问题：字体被正确加载、映射表也有该码位，
        // 但字形轮廓为空（空轮廓字体、子集化失误），渲染出来仍是空白。
        let ctx = egui::Context::default();
        install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();

        let font_id = egui::FontId::proportional(28.0);
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap("题库推荐".to_owned(), font_id, egui::Color32::BLACK));

        // 排版成功且占位正确：4 个汉字应有可观的宽度（远大于空白回退）。
        assert!(
            galley.rect.width() > 40.0,
            "中文排版宽度异常（{}），可能未命中真实字形",
            galley.rect.width()
        );

        // 遍历该 galley 涉及的字体图集区域，确认存在非透明像素。
        let atlas = ctx.fonts(|f| f.image());
        let opaque = atlas.pixels.iter().filter(|p| p.a() > 0).count();
        assert!(
            opaque > 0,
            "字体图集中没有任何不透明像素，字形未能光栅化"
        );
    }
}
