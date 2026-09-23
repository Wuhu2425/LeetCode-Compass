//! AI 小助理的提示词编排。
//!
//! ## 设计思路
//!
//! 助理的智能程度取决于**喂给它的上下文质量**。本模块负责把应用内
//! 已经掌握的结构化数据（账号画像、技能分布、推荐结果、竞赛分析）
//! 组装成一份紧凑、准确的系统提示。
//!
//! 关键决策：**不把原始 JSON 直接丢给模型**。理由有三：
//! 1. 原始数据冗余度高（大量字段模型用不上），浪费 token；
//! 2. 自然语言的统计描述更容易被模型正确引用；
//! 3. 可以在组装阶段完成数据清洗，避免异常值干扰判断。
//!
//! ## 上下文长度控制
//!
//! 桌面工具的 LLM 额度通常有限。本模块对各项数据设定了明确的截断上界
//! （标签数、推荐数、提交记录数），保证提示词规模可控。

use crate::leetcode::types::RecentSubmission;
use crate::models::{ChatMessage, ContestRecord, TagProgress, UserProfile};
use crate::logic::contest::ContestAnalysis;
use crate::logic::recommend::RecommendationSet;

/// 系统提示中最多列出的标签数。
const MAX_TAGS_IN_PROMPT: usize = 15;
/// 系统提示中最多列出的推荐题目数。
const MAX_RECS_IN_PROMPT: usize = 8;
/// 系统提示中最多列出的最近提交数。
const MAX_SUBMISSIONS_IN_PROMPT: usize = 10;
/// 保留的历史对话轮数上限（超出后从最旧的开始丢弃）。
pub const MAX_HISTORY_MESSAGES: usize = 20;

/// 助理可用的全部上下文数据。
///
/// 各字段均可缺失——用户可能尚未绑定账号、未授权凭据、未拉取数据。
/// 提示词组装会明确告知模型"哪些数据不可用"，避免模型凭空推测。
#[derive(Default, Clone)]
pub struct AssistantContext {
    pub profile: Option<UserProfile>,
    pub tag_stats: Vec<TagProgress>,
    pub recommendations: Option<RecommendationSet>,
    pub contest_records: Vec<ContestRecord>,
    pub contest_analysis: Option<ContestAnalysis>,
    pub recent_submissions: Vec<RecentSubmission>,
    /// 本地题库中的题目总数。
    pub problem_count: usize,
    /// 是否已配置有效的 LeetCode 会话凭据。
    pub authenticated: bool,
    /// 当前站点。
    ///
    /// 用于区分"数据尚未获取"与"该站点不提供此数据"——两者对模型而言
    /// 的含义完全不同：前者可以建议用户刷新，后者建议了也没用，
    /// 应当直接说明是站点限制。
    pub site: crate::config::LeetCodeSite,
}

