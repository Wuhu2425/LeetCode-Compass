//! 推荐引擎。
//!
//! ## 设计目标
//!
//! 在**两种数据可用性**下都能给出有依据的推荐：
//!
//! - **降级模式（无认证）**：拿不到逐题完成状态，但有全量题库 +
//!   用户的标签解题统计。推荐基于"薄弱知识点的候选题目"。
//! - **增强模式（有认证）**：额外知道哪些题已通过、哪些尝试失败过、
//!   以及提交活跃度。推荐可以排除已完成的题，并把"试过没做出来"的
//!   题提到前面。
//!
//! ## 算法骨架
//!
//! ```text
//! 1. 候选池 = 题库 - 已通过(仅在增强模式可判定) - 会员题(可选排除)
//! 2. 薄弱标签识别：按"掌握率"排序，取最低的一批
//! 3. 打分：score(q) = 标签权重 × 难度适配 × 通过率因子 × 状态加成
//! 4. 排序取 Top-N，生成人类可读理由
//! ```
//!
//! ## 确定性与可解释性
//!
//! 算法必须满足：
//! - **确定性**：同样的输入必然产生同样的输出顺序（所有排序都有
//!   明确的次级比较键，不依赖 HashMap 的迭代顺序）。
//! - **可解释**：每个推荐都附带理由，用户能理解"为什么推这道"。
//!   黑盒推荐对学习工具毫无价值。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::models::{
    Difficulty, Problem, Recommendation, SolveStatus, TagProgress, UserProfile,
};

/// 推荐配置。
#[derive(Debug, Clone)]
pub struct RecommendConfig {
    /// 返回的推荐数量。
    pub top_n: usize,
    /// 参与打分的薄弱标签数量上限。
    pub max_weak_tags: usize,
    /// 是否排除会员专属题（未订阅的用户无法练习）。
    pub exclude_paid_only: bool,
    /// 增强模式下是否排除已通过的题目。
    pub exclude_solved: bool,
    /// "尝试过但未通过"的题目得分加成倍数。
    pub attempted_boost: f64,
}

impl Default for RecommendConfig {
    fn default() -> Self {
        Self {
            top_n: 12,
            max_weak_tags: 8,
            exclude_paid_only: true,
            exclude_solved: true,
            attempted_boost: 1.5,
        }
    }
}

/// 推荐引擎的运行模式。
///
/// 需要 `Serialize/Deserialize`：推荐结果会落库缓存，重启后直接恢复，
/// 否则用户每次打开应用都要重新生成一次（见 `storage::save_recommendations`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendMode {
    /// 无凭据：无法判定完成状态。
    Basic,
    /// 有凭据：可判定完成状态。
    ProgressAware,
}

impl RecommendMode {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Basic => "基础模式（基于技能分布）",
            Self::ProgressAware => "进阶模式（基于完成进度）",
        }
    }

    /// 从用户配置推导模式。
    ///
    /// 注意判定依据是"凭据是否完整"而非"画像里的 authenticated 标志"——
    /// 前者才是算法能力边界的真实来源。
    pub fn detect(authenticated: bool) -> Self {
        if authenticated {
            Self::ProgressAware
        } else {
            Self::Basic
        }
    }
}

/// 推荐结果集合。
///
/// 可序列化：整份结果会缓存到本地，重启后无需重新计算即可展示
/// （生成时间随结果一同保存，界面据此提示新鲜度）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendationSet {
    pub items: Vec<Recommendation>,
    pub mode: RecommendMode,
    /// 识别出的薄弱标签（含掌握率），供 UI 展示诊断结论。
    pub weak_tags: Vec<WeakTag>,
    /// 若无法生成推荐，此处说明原因。
    pub note: Option<String>,
}

/// 一个薄弱知识点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeakTag {
    pub tag_name: String,
    pub tag_slug: String,
    /// 用户已解题数。
    pub solved: u32,
    /// 题库中该标签的题目总数。
    pub total: u32,
    /// 掌握率 0.0..=1.0。
    pub mastery: f64,
}

/// 推荐引擎。无状态，所有输入通过参数传入。
pub struct RecommendationEngine;