impl AssistantContext {
    /// 生成系统提示词。
    pub fn build_system_prompt(&self) -> String {
        let mut p = String::with_capacity(2048);

        p.push_str(SYSTEM_ROLE);
        p.push_str("\n\n===== 当前账号数据 =====\n");

        // 数据可用性总览：让模型先知道手里有什么牌。
        p.push_str(&self.data_availability_summary());

        // 账号画像。
        p.push_str("\n--- 账号概况 ---\n");
        match &self.profile {
            Some(profile) => {
                p.push_str(&format!("用户名：{}\n", profile.username));
                if let Some(name) = &profile.real_name {
                    p.push_str(&format!("昵称：{}\n", name));
                }
                if let Some(r) = profile.ranking {
                    p.push_str(&format!("全球排名：第 {r} 名\n"));
                }
                if let Some(rep) = profile.reputation {
                    p.push_str(&format!("声望：{rep}\n"));
                }
                p.push_str(&format!(
                    "已通过题目：{} 道（简单 {} / 中等 {} / 困难 {}）\n",
                    profile.solved.total(),
                    profile.solved.easy,
                    profile.solved.medium,
                    profile.solved.hard
                ));
                if profile.failed.total() > 0 {
                    p.push_str(&format!(
                        "尝试未通过：{} 道（简单 {} / 中等 {} / 困难 {}）\n",
                        profile.failed.total(),
                        profile.failed.easy,
                        profile.failed.medium,
                        profile.failed.hard
                    ));
                }
                if let Some(s) = profile.streak {
                    p.push_str(&format!("连续活跃 {s} 天\n"));
                }
                if let Some(d) = profile.total_active_days {
                    p.push_str(&format!("累计活跃 {d} 天\n"));
                }
                // 难度构成分析：帮助模型判断用户水平层次。
                p.push_str(&self.difficulty_profile_hint(profile));
            }
            None => {
                // **必须区分两种缺失**：profile 为 None 可能是"没绑账号"，
                // 也可能是"已绑定但本会话还没拉取数据"。混为一谈会让
                // 已绑定的用户每次新会话都被劝去"绑定账号"（用户实测报告）。
                if self.authenticated {
                    p.push_str(
                        "（用户**已经绑定** LeetCode 账号且会话凭据有效，\
                         但本会话尚未拉取个人统计数据——请勿声称用户未绑定账号。\
                         可以建议用户点击\u{201c}刷新账号\u{201d}获取最新数据，\
                         同时直接回答通用的 LeetCode 学习问题。）\n",
                    );
                } else {
                    p.push_str(
                        "（用户尚未绑定 LeetCode 账号，无法提供个人统计数据。\
                         请先建议用户绑定账号，同时可以回答通用的 LeetCode 学习问题。）\n",
                    );
                }
            }
        }

        // 技能标签分布。
        p.push_str("\n--- 知识点分布 ---\n");
        if self.tag_stats.is_empty() {
            // 区分"还没拉"与"这个站没有"——对下游建议的含义完全不同。
            if self.site.supports_tag_stats() {
                p.push_str("（尚未获取到技能分布数据，可建议用户点击刷新）\n");
            } else {
                p.push_str(
                    "（当前站点不提供技能标签统计接口，此项数据**永久不可得**。\
                     不要建议用户刷新或检查配置；如需按知识点分析，\
                     应说明这是力扣中国站的接口限制，建议改用国际站账号。）\n",
                );
            }
        } else {
            let mut sorted: Vec<&TagProgress> = self.tag_stats.iter().collect();
            sorted.sort_by(|a, b| b.solved.cmp(&a.solved).then_with(|| a.tag_slug.cmp(&b.tag_slug)));

            p.push_str(&format!(
                "共覆盖 {} 个知识点标签。以下按已解题数降序：\n",
                self.tag_stats.len()
            ));
            for t in sorted.iter().take(MAX_TAGS_IN_PROMPT) {
                let mastery = if t.total > 0 {
                    format!("{:.0}%", t.mastery() * 100.0)
                } else {
                    "未知".to_string()
                };
                p.push_str(&format!(
                    "  · {}：已解 {} 题（掌握度 {mastery}）\n",
                    t.tag_name, t.solved
                ));
            }
            if sorted.len() > MAX_TAGS_IN_PROMPT {
                p.push_str(&format!(
                    "  …另有 {} 个标签未列出\n",
                    sorted.len() - MAX_TAGS_IN_PROMPT
                ));
            }

            // 零解题的标签是重要信号。
            let untouched: Vec<&&TagProgress> =
                sorted.iter().filter(|t| t.solved == 0).take(5).collect();
            if !untouched.is_empty() {
                p.push_str("完全未练习过（0 题）的知识点：\n");
                for t in untouched {
                    p.push_str(&format!("  · {}\n", t.tag_name));
                }
            }
        }

        // 推荐结果。
        p.push_str("\n--- 系统推荐（算法生成） ---\n");
        match &self.recommendations {
            Some(set) if !set.items.is_empty() => {
                p.push_str(&format!("推荐模式：{}\n", set.mode.label_zh()));
                if !set.weak_tags.is_empty() {
                    p.push_str("识别出的薄弱知识点：\n");
                    for t in set.weak_tags.iter().take(5) {
                        p.push_str(&format!(
                            "  · {}（已解 {}/{} 题）\n",
                            t.tag_name, t.solved, t.total
                        ));
                    }
                }
                p.push_str("推荐练习的题目：\n");
                // 复用 recommend 模块的格式化函数，避免两处各写一份
                // 题目清单格式（改一处忘另一处会导致口径不一致）。
                p.push_str(&crate::logic::recommend::RecommendationEngine::recommendations_for_llm(
                    &set.items,
                    MAX_RECS_IN_PROMPT,
                ));
                if let Some(note) = &set.note {
                    p.push_str(&format!("补充说明：{note}\n"));
                }
            }
            _ => {
                p.push_str("（暂无推荐结果。用户可在「推荐」页面生成）\n");
            }
        }

        // 竞赛情况。
        p.push_str("\n--- 竞赛表现 ---\n");
        if self.contest_records.is_empty() {
            p.push_str("（暂无竞赛记录）\n");
        } else if let Some(a) = &self.contest_analysis {
            p.push_str(&a.to_llm_context());
            if !a.insights.is_empty() {
                p.push_str("分析要点：\n");
                for i in a.insights.iter().take(6) {
                    p.push_str(&format!("  · {i}\n"));
                }
            }
        } else {
            p.push_str(&format!("共参加 {} 场竞赛。\n", self.contest_records.len()));
        }

        // 近期提交。
        if !self.recent_submissions.is_empty() {
            p.push_str("\n--- 近期提交 ---\n");
            let accepted = self
                .recent_submissions
                .iter()
                .filter(|s| s.is_accepted)
                .count();
            p.push_str(&format!(
                "最近 {} 次提交中 {} 次通过（通过率约 {:.0}%）\n",
                self.recent_submissions.len(),
                accepted,
                accepted as f64 / self.recent_submissions.len() as f64 * 100.0
            ));
            for s in self.recent_submissions.iter().take(MAX_SUBMISSIONS_IN_PROMPT) {
                let mark = if s.is_accepted { "✓" } else { "✗" };
                let lang = s.lang.as_deref().unwrap_or("未知语言");
                p.push_str(&format!("  {mark} {}（{}）\n", s.title, lang));
            }
        }

        p.push_str("\n===== 数据结束 =====\n");
        p.push_str(BEHAVIOR_GUIDE);

        p
    }

    /// 数据可用性总览。
    ///
    /// 这一节的作用是**防止模型编造数据**。明确列出哪些数据缺失后，
    /// 模型会倾向于承认"我没有这个信息"而不是推测。
    fn data_availability_summary(&self) -> String {
        let mut s = String::new();
        let has = |b: bool| if b { "✓" } else { "✗" };

        // 先声明站点，因为它决定了下面若干项的"✗"是暂时还是永久。
        s.push_str(&format!("当前站点：{}\n", self.site.label_zh()));

        s.push_str(&format!(
            "本地题库：{} 道题 {}\n",
            self.problem_count,
            has(self.problem_count > 0)
        ));
        s.push_str(&format!(
            "账号画像：{}\n",
            if self.profile.is_some() {
                "✓"
            } else if self.authenticated {
                "✗（已绑定凭据但本会话未拉取数据，可建议用户刷新）"
            } else {
                "✗（未绑定账号）"
            }
        ));

        // 对站点不支持的能力，把"✗"升级为"✗（本站不支持）"。
        // 这个后缀很关键：没有它，模型会建议用户"刷新试试"或
        // "检查配置"，而这些建议在该站点**永远无效**。
        let mark = |supported: bool, present: bool| {
            if !supported {
                "✗（本站接口不支持，无法获取）"
            } else if present {
                "✓"
            } else {
                "✗（可建议用户刷新）"
            }
        };

        s.push_str(&format!(
            "知识点分布：{}\n",
            mark(self.site.supports_tag_stats(), !self.tag_stats.is_empty())
        ));
        s.push_str(&format!(
            "逐题完成状态：{}\n",
            if self.authenticated {
                "✓（已授权，数据准确）"
            } else {
                "✗（未授权会话凭据，无法获知哪些题已做）"
            }
        ));
        s.push_str(&format!(
            "近期提交记录：{}\n",
            mark(
                self.site.supports_recent_submissions(),
                !self.recent_submissions.is_empty()
            )
        ));
        s.push_str(&format!(
            "竞赛记录：{}\n",
            mark(
                self.site.supports_contest(),
                !self.contest_records.is_empty()
            )
        ));
        s.push_str(&format!(
            "提交日历/连续活跃：{}\n",
            mark(self.site.supports_calendar(), self.profile.as_ref().is_some_and(|p| p.streak.is_some()))
        ));

        s
    }