impl RecommendationEngine {
    /// 生成推荐。
    ///
    /// - `problems`：本地题库（含完成状态，未认证时多为 `Unknown`）
    /// - `tag_stats`：用户的标签解题统计（匿名可得）
    /// - `profile`：用户画像，可为 `None`
    /// - `mode`：运行模式
    pub fn recommend(
        problems: &[Problem],
        tag_stats: &[TagProgress],
        profile: Option<&UserProfile>,
        mode: RecommendMode,
        config: &RecommendConfig,
    ) -> RecommendationSet {
        // 前置检查：题库为空时无需继续。
        if problems.is_empty() {
            return RecommendationSet {
                items: Vec::new(),
                mode,
                weak_tags: Vec::new(),
                note: Some("题库为空。请先在首页点击「刷新题库」拉取题目数据。".to_string()),
            };
        }

        // 后置检查：无标签统计时无法定位薄弱项。
        // 但仍可退化为"按难度递进的通用推荐"，只是说服力较弱。
        if tag_stats.is_empty() {
            let items = Self::fallback_recommend(problems, profile, mode, config);
            return RecommendationSet {
                items,
                mode,
                weak_tags: Vec::new(),
                note: Some(
                    "尚未获取到技能分布数据。已按难度递进给出通用推荐；\
                     绑定账号后可获得基于薄弱知识点的精准推荐。"
                        .to_string(),
                ),
            };
        }

        // 步骤 1：统计每个标签在题库中的题目总数。
        let tag_totals = Self::compute_tag_totals(problems);

        // 步骤 2：识别薄弱标签。
        let weak_tags = Self::find_weak_tags(tag_stats, &tag_totals, config.max_weak_tags);

        // 步骤 3：确定难度适配区间（基于用户当前解题构成推算）。
        let target_difficulty = Self::estimate_target_difficulty(profile, tag_stats);

        // 步骤 4：构建标签权重表（薄弱标签 -> 权重）。
        let weights = Self::build_tag_weights(&weak_tags);

        // 步骤 5：为每个候选题目打分。
        let mut scored: Vec<Recommendation> = problems
            .iter()
            .filter(|p| Self::is_candidate(p, mode, config))
            .filter_map(|p| {
                Self::score_problem(p, &weights, &weak_tags, target_difficulty, mode, config)
            })
            .collect();

        // 步骤 6：排序。必须提供完整的次级比较键以保证确定性。
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                // 次级键 1：难度由易到难（同等得分先做简单的）
                .then_with(|| a.problem.difficulty.order().cmp(&b.problem.difficulty.order()))
                // 次级键 2：题号升序（稳定且用户可预期）
                .then_with(|| {
                    a.problem
                        .frontend_id_numeric()
                        .cmp(&b.problem.frontend_id_numeric())
                })
                // 次级键 3：slug 字典序（处理题号相同或非数字题号）
                .then_with(|| a.problem.title_slug.cmp(&b.problem.title_slug))
        });

        scored.truncate(config.top_n);

        let note = if scored.is_empty() {
            Some(
                "没有找到合适的推荐题目。可能原因：题库中的相关题目都已完成，\
                 或筛选条件过严。可在设置页调整推荐偏好。"
                    .to_string(),
            )
        } else {
            None
        };

        RecommendationSet {
            items: scored,
            mode,
            weak_tags,
            note,
        }
    }

    /// 统计题库中各标签的题目总数。
    fn compute_tag_totals(problems: &[Problem]) -> HashMap<String, u32> {
        let mut totals: HashMap<String, u32> = HashMap::new();
        for p in problems {
            for t in &p.tags {
                if !t.slug.is_empty() {
                    *totals.entry(t.slug.clone()).or_insert(0) += 1;
                }
            }
        }
        totals
    }

    /// 识别薄弱标签。
    ///
    /// 判定逻辑：计算每个标签的掌握率，取掌握率最低的一批。
    ///
    /// 关键细节——**过滤样本过少的标签**：若某标签题库中只有 2 道题，
    /// 用户做了 0 道，掌握率算出来是 0%，会排在第一位。但"没做过 2 道题"
    /// 并不说明薄弱，只说明该知识点题目稀少。因此设置最小题量门槛。
    fn find_weak_tags(
        tag_stats: &[TagProgress],
        tag_totals: &HashMap<String, u32>,
        max_count: usize,
    ) -> Vec<WeakTag> {
        /// 标签至少要有这么多题，掌握率才有统计意义。
        const MIN_TAG_SIZE: u32 = 8;

        let mut candidates: Vec<WeakTag> = tag_stats
            .iter()
            .filter_map(|t| {
                let total = *tag_totals.get(&t.tag_slug)?;
                // 题库总量太小，掌握率不可靠。
                if total < MIN_TAG_SIZE {
                    return None;
                }
                // solved 可能因远端数据异常大于 total，需截断。
                let solved = t.solved.min(total);
                Some(WeakTag {
                    tag_name: t.tag_name.clone(),
                    tag_slug: t.tag_slug.clone(),
                    solved,
                    total,
                    mastery: solved as f64 / total as f64,
                })
            })
            .collect();

        // 排序：掌握率升序；掌握率相同则"未做题数多的优先"（潜在收益更大）；
        // 再相同则按 slug 升序（确定性）。
        candidates.sort_by(|a, b| {
            a.mastery
                .partial_cmp(&b.mastery)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| (b.total - b.solved).cmp(&(a.total - a.solved)))
                .then_with(|| a.tag_slug.cmp(&b.tag_slug))
        });

        candidates.truncate(max_count);
        candidates
    }

    /// 推估用户的"目标难度"。
    ///
    /// 逻辑：看用户当前解题构成中占比最高的一档，作为推荐难度中心。
    /// 例如解出的题里 Medium 最多，那么推荐应偏向 Medium。
    ///
    /// 若无画像数据，默认 Medium（多数用户的主战场）。
    fn estimate_target_difficulty(
        profile: Option<&UserProfile>,
        tag_stats: &[TagProgress],
    ) -> Difficulty {
        let counts = match profile {
            Some(p) if p.solved.total() > 0 => p.solved,
            _ => {
                // 无画像时用标签统计总量粗略推断：
                // 解题总数少 -> 偏 Easy；多 -> 偏 Medium/Hard。
                let total: u32 = tag_stats.iter().map(|t| t.solved).sum();
                return match total {
                    0..=30 => Difficulty::Easy,
                    31..=200 => Difficulty::Medium,
                    _ => Difficulty::Hard,
                };
            }
        };

        // 找出占比最高的一档。
        let mut best = (Difficulty::Medium, counts.medium);
        if counts.easy > best.1 {
            best = (Difficulty::Easy, counts.easy);
        }
        if counts.hard > best.1 {
            best = (Difficulty::Hard, counts.hard);
        }
        best.0
    }

    /// 构建标签权重表：掌握率越低，权重越高。
    ///
    /// 使用 `(1 - mastery) + 基础值` 而非 `1/mastery`：后者在 mastery 接近
    /// 0 时会爆炸性放大，让某单一标签完全主导推荐。加基础值保证即使是
    /// 相当熟悉的标签也有一定推荐机会。
    fn build_tag_weights(weak_tags: &[WeakTag]) -> HashMap<String, f64> {
        weak_tags
            .iter()
            .map(|t| {
                let w = (1.0 - t.mastery) + 0.3;
                (t.tag_slug.clone(), w)
            })
            .collect()
    }

    /// 判断某题是否进入候选池。
    fn is_candidate(p: &Problem, mode: RecommendMode, config: &RecommendConfig) -> bool {
        if config.exclude_paid_only && p.is_paid_only {
            return false;
        }

        // 仅在增强模式下才可能可靠判定"已完成"。
        // 基础模式下 status 几乎全是 Unknown，此时执行排除会把整个题库清空。
        if config.exclude_solved && mode == RecommendMode::ProgressAware && p.status.is_solved() {
            return false;
        }

        true
    }

    /// 为单道题目打分。
    ///
    /// 返回 `None` 表示该题没有命中任何薄弱标签，不值得推荐
    /// （在标签驱动模式下，推荐必须与用户的薄弱项相关）。
    fn score_problem(
        p: &Problem,
        weights: &HashMap<String, f64>,
        weak_tags: &[WeakTag],
        target: Difficulty,
        mode: RecommendMode,
        config: &RecommendConfig,
    ) -> Option<Recommendation> {
        // 计算命中的薄弱标签。
        let mut matched: Vec<&WeakTag> = Vec::new();
        let mut tag_score = 0.0;

        for t in &p.tags {
            if let Some(w) = weights.get(&t.slug) {
                tag_score += w;
                // 找出对应的 WeakTag 以便生成理由。
                if let Some(wt) = weak_tags.iter().find(|x| x.tag_slug == t.slug) {
                    matched.push(wt);
                }
            }
        }

        if matched.is_empty() {
            return None;
        }

        // 多标签命中给予轻微加成，但避免线性叠加导致"标签多的题"通吃。
        // 用 sqrt 做次线性压缩。
        let base = tag_score.sqrt();

        // 难度适配：与目标难度越接近得分越高。
        // 差值 0 -> 1.0；差值 1 -> 0.75；差值 2 -> 0.5
        let diff_gap = (p.difficulty.order() as i32 - target.order() as i32).unsigned_abs();
        let difficulty_factor = match diff_gap {
            0 => 1.0,
            1 => 0.75,
            _ => 0.5,
        };

        // 通过率因子：过低的通过率（<15%）往往是偏题怪题或需要特殊技巧，
        // 对学习者的边际收益低。过高的通过率（>75%）说明是基础题，
        // 挑战性不足。中间区间最优。
        let ac = p.ac_rate.clamp(0.0, 100.0);
        let ac_factor = if ac < 15.0 {
            0.6
        } else if ac > 75.0 {
            0.7
        } else {
            1.0
        };

        // 状态加成：仅在增强模式下有意义。
        let status_factor = match (mode, p.status) {
            (RecommendMode::ProgressAware, SolveStatus::Attempted) => config.attempted_boost,
            // 已通过的题不应出现在推荐中（已由 is_candidate 过滤），
            // 此处保留兜底：万一该题因其他原因进入，不应获得加成。
            (RecommendMode::ProgressAware, SolveStatus::Solved) => 0.0,
            _ => 1.0,
        };

        if status_factor == 0.0 {
            return None;
        }

        let score = base * difficulty_factor * ac_factor * status_factor;

        // 生成人类可读的理由。
        let mut reasons = Vec::new();
        matched.sort_by(|a, b| {
            a.mastery
                .partial_cmp(&b.mastery)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.tag_slug.cmp(&b.tag_slug))
        });
        for wt in matched.iter().take(2) {
            let pct = (wt.mastery * 100.0).round() as u32;
            reasons.push(format!(
                "知识点「{}」掌握度较低（{}/{} 题，约 {}%）",
                wt.tag_name, wt.solved, wt.total, pct
            ));
        }
        if let SolveStatus::Attempted = p.status {
            if mode == RecommendMode::ProgressAware {
                reasons.push("你曾尝试过这道题但尚未通过，值得再攻一次".to_string());
            }
        }
        reasons.push(format!(
            "难度「{}」与当前水平匹配，全站通过率 {:.1}%",
            p.difficulty.label_zh(),
            ac
        ));

        Some(Recommendation {
            problem: p.clone(),
            score,
            reasons,
            matched_tags: matched.iter().map(|t| t.tag_name.clone()).collect(),
        })
    }

    /// 无标签统计时的降级推荐：按难度递进给出通用题目。
    ///
    /// 策略：从目标难度开始，优先推荐该难度中通过率适中的题目。
    /// 这不是"个性化推荐"，但比什么都不给强，且明确告知用户数据不足。
    fn fallback_recommend(
        problems: &[Problem],
        profile: Option<&UserProfile>,
        mode: RecommendMode,
        config: &RecommendConfig,
    ) -> Vec<Recommendation> {
        let target = Self::estimate_target_difficulty(profile, &[]);

        let mut items: Vec<Recommendation> = problems
            .iter()
            .filter(|p| Self::is_candidate(p, mode, config))
            .filter(|p| p.difficulty == target)
            .map(|p| {
                let ac = p.ac_rate.clamp(0.0, 100.0);
                // 通过率越接近 50%，通常越适合作为练习（既有挑战又能做出来）。
                let score = 1.0 - ((ac - 50.0).abs() / 50.0) * 0.5;
                Recommendation {
                    problem: p.clone(),
                    score,
                    reasons: vec![format!(
                        "「{}」难度的通用推荐，全站通过率 {:.1}%",
                        p.difficulty.label_zh(),
                        ac
                    )],
                    matched_tags: Vec::new(),
                }
            })
            .collect();

        items.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.problem
                        .frontend_id_numeric()
                        .cmp(&b.problem.frontend_id_numeric())
                })
                .then_with(|| a.problem.title_slug.cmp(&b.problem.title_slug))
        });

        items.truncate(config.top_n);
        items
    }

    /// 生成用于 LLM 的候选题目清单文本。
    ///
    /// 只传前若干条，控制 token 消耗。
    pub fn recommendations_for_llm(items: &[Recommendation], limit: usize) -> String {
        if items.is_empty() {
            return "（无推荐题目）".to_string();
        }
        let mut out = String::new();
        for (i, r) in items.iter().take(limit).enumerate() {
            out.push_str(&format!(
                "{}. [{}. {}] {}（{}，通过率 {:.1}%）\n",
                i + 1,
                r.problem.frontend_id,
                r.problem.title,
                r.problem.difficulty.label_zh(),
                r.problem.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join("/"),
                r.problem.ac_rate
            ));
        }
        out
    }
}