    /// 根据解题构成给出水平层级的描述。
    ///
    /// 与其让模型自己从三个数字推断水平，不如直接给出结论——这能显著
    /// 提升建议的针对性。
    fn difficulty_profile_hint(&self, profile: &UserProfile) -> String {
        let total = profile.solved.total();
        if total == 0 {
            return "（尚无解题记录，属于初学者阶段）\n".to_string();
        }

        let easy_pct = profile.solved.easy as f64 / total as f64;
        let hard_pct = profile.solved.hard as f64 / total as f64;

        // 判断依据：
        // - 简单题占比过高 -> 仍在打基础
        // - 困难题占比可观 -> 已具备较强能力
        // - 中等题为主且困难题有一定比例 -> 主力进阶阶段
        let level = if total < 20 {
            "入门阶段：题目总量较少，建议先建立稳定的刷题节奏"
        } else if easy_pct > 0.7 {
            "基础阶段：以简单题为主，建议开始系统性接触中等难度题"
        } else if hard_pct > 0.25 {
            "进阶阶段：困难题占比较高，能力已较强，建议针对性攻克薄弱专项"
        } else if easy_pct < 0.25 {
            "高阶阶段：简单题占比很低，精力集中在中等与困难题"
        } else {
            "成长阶段：难度分布较为均衡，处于主力提升期"
        };

        format!("水平判断：{level}\n")
    }

    /// 组装发给模型的完整消息列表。
    ///
    /// 结构：`[system] + 历史对话（已裁剪） + [本轮用户输入]`
    ///
    /// `max_turns` 控制保留的历史轮数。裁剪策略是**保留最近的消息**：
    /// 较早的对话对当前问题的价值通常更低，而且助理的系统提示里已经
    /// 包含了完整的数据快照，因此丢弃早期上下文不会丢失关键事实。
    pub fn build_messages(
        &self,
        history: &[ChatMessage],
        user_input: &str,
        max_turns: usize,
    ) -> Vec<crate::llm::WireMessage> {
        let mut msgs = Vec::with_capacity(history.len() + 2);

        // 系统提示始终在首位。
        msgs.push(crate::llm::WireMessage::system(self.build_system_prompt()));

        // 裁剪历史：保留最近 max_turns 条。
        let limit = if max_turns == 0 {
            MAX_HISTORY_MESSAGES
        } else {
            max_turns.min(MAX_HISTORY_MESSAGES)
        };

        let start = history.len().saturating_sub(limit);
        for m in &history[start..] {
            // 错误提示不发给模型：它们是本地生成的 UI 提示，
            // 作为对话内容会误导模型。
            if m.is_error {
                continue;
            }
            let wire = match m.role {
                crate::models::ChatRole::System => crate::llm::WireMessage::system(&m.content),
                crate::models::ChatRole::User => crate::llm::WireMessage::user(&m.content),
                crate::models::ChatRole::Assistant => {
                    crate::llm::WireMessage::assistant(&m.content)
                }
            };
            msgs.push(wire);
        }

        msgs.push(crate::llm::WireMessage::user(user_input));
        msgs
    }
}

/// 助理的角色设定。
///
/// 撰写要点：
/// - 明确身份与专长领域，让回答风格收敛到"算法学习教练"
/// - 明确禁止编造数据（这是最容易出问题的地方）
/// - 要求给出可执行的建议而非泛泛而谈
const SYSTEM_ROLE: &str = r#"你是「LeetCode Compass」应用内置的算法学习助理，一位经验丰富的算法竞赛教练与刷题规划师。

你的职责：
1. 基于用户真实的 LeetCode 数据，分析其学习状况（水平定位、强项弱项、进步趋势）
2. 给出具体、可执行的练习建议（推荐哪一类题、练多少、关注什么）
3. 解答算法与数据结构问题，讲解解题思路
4. 帮助用户规划刷题路线，特别是面试准备场景