/// 统计用户已解题目涉及的知识点覆盖面。
///
/// 供 UI 展示"你的知识图谱覆盖率"。
pub fn knowledge_coverage(tag_stats: &[TagProgress]) -> (usize, usize) {
    let covered = tag_stats.iter().filter(|t| t.solved > 0).count();
    (covered, tag_stats.len())
}

/// 找出用户完全不熟悉的知识点（解出 0 题但题库中有足量题目）。
pub fn untouched_tags(tag_stats: &[TagProgress], min_size: u32) -> Vec<&TagProgress> {
    let mut v: Vec<&TagProgress> = tag_stats
        .iter()
        .filter(|t| t.solved == 0 && t.total >= min_size)
        .collect();
    v.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.tag_slug.cmp(&b.tag_slug)));
    v
}

/// 去重：同一标签只保留一次（防御远端数据重复）。
pub fn dedupe_tags(tags: &mut Vec<TagProgress>) {
    let mut seen: HashSet<String> = HashSet::new();
    tags.retain(|t| seen.insert(t.tag_slug.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DifficultyCounts, TopicTag};

    fn mk_problem(
        slug: &str,
        id: &str,
        difficulty: Difficulty,
        ac: f64,
        tags: &[(&str, &str)],
        status: SolveStatus,
    ) -> Problem {
        Problem {
            question_id: id.to_string(),
            frontend_id: id.to_string(),
            title: format!("Problem {slug}"),
            title_slug: slug.to_string(),
            difficulty,
            ac_rate: ac,
            tags: tags
                .iter()
                .map(|(n, s)| TopicTag {
                    name: n.to_string(),
                    slug: s.to_string(),
                })
                .collect(),
            status,
            is_paid_only: false,
        }
    }

    /// 构造一个题库：array 和 dp 各有足够多的题以通过最小题量门槛。
    fn mk_problem_set() -> Vec<Problem> {
        let mut v = Vec::new();
        for i in 0..12 {
            v.push(mk_problem(
                &format!("array-{i}"),
                &format!("{}", 100 + i),
                Difficulty::Easy,
                60.0,
                &[("Array", "array")],
                SolveStatus::Unknown,
            ));
        }
        for i in 0..12 {
            v.push(mk_problem(
                &format!("dp-{i}"),
                &format!("{}", 200 + i),
                Difficulty::Medium,
                45.0,
                &[("Dynamic Programming", "dynamic-programming")],
                SolveStatus::Unknown,
            ));
        }
        v
    }

    /// 用户在 array 上很熟、在 dp 上很弱。
    fn mk_tag_stats() -> Vec<TagProgress> {
        vec![
            TagProgress {
                tag_name: "Array".into(),
                tag_slug: "array".into(),
                solved: 12,
                total: 12,
            },
            TagProgress {
                tag_name: "Dynamic Programming".into(),
                tag_slug: "dynamic-programming".into(),
                solved: 1,
                total: 12,
            },
        ]
    }

    #[test]
    fn recommends_problems_from_weak_tags_not_strong_ones() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );

        assert!(!set.items.is_empty());
        // 用户 DP 弱，推荐应以 DP 题为主。
        let dp_count = set
            .items
            .iter()
            .filter(|r| r.problem.title_slug.starts_with("dp-"))
            .count();
        assert!(
            dp_count > set.items.len() / 2,
            "推荐应偏向薄弱知识点 DP，实得 {dp_count}/{}",
            set.items.len()
        );
    }

    #[test]
    fn weak_tags_are_sorted_by_mastery_ascending() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert_eq!(set.weak_tags.len(), 2);
        assert_eq!(set.weak_tags[0].tag_slug, "dynamic-programming");
        assert!(set.weak_tags[0].mastery < set.weak_tags[1].mastery);
    }

    #[test]
    fn tiny_tags_are_excluded_from_weak_list() {
        // 一个只有 2 道题的标签，掌握率 0%，但不应被视为"薄弱"。
        let mut problems = mk_problem_set();
        for i in 0..2 {
            problems.push(mk_problem(
                &format!("niche-{i}"),
                &format!("{}", 300 + i),
                Difficulty::Hard,
                20.0,
                &[("Niche Topic", "niche-topic")],
                SolveStatus::Unknown,
            ));
        }
        let mut stats = mk_tag_stats();
        stats.push(TagProgress {
            tag_name: "Niche Topic".into(),
            tag_slug: "niche-topic".into(),
            solved: 0,
            total: 2,
        });

        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert!(
            !set.weak_tags.iter().any(|t| t.tag_slug == "niche-topic"),
            "样本量不足的标签不应进入薄弱列表"
        );
    }

    #[test]
    fn progress_aware_mode_excludes_solved_problems() {
        let mut problems = mk_problem_set();
        // 把一半 DP 题标记为已完成。
        for p in problems.iter_mut() {
            let is_dp_sample = p.title_slug.starts_with("dp-0") || p.title_slug.starts_with("dp-1");
            let keep_unsolved = p.title_slug == "dp-10" || p.title_slug == "dp-11";
            if is_dp_sample && !keep_unsolved {
                p.status = SolveStatus::Solved;
            }
        }

        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::ProgressAware,
            &RecommendConfig::default(),
        );

        assert!(
            set.items.iter().all(|r| !r.problem.status.is_solved()),
            "已完成的题目不应出现在推荐中"
        );
    }

    #[test]
    fn basic_mode_does_not_exclude_unknown_status_problems() {
        // 这是关键回归测试：未认证时所有题 status 都是 Unknown，
        // 若把它们当作"已完成"排除掉，推荐结果会变成空列表。
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert!(
            !set.items.is_empty(),
            "基础模式下不应因 Unknown 状态而清空推荐"
        );
    }

    #[test]
    fn attempted_problems_get_boosted_in_progress_aware_mode() {
        let mut problems = vec![
            mk_problem(
                "dp-attempted",
                "1",
                Difficulty::Medium,
                45.0,
                &[("Dynamic Programming", "dynamic-programming")],
                SolveStatus::Attempted,
            ),
            mk_problem(
                "dp-fresh",
                "2",
                Difficulty::Medium,
                45.0,
                &[("Dynamic Programming", "dynamic-programming")],
                SolveStatus::Unknown,
            ),
        ];
        // 补足标签题量门槛。
        for i in 0..10 {
            problems.push(mk_problem(
                &format!("dp-fill-{i}"),
                &format!("{}", 10 + i),
                Difficulty::Medium,
                45.0,
                &[("Dynamic Programming", "dynamic-programming")],
                SolveStatus::Unknown,
            ));
        }

        let stats = vec![TagProgress {
            tag_name: "Dynamic Programming".into(),
            tag_slug: "dynamic-programming".into(),
            solved: 0,
            total: 12,
        }];

        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::ProgressAware,
            &RecommendConfig::default(),
        );

        // 尝试过的题应排在首位（其余条件相同）。
        assert_eq!(set.items[0].problem.title_slug, "dp-attempted");
        assert!(
            set.items[0]
                .reasons
                .iter()
                .any(|r| r.contains("曾尝试过")),
            "应在理由中说明这是重做题"
        );
    }

    #[test]
    fn paid_only_problems_excluded_when_configured() {
        let mut problems = mk_problem_set();

        // 会员题挂在"用户最薄弱"的标签上（dp：1/12），与其余 dp 题得分相同。
        // 因此把 top_n 设为足够大，让所有候选都能进入结果——
        // 这样断言考察的就是"过滤逻辑"本身，而不是被 top_n 截断的
        // 排序细节（后者会随 tie-break 规则变动而假性失败）。
        let mut paid = mk_problem(
            "dp-paid",
            "999",
            Difficulty::Medium,
            45.0,
            &[("Dynamic Programming", "dynamic-programming")],
            SolveStatus::Unknown,
        );
        paid.is_paid_only = true;
        problems.push(paid);

        let stats = mk_tag_stats();
        let total = problems.len();

        // 开启排除：结果中不得出现会员题。
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig {
                exclude_paid_only: true,
                top_n: total,
                ..Default::default()
            },
        );
        assert!(
            set.items.iter().all(|r| !r.problem.is_paid_only),
            "开启排除会员题后，结果中不得出现会员题"
        );

        // 关闭排除：会员题必须出现。
        let set2 = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig {
                exclude_paid_only: false,
                top_n: total,
                ..Default::default()
            },
        );
        assert!(
            set2.items.iter().any(|r| r.problem.is_paid_only),
            "关闭排除后，会员题应可进入推荐（实得 {} 条 / 共 {} 候选）",
            set2.items.len(),
            total
        );
    }

    #[test]
    fn determinism_same_input_same_output() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let cfg = RecommendConfig::default();

        let a = RecommendationEngine::recommend(&problems, &stats, None, RecommendMode::Basic, &cfg);
        let b = RecommendationEngine::recommend(&problems, &stats, None, RecommendMode::Basic, &cfg);

        let slugs_a: Vec<&str> = a.items.iter().map(|r| r.problem.title_slug.as_str()).collect();
        let slugs_b: Vec<&str> = b.items.iter().map(|r| r.problem.title_slug.as_str()).collect();
        assert_eq!(slugs_a, slugs_b, "同输入必须产生同输出顺序");
    }

    #[test]
    fn determinism_holds_regardless_of_input_order() {
        // 题目顺序变化不应影响推荐结果（内部排序必须是全序）。
        let problems = mk_problem_set();
        let mut shuffled = problems.clone();
        shuffled.reverse();

        let stats = mk_tag_stats();
        let cfg = RecommendConfig::default();

        let a = RecommendationEngine::recommend(&problems, &stats, None, RecommendMode::Basic, &cfg);
        let b = RecommendationEngine::recommend(&shuffled, &stats, None, RecommendMode::Basic, &cfg);

        let slugs_a: Vec<&str> = a.items.iter().map(|r| r.problem.title_slug.as_str()).collect();
        let slugs_b: Vec<&str> = b.items.iter().map(|r| r.problem.title_slug.as_str()).collect();
        assert_eq!(slugs_a, slugs_b, "输入顺序不应影响推荐顺序");
    }

    #[test]
    fn empty_problem_set_yields_actionable_note() {
        let set = RecommendationEngine::recommend(
            &[],
            &mk_tag_stats(),
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert!(set.items.is_empty());
        let note = set.note.unwrap();
        assert!(note.contains("题库为空"), "应给出可操作的提示: {note}");
    }

    #[test]
    fn empty_tag_stats_falls_back_to_difficulty_based() {
        let problems = mk_problem_set();
        let set = RecommendationEngine::recommend(
            &problems,
            &[],
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert!(!set.items.is_empty(), "应退化为难度递进推荐而非返回空");
        assert!(set.weak_tags.is_empty());
        assert!(set.note.unwrap().contains("技能分布"));
    }

    #[test]
    fn all_problems_solved_yields_note_not_panic() {
        let mut problems = mk_problem_set();
        for p in problems.iter_mut() {
            p.status = SolveStatus::Solved;
        }
        let set = RecommendationEngine::recommend(
            &problems,
            &mk_tag_stats(),
            None,
            RecommendMode::ProgressAware,
            &RecommendConfig::default(),
        );
        assert!(set.items.is_empty());
        assert!(set.note.is_some(), "全部完成时应给出说明而非静默返回空");
    }

    #[test]
    fn top_n_limit_is_respected() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let cfg = RecommendConfig {
            top_n: 3,
            ..Default::default()
        };
        let set =
            RecommendationEngine::recommend(&problems, &stats, None, RecommendMode::Basic, &cfg);
        assert_eq!(set.items.len(), 3);
    }

    #[test]
    fn every_recommendation_has_at_least_one_reason() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        for r in &set.items {
            assert!(!r.reasons.is_empty(), "推荐 {} 缺少理由", r.problem.title_slug);
            assert!(
                r.score.is_finite() && r.score > 0.0,
                "分数必须为有限正值，实得 {}",
                r.score
            );
        }
    }

    #[test]
    fn target_difficulty_follows_user_distribution() {
        // 用户解出的大多是 Hard -> 目标难度应为 Hard。
        let profile = UserProfile {
            solved: DifficultyCounts {
                easy: 5,
                medium: 20,
                hard: 60,
            },
            ..Default::default()
        };
        let d = RecommendationEngine::estimate_target_difficulty(Some(&profile), &[]);
        assert_eq!(d, Difficulty::Hard);

        let profile2 = UserProfile {
            solved: DifficultyCounts {
                easy: 60,
                medium: 10,
                hard: 2,
            },
            ..Default::default()
        };
        assert_eq!(
            RecommendationEngine::estimate_target_difficulty(Some(&profile2), &[]),
            Difficulty::Easy
        );
    }

    #[test]
    fn target_difficulty_defaults_to_medium_without_data() {
        assert_eq!(
            RecommendationEngine::estimate_target_difficulty(None, &[]),
            Difficulty::Easy,
            "零数据时应从 Easy 起步"
        );
        let small = vec![TagProgress {
            tag_name: "X".into(),
            tag_slug: "x".into(),
            solved: 10,
            total: 100,
        }];
        assert_eq!(
            RecommendationEngine::estimate_target_difficulty(None, &small),
            Difficulty::Easy
        );
        let large = vec![TagProgress {
            tag_name: "X".into(),
            tag_slug: "x".into(),
            solved: 500,
            total: 1000,
        }];
        assert_eq!(
            RecommendationEngine::estimate_target_difficulty(None, &large),
            Difficulty::Hard
        );
    }

    #[test]
    fn score_is_finite_with_extreme_ac_rates() {
        let problems = vec![
            mk_problem(
                "p0",
                "1",
                Difficulty::Medium,
                0.0,
                &[("DP", "dp")],
                SolveStatus::Unknown,
            ),
            mk_problem(
                "p1",
                "2",
                Difficulty::Medium,
                100.0,
                &[("DP", "dp")],
                SolveStatus::Unknown,
            ),
        ];
        let mut all = problems;
        for i in 2..12 {
            all.push(mk_problem(
                &format!("p{i}"),
                &format!("{i}"),
                Difficulty::Medium,
                50.0,
                &[("DP", "dp")],
                SolveStatus::Unknown,
            ));
        }
        let stats = vec![TagProgress {
            tag_name: "DP".into(),
            tag_slug: "dp".into(),
            solved: 0,
            total: 12,
        }];
        let set = RecommendationEngine::recommend(
            &all,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        for r in &set.items {
            assert!(r.score.is_finite(), "极端通过率不应产生 NaN/Inf");
        }
    }

    #[test]
    fn mastery_never_exceeds_one_when_solved_exceeds_total() {
        // 远端数据异常：已解题数大于题库总量。
        let problems = mk_problem_set();
        let stats = vec![TagProgress {
            tag_name: "Array".into(),
            tag_slug: "array".into(),
            solved: 999, // 异常值
            total: 12,
        }];
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        for t in &set.weak_tags {
            assert!(t.mastery <= 1.0, "掌握率必须被截断到 1.0");
        }
    }

    #[test]
    fn mode_detection_follows_auth_state() {
        assert_eq!(
            RecommendMode::detect(false),
            RecommendMode::Basic
        );
        assert_eq!(
            RecommendMode::detect(true),
            RecommendMode::ProgressAware
        );
    }




    #[test]
    fn recommendations_for_llm_formats_and_limits() {
        let problems = mk_problem_set();
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        let text = RecommendationEngine::recommendations_for_llm(&set.items, 3);
        assert_eq!(text.lines().count(), 3, "应只输出前 3 条");
        assert!(text.contains("1."), "应有编号");
    }

    #[test]
    fn recommendations_for_llm_handles_empty() {
        assert_eq!(
            RecommendationEngine::recommendations_for_llm(&[], 5),
            "（无推荐题目）"
        );
    }

    #[test]
    fn knowledge_coverage_counts_correctly() {
        let stats = vec![
            TagProgress {
                tag_name: "A".into(),
                tag_slug: "a".into(),
                solved: 5,
                total: 10,
            },
            TagProgress {
                tag_name: "B".into(),
                tag_slug: "b".into(),
                solved: 0,
                total: 10,
            },
        ];
        assert_eq!(knowledge_coverage(&stats), (1, 2));
    }

    #[test]
    fn untouched_tags_filters_and_sorts_by_size() {
        let stats = vec![
            TagProgress {
                tag_name: "Small".into(),
                tag_slug: "small".into(),
                solved: 0,
                total: 3,
            },
            TagProgress {
                tag_name: "Big".into(),
                tag_slug: "big".into(),
                solved: 0,
                total: 50,
            },
            TagProgress {
                tag_name: "Touched".into(),
                tag_slug: "touched".into(),
                solved: 1,
                total: 50,
            },
        ];
        let v = untouched_tags(&stats, 8);
        assert_eq!(v.len(), 1, "只有 Big 满足条件");
        assert_eq!(v[0].tag_slug, "big");
    }

    #[test]
    fn dedupe_tags_removes_duplicates_keeping_first() {
        let mut v = vec![
            TagProgress {
                tag_name: "First".into(),
                tag_slug: "dup".into(),
                solved: 1,
                total: 10,
            },
            TagProgress {
                tag_name: "Second".into(),
                tag_slug: "dup".into(),
                solved: 5,
                total: 10,
            },
        ];
        dedupe_tags(&mut v);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].tag_name, "First");
    }

    #[test]
    fn problems_without_matching_tags_are_not_recommended() {
        let mut problems = mk_problem_set();
        // 添加一道标签完全不在用户统计里的题。
        problems.push(mk_problem(
            "unrelated",
            "999",
            Difficulty::Medium,
            50.0,
            &[("Obscure", "obscure")],
            SolveStatus::Unknown,
        ));
        let stats = mk_tag_stats();
        let set = RecommendationEngine::recommend(
            &problems,
            &stats,
            None,
            RecommendMode::Basic,
            &RecommendConfig::default(),
        );
        assert!(
            !set.items.iter().any(|r| r.problem.title_slug == "unrelated"),
            "未命中薄弱标签的题不应被推荐"
        );
    }
}