回答风格：
- 直接给出结论，再解释理由，避免冗长的铺垫
- 用具体数字说话（引用上面数据中的真实统计）
- 建议要可执行，例如「接下来一周集中做 5-8 道滑动窗口的中等题」而非「多练习」
- 涉及算法讲解时，先讲思路和复杂度，再给关键代码要点

语言：使用简体中文回答。"#;

/// 行为约束。
///
/// 与 SYSTEM_ROLE 分开是为了方便在不改动角色设定的前提下调整约束。
const BEHAVIOR_GUIDE: &str = r#"
===== 重要约束 =====
1. 只使用上面提供的真实数据。数据中没有的信息，明确说明「我这边没有这项数据」，绝不编造具体数字。
2. 若「逐题完成状态」标记为未授权，不要假设某道题用户是否做过；可以建议用户授权以获得更精准的分析。
3. 若账号概况显示「未绑定账号」，先引导其完成绑定与授权，同时可以回答通用的算法学习问题。
4. 不要建议用户进行任何形式的作弊行为（如抄袭题解、代写代码）。
5. 涉及具体题目时，优先提及上面推荐列表中已有的题目。
6. 回答控制在合理的长度内（通常 300 字以内），除非用户明确要求详细讲解。"#;

/// 快捷提问模板。
///
/// 提供预置问题可降低用户的使用门槛——很多用户不知道能问什么。
pub fn quick_prompts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("我接下来该练什么", "根据我的账号数据，分析我的薄弱环节，并给出接下来两周具体的刷题计划。"),
        ("分析我的水平", "综合我的解题数据和竞赛表现，客观评估我目前的算法水平处于什么阶段？"),
        ("我的强项和弱项", "我的知识点分布中，哪些是我的强项，哪些是明显的短板？短板应该优先补哪个？"),
        ("竞赛表现复盘", "分析我最近的竞赛记录，我的主要问题在哪里？是速度、准确率还是知识面？"),
        ("面试准备建议", "我准备参加技术面试，根据我现在的水平，应该重点准备哪些题型和知识点？"),
        ("解释推荐理由", "解释一下系统为什么给我推荐这些题目，这些题目之间的联系是什么？"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DifficultyCounts;

    fn ctx_with_profile() -> AssistantContext {
        AssistantContext {
            profile: Some(UserProfile {
                username: "alice".into(),
                real_name: Some("Alice".into()),
                ranking: Some(50000),
                reputation: Some(1200),
                solved: DifficultyCounts {
                    easy: 120,
                    medium: 60,
                    hard: 8,
                },
                failed: DifficultyCounts {
                    easy: 30,
                    medium: 40,
                    hard: 10,
                },
                streak: Some(12),
                total_active_days: Some(90),
                ..Default::default()
            }),
            tag_stats: vec![
                TagProgress {
                    tag_name: "Array".into(),
                    tag_slug: "array".into(),
                    solved: 80,
                    total: 200,
                },
                TagProgress {
                    tag_name: "Dynamic Programming".into(),
                    tag_slug: "dynamic-programming".into(),
                    solved: 3,
                    total: 120,
                },
                TagProgress {
                    tag_name: "Graph".into(),
                    tag_slug: "graph".into(),
                    solved: 0,
                    total: 80,
                },
            ],
            problem_count: 4059,
            authenticated: true,
            ..Default::default()
        }
    }

    #[test]
    fn system_prompt_includes_role_and_data() {
        let p = ctx_with_profile().build_system_prompt();
        assert!(p.contains("算法学习助理"), "应包含角色设定");
        assert!(p.contains("alice"));
        assert!(p.contains("Array"));
        assert!(p.contains("Dynamic Programming"));
        assert!(p.contains("4059"));
        assert!(p.contains("188"), "总解题数 120+60+8=188 应出现");
    }

    #[test]
    fn system_prompt_flags_missing_data_explicitly() {
        // 空上下文时，提示词必须明确告知模型"没有数据"，
        // 否则模型容易凭空编造统计数字。
        let p = AssistantContext::default().build_system_prompt();
        assert!(p.contains("尚未绑定"));
        assert!(p.contains("尚未获取到技能分布"));
        assert!(p.contains("暂无竞赛记录"));
        // 未授权状态必须明示。
        assert!(p.contains("未授权"));
    }

    #[test]
    fn availability_summary_marks_presence_correctly() {
        let ctx = ctx_with_profile();
        let s = ctx.data_availability_summary();
        assert!(s.contains("本地题库：4059 道题 ✓"));
        assert!(s.contains("账号画像：✓"));
        assert!(s.contains("逐题完成状态：✓"));

        let empty = AssistantContext::default().data_availability_summary();
        assert!(empty.contains("账号画像：✗"));
        assert!(empty.contains("逐题完成状态：✗"));
    }

    #[test]
    fn authenticated_without_profile_never_says_unbound() {
        // 用户实测缺陷：已绑定账号，但新会话还没拉取个人数据，
        // 助理却反复劝用户"去绑定账号"。修复后提示词必须明确
        // 告知模型"已绑定、只是数据未拉取"。
        let ctx = AssistantContext {
            authenticated: true,
            ..Default::default()
        };
        let p = ctx.build_system_prompt();
        assert!(p.contains("已经绑定"), "必须告知模型账号已绑定");
        assert!(p.contains("刷新账号"), "应建议用户刷新数据");
        assert!(
            !p.contains("尚未绑定"),
            "已绑定时严禁出现\u{201c}尚未绑定\u{201d}措辞"
        );
        let s = ctx.data_availability_summary();
        assert!(s.contains("已绑定凭据但本会话未拉取数据"));
    }

    #[test]
    fn unauthenticated_context_warns_about_missing_status() {
        let mut ctx = ctx_with_profile();
        ctx.authenticated = false;
        let p = ctx.build_system_prompt();
        assert!(
            p.contains("无法获知哪些题已做"),
            "必须明确告知模型完成状态不可知"
        );
    }

    // -----------------------------------------------------------------------
    // 站点能力差异在提示词中的体现
    // -----------------------------------------------------------------------

    /// 国际站缺失数据应提示"可以刷新"。
    #[test]
    fn com_missing_data_is_marked_as_refreshable() {
        let ctx = AssistantContext::default(); // 默认站点是国际站
        let s = ctx.data_availability_summary();
        assert!(
            s.contains("可建议用户刷新"),
            "国际站缺数据应提示可刷新，实得:\n{s}"
        );
        assert!(
            !s.contains("本站接口不支持"),
            "国际站不应出现'本站不支持'，实得:\n{s}"
        );
    }

    /// 中国站不支持的能力必须被标注为"永久不可得"。
    ///
    /// 这是本组测试的核心价值：若只标 `✗`，模型会建议用户刷新或检查配置，
    /// 而这些建议在中国站**永远无效**，会让用户陷入无意义的排查循环。
    #[test]
    fn cn_unsupported_capabilities_are_marked_permanent() {
        let ctx = AssistantContext {
            site: crate::config::LeetCodeSite::Cn,
            ..Default::default()
        };
        let s = ctx.data_availability_summary();

        assert!(
            s.contains("本站接口不支持"),
            "中国站的缺失项必须标注为接口不支持，实得:\n{s}"
        );
        assert!(s.contains("中国站"), "应声明当前站点，实得:\n{s}");
    }

    /// 中国站的技能分布段落不得引导模型建议"刷新"。
    #[test]
    fn cn_tag_stats_section_steers_model_away_from_refresh() {
        let ctx = AssistantContext {
            site: crate::config::LeetCodeSite::Cn,
            ..Default::default()
        };
        let p = ctx.build_system_prompt();

        assert!(
            p.contains("永久不可得"),
            "必须明确告知模型该数据永久不可得，实得段落:\n{p}"
        );
        assert!(
            p.contains("不要建议用户刷新"),
            "必须显式禁止'刷新'这类无效建议"
        );
    }

    /// 国际站的技能分布段落仍应保留"可刷新"的引导。
    #[test]
    fn com_tag_stats_section_still_allows_refresh_hint() {
        let ctx = AssistantContext::default();
        let p = ctx.build_system_prompt();
        assert!(
            p.contains("可建议用户点击刷新"),
            "国际站应保留刷新引导，实得:\n{p}"
        );
    }

    #[test]
    fn difficulty_hint_classifies_levels() {
        // 简单题占比过高 -> 基础阶段
        let beginner = UserProfile {
            solved: DifficultyCounts {
                easy: 90,
                medium: 10,
                hard: 0,
            },
            ..Default::default()
        };
        let hint = AssistantContext::default().difficulty_profile_hint(&beginner);
        assert!(hint.contains("基础阶段"), "实得: {hint}");

        // 困难题占比高 -> 进阶阶段
        let advanced = UserProfile {
            solved: DifficultyCounts {
                easy: 30,
                medium: 50,
                hard: 40,
            },
            ..Default::default()
        };
        let hint2 = AssistantContext::default().difficulty_profile_hint(&advanced);
        assert!(hint2.contains("进阶阶段"), "实得: {hint2}");

        // 零解题 -> 初学者
        let empty = UserProfile::default();
        let hint3 = AssistantContext::default().difficulty_profile_hint(&empty);
        assert!(hint3.contains("初学者"));
    }

    #[test]
    fn untouched_tags_are_highlighted() {
        let p = ctx_with_profile().build_system_prompt();
        assert!(
            p.contains("完全未练习过"),
            "0 题的标签是重要信号，应单独列出"
        );
        assert!(p.contains("Graph"));
    }

    #[test]
    fn tag_listing_is_truncated_with_notice() {
        let mut ctx = AssistantContext::default();
        for i in 0..30 {
            ctx.tag_stats.push(TagProgress {
                tag_name: format!("Tag{i}"),
                tag_slug: format!("tag-{i}"),
                solved: (30 - i) as u32,
                total: 50,
            });
        }
        let p = ctx.build_system_prompt();
        assert!(p.contains("另有 15 个标签未列出"), "应说明被截断");
    }

    #[test]
    fn build_messages_puts_system_first_then_history_then_input() {
        let ctx = ctx_with_profile();
        let history = vec![
            ChatMessage::user("之前的问题"),
            ChatMessage::assistant("之前的回答"),
        ];
        let msgs = ctx.build_messages(&history, "新的问题", 10);

        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.contains("算法学习助理"));
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "之前的问题");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[3].content, "新的问题");
    }

    #[test]
    fn build_messages_truncates_history_keeping_most_recent() {
        let ctx = AssistantContext::default();
        let mut history = Vec::new();
        for i in 0..30 {
            history.push(ChatMessage::user(format!("问题{i}")));
            history.push(ChatMessage::assistant(format!("回答{i}")));
        }
        let msgs = ctx.build_messages(&history, "最新问题", 4);

        // system + 4 条历史 + 本轮 = 6
        assert_eq!(msgs.len(), 6);
        // 保留的应是最近的几条。
        assert_eq!(msgs[1].content, "问题28");
        assert_eq!(msgs[4].content, "回答29");
        assert_eq!(msgs[5].content, "最新问题");
    }

    #[test]
    fn build_messages_skips_error_messages() {
        let ctx = AssistantContext::default();
        let history = vec![
            ChatMessage::user("问题"),
            ChatMessage::error("网络错误，请重试"),
            ChatMessage::assistant("正常回答"),
        ];
        let msgs = ctx.build_messages(&history, "继续", 10);

        // system + 2 条有效历史 + 本轮
        assert_eq!(msgs.len(), 4);
        assert!(
            !msgs.iter().any(|m| m.content.contains("网络错误")),
            "本地错误提示不应发送给模型"
        );
    }

    #[test]
    fn build_messages_caps_history_at_max_even_if_asked_more() {
        let ctx = AssistantContext::default();
        let mut history = Vec::new();
        for i in 0..100 {
            history.push(ChatMessage::user(format!("q{i}")));
        }
        let msgs = ctx.build_messages(&history, "now", 500);
        // 应被 MAX_HISTORY_MESSAGES 限制。
        assert_eq!(msgs.len(), MAX_HISTORY_MESSAGES + 2);
    }

    #[test]
    fn build_messages_with_empty_history() {
        let ctx = AssistantContext::default();
        let msgs = ctx.build_messages(&[], "你是什么", 10);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
    }

    #[test]
    fn zero_turns_uses_default_limit() {
        let ctx = AssistantContext::default();
        let msgs = ctx.build_messages(&[], "hi", 0);
        assert_eq!(msgs.len(), 2, "0 应回退到默认上限而非丢弃全部历史");
    }

    #[test]
    fn quick_prompts_are_nonempty_and_diverse() {
        let qp = quick_prompts();
        assert!(qp.len() >= 4);
        for (label, prompt) in qp {
            assert!(!label.is_empty());
            assert!(prompt.len() > 10, "提示词应有实质内容: {prompt}");
        }
    }

    #[test]
    fn system_prompt_contains_anti_fabrication_rule() {
        let p = AssistantContext::default().build_system_prompt();
        assert!(
            p.contains("绝不编造"),
            "防止模型编造数据的约束必须存在"
        );
    }

    #[test]
    fn recommendations_are_included_when_present() {
        use crate::logic::recommend::{RecommendConfig, RecommendMode, RecommendationEngine};
        use crate::models::{Difficulty, Problem, SolveStatus, TopicTag};

        let problems: Vec<Problem> = (0..12)
            .map(|i| Problem {
                question_id: i.to_string(),
                frontend_id: i.to_string(),
                title: format!("DP Problem {i}"),
                title_slug: format!("dp-{i}"),
                difficulty: Difficulty::Medium,
                ac_rate: 45.0,
                tags: vec![TopicTag {
                    name: "Dynamic Programming".into(),
                    slug: "dynamic-programming".into(),
                }],
                status: SolveStatus::Unknown,
                is_paid_only: false,
            })
            .collect();

        let mut ctx = ctx_with_profile();
        ctx.recommendations = Some(RecommendationEngine::recommend(
            &problems,
            &ctx.tag_stats,
            ctx.profile.as_ref(),
            RecommendMode::Basic,
            &RecommendConfig::default(),
        ));

        let p = ctx.build_system_prompt();
        assert!(p.contains("推荐练习的题目"), "应包含推荐列表");
        assert!(p.contains("DP Problem"), "应列出具体题目名");
    }

    #[test]
    fn contest_insights_are_included_when_present() {
        use crate::models::{ContestRecord, ContestSummary};

        let records = vec![
            ContestRecord {
                title: "Weekly 380".into(),
                start_time: 1000,
                rating: 1400.0,
                ranking: 500,
                total_participants: 10000,
                problems_solved: 2,
                total_problems: 4,
                attended: true,
            },
            ContestRecord {
                title: "Weekly 381".into(),
                start_time: 2000,
                rating: 1600.0,
                ranking: 200,
                total_participants: 10000,
                problems_solved: 4,
                total_problems: 4,
                attended: true,
            },
            ContestRecord {
                title: "Weekly 382".into(),
                start_time: 3000,
                rating: 1650.0,
                ranking: 180,
                total_participants: 10000,
                problems_solved: 3,
                total_problems: 4,
                attended: true,
            },
        ];

        let mut ctx = ctx_with_profile();
        ctx.contest_analysis = Some(ContestAnalysis::analyze(&records, ContestSummary::default()));
        ctx.contest_records = records;

        let p = ctx.build_system_prompt();
        assert!(p.contains("竞赛场次：3"));
        assert!(p.contains("分析要点"));
    }

    #[test]
    fn recent_submissions_summary_computed() {
        let mut ctx = ctx_with_profile();
        ctx.recent_submissions = vec![
            RecentSubmission {
                title: "Two Sum".into(),
                title_slug: "two-sum".into(),
                status: "Accepted".into(),
                is_accepted: true,
                submitted_at: None,
                lang: Some("rust".into()),
            },
            RecentSubmission {
                title: "Three Sum".into(),
                title_slug: "three-sum".into(),
                status: "Wrong Answer".into(),
                is_accepted: false,
                submitted_at: None,
                lang: Some("python3".into()),
            },
        ];
        let p = ctx.build_system_prompt();
        assert!(p.contains("2 次提交中 1 次通过"));
        assert!(p.contains("✓ Two Sum"));
        assert!(p.contains("✗ Three Sum"));
    }
}
