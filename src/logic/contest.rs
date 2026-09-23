//! 竞赛记录分析。
//!
//! ## 分析维度
//!
//! 1. **趋势**：Rating 随时间的走向（上升 / 停滞 / 下滑）
//! 2. **稳定性**：单场表现波动程度（标准差 + 极差），区分"稳步提升"与
//!    "大起大落"
//! 3. **解题率**：平均解出题数占总题数的比例，反映"能否做完"
//! 4. **活跃度**：参赛频率与间隔，反映投入程度
//!
//! ## 为什么不用简单平均
//!
//! Rating 是累积量，单纯的平均值会被早期低分场次拖累，产生"退步"的
//! 错觉。因此趋势判断采用**前半段 vs 后半段对比**：把参赛历史按时间
//! 一分为二，比较两段的平均 Rating。这比"首场 vs 末场"稳健得多
//! （不会被单场异常值主导）。
//!
//! ## 数据前提
//!
//! `ContestRecord` 只包含**实际参赛**的场次（`attended: true`），
//! 该过滤在 DTO 转换层已完成（见 `leetcode::types::ContestHistoryData`）。

use crate::models::{ContestRecord, ContestSummary};

/// 趋势方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// 明显上升。
    Improving,
    /// 基本持平。
    Stable,
    /// 明显下滑。
    Declining,
    /// 数据不足，无法判断。
    Insufficient,
}

impl Trend {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Improving => "稳步上升",
            Self::Stable => "基本持平",
            Self::Declining => "有所下滑",
            Self::Insufficient => "数据不足",
        }
    }

    /// 用于 UI 的符号指示。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Improving => "↗",
            Self::Stable => "→",
            Self::Declining => "↘",
            Self::Insufficient => "?",
        }
    }
}

/// 稳定性评级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// 波动很小。
    Consistent,
    /// 波动中等。
    Moderate,
    /// 波动较大。
    Volatile,
    /// 数据不足。
    Insufficient,
}

impl Stability {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Consistent => "发挥稳定",
            Self::Moderate => "波动中等",
            Self::Volatile => "起伏较大",
            Self::Insufficient => "数据不足",
        }
    }
}

/// 完整的竞赛分析结果。
#[derive(Debug, Clone)]
pub struct ContestAnalysis {
    /// 有效参赛场次（已过滤未参赛）。
    pub attended_count: usize,
    /// 总体排名摘要。
    pub summary: ContestSummary,
    /// Rating 趋势方向。
    pub trend: Trend,
    /// Rating 变化量（后半段均值 - 前半段均值）。
    pub rating_delta: f64,
    /// 最近一次 Rating。
    pub latest_rating: Option<f64>,
    /// 最高 Rating 及其场次。
    pub peak: Option<(f64, ContestRecord)>,
    /// 平均 Rating。
    pub average_rating: Option<f64>,
    /// Rating 标准差（波动性）。
    pub rating_stddev: Option<f64>,
    /// 稳定性评级。
    pub stability: Stability,
    /// 平均解题率 0.0..=1.0。
    pub average_solve_ratio: Option<f64>,
    /// 全对的场次数量。
    pub perfect_rounds: usize,
    /// 平均名次百分位（越小越好）。
    pub average_percentile: Option<f64>,
    /// 文字总结要点。
    pub insights: Vec<String>,
}

impl ContestAnalysis {
    /// 分析竞赛记录。
    ///
    /// `records` 应已按时间升序排列（`ContestHistoryData::into_records`
    /// 保证这一点）。本函数内部会再做一次防御性排序。
    pub fn analyze(records: &[ContestRecord], summary: ContestSummary) -> Self {
        // 防御性排序：即使调用方传入乱序数据也能正确分析。
        let mut recs: Vec<ContestRecord> = records.iter().filter(|r| r.attended).cloned().collect();
        recs.sort_by_key(|r| r.start_time);

        if recs.is_empty() {
            return Self {
                attended_count: 0,
                summary,
                trend: Trend::Insufficient,
                rating_delta: 0.0,
                latest_rating: None,
                peak: None,
                average_rating: None,
                rating_stddev: None,
                stability: Stability::Insufficient,
                average_solve_ratio: None,
                perfect_rounds: 0,
                average_percentile: None,
                insights: vec![
                    "暂无竞赛记录。参加周赛或双周赛可以更好地检验学习效果，建议先积累 2-3 场数据。"
                        .to_string(),
                ],
            };
        }

        let ratings: Vec<f64> = recs.iter().map(|r| r.rating).collect();
        let latest_rating = ratings.last().copied();
        let average_rating = Some(mean(&ratings));
        let rating_stddev = stddev(&ratings);

        // 峰值：找出 Rating 最高的场次。用最大值的下标定位。
        let peak = recs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.rating
                    .partial_cmp(&b.rating)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, r)| (r.rating, r.clone()));

        // 趋势：前后半段均值对比。
        let (trend, rating_delta) = Self::analyze_trend(&ratings);

        // 稳定性评级。
        let stability = Self::classify_stability(rating_stddev, ratings.len());

        // 解题率：只统计总题数已知的场次。
        let solve_ratios: Vec<f64> = recs
            .iter()
            .filter(|r| r.total_problems > 0)
            .map(|r| r.problems_solved as f64 / r.total_problems as f64)
            .collect();
        let average_solve_ratio = if solve_ratios.is_empty() {
            None
        } else {
            Some(mean(&solve_ratios))
        };

        // 全对场次：解题数等于总题数（且总题数大于 0）。
        let perfect_rounds = recs
            .iter()
            .filter(|r| r.total_problems > 0 && r.problems_solved >= r.total_problems)
            .count();

        // 名次百分位。
        let percentiles: Vec<f64> = recs.iter().filter_map(|r| r.percentile()).collect();
        let average_percentile = if percentiles.is_empty() {
            None
        } else {
            Some(mean(&percentiles))
        };

        let insights = Self::build_insights(
            &recs,
            trend,
            rating_delta,
            stability,
            average_rating,
            latest_rating,
            peak.as_ref().map(|(r, _)| *r),
            average_solve_ratio,
            perfect_rounds,
            average_percentile,
        );

        Self {
            attended_count: recs.len(),
            summary,
            trend,
            rating_delta,
            latest_rating,
            peak,
            average_rating,
            rating_stddev,
            stability,
            average_solve_ratio,
            perfect_rounds,
            average_percentile,
            insights,
        }
    }

    /// 判断 Rating 趋势。
    ///
    /// 把历史按时间对半切，比较两段均值。
    ///
    /// 判定阈值取 ±25 分：LeetCode 单场 Rating 波动通常在 ±50 以上，
    /// 若阈值取得太小，正常波动会被误判为趋势。25 分意味着需要持续性
    /// 的表现变化才能触发趋势判定。
    ///
    /// 少于 3 场时不做趋势判断（样本太少，任何结论都不可靠）。
    fn analyze_trend(ratings: &[f64]) -> (Trend, f64) {
        /// 触发趋势判定的最小 Rating 变化。
        const TREND_THRESHOLD: f64 = 25.0;

        if ratings.len() < 3 {
            return (Trend::Insufficient, 0.0);
        }

        let mid = ratings.len() / 2;
        let first_half = &ratings[..mid];
        let second_half = &ratings[mid..];

        // 两段都不为空（len >= 3 保证 mid >= 1 且后半段非空）。
        if first_half.is_empty() || second_half.is_empty() {
            return (Trend::Insufficient, 0.0);
        }

        let delta = mean(second_half) - mean(first_half);

        let trend = if delta > TREND_THRESHOLD {
            Trend::Improving
        } else if delta < -TREND_THRESHOLD {
            Trend::Declining
        } else {
            Trend::Stable
        };

        (trend, delta)
    }

    /// 根据标准差评级稳定性。
    ///
    /// 阈值依据：LeetCode Rating 的理论波动范围约为 ±150。标准差在
    /// 40 以内说明发挥平稳；超过 90 说明大起大落。
    fn classify_stability(stddev: Option<f64>, count: usize) -> Stability {
        // 少于 3 场时标准差没有统计意义。
        if count < 3 {
            return Stability::Insufficient;
        }
        match stddev {
            None => Stability::Insufficient,
            Some(s) if s < 40.0 => Stability::Consistent,
            Some(s) if s < 90.0 => Stability::Moderate,
            Some(_) => Stability::Volatile,
        }
    }

    /// 生成文字总结要点。
    #[allow(clippy::too_many_arguments)]
    fn build_insights(
        recs: &[ContestRecord],
        trend: Trend,
        rating_delta: f64,
        stability: Stability,
        average_rating: Option<f64>,
        latest_rating: Option<f64>,
        peak_rating: Option<f64>,
        average_solve_ratio: Option<f64>,
        perfect_rounds: usize,
        average_percentile: Option<f64>,
    ) -> Vec<String> {
        let mut v = Vec::new();

        // 1. 参赛规模。
        v.push(format!("共参加 {} 场竞赛。", recs.len()));

        // 2. 当前水平。
        if let (Some(latest), Some(avg)) = (latest_rating, average_rating) {
            let vs_avg = latest - avg;
            let cmp = if vs_avg.abs() < 20.0 {
                "与个人平均水平相当".to_string()
            } else if vs_avg > 0.0 {
                format!("高于个人平均 {:.0} 分", vs_avg)
            } else {
                format!("低于个人平均 {:.0} 分", -vs_avg)
            };
            v.push(format!(
                "当前 Rating 约 {:.0}，平均 {:.0}，{cmp}。",
                latest, avg
            ));
        }

        // 3. 趋势。
        match trend {
            Trend::Improving => v.push(format!(
                "近期 Rating 呈上升趋势（后半段较前半段平均 +{:.0} 分），进步明显，保持当前节奏。",
                rating_delta
            )),
            Trend::Declining => v.push(format!(
                "近期 Rating 有所回落（后半段较前半段平均 {:.0} 分）。建议复盘失分场次，\
                 集中攻克一到两个薄弱知识点，而不是盲目加大刷题量。",
                rating_delta
            )),
            Trend::Stable => v.push(
                "Rating 基本持平，处于平台期。若想突破，可尝试系统性地补齐薄弱知识点，\
                 或提高中等难度题的解题速度。"
                    .to_string(),
            ),
            Trend::Insufficient => {
                v.push("参赛场次较少，暂无法判断 Rating 趋势。建议再积累几场数据。".to_string())
            }
        }

        // 4. 稳定性。
        match stability {
            Stability::Consistent => {
                v.push("各场表现稳定，说明基础扎实、临场发挥可控。".to_string())
            }
            Stability::Moderate => {
                v.push("各场表现有一定波动，属于正常范围。可在赛前做一次限时模拟以稳定手感。".to_string())
            }
            Stability::Volatile => {
                v.push("各场表现起伏较大。建议分析是题目风格差异还是时间分配问题——\
                     波动大通常意味着能力分布不均衡。"
                    .to_string())
            }
            Stability::Insufficient => {}
        }

        // 5. 峰值。
        if let Some(p) = peak_rating {
            if let Some(latest) = latest_rating {
                if p > latest + 50.0 {
                    v.push(format!(
                        "历史最高 Rating 为 {:.0}，当前 {:.0}，距峰值还有 {:.0} 分。",
                        p,
                        latest,
                        p - latest
                    ));
                } else {
                    v.push(format!("历史最高 Rating 为 {:.0}，正处于或接近个人最佳状态。", p));
                }
            }
        }

        // 6. 解题率。
        if let Some(ratio) = average_solve_ratio {
            let pct = ratio * 100.0;
            let comment = if pct >= 75.0 {
                "解题完成度高，主要提升空间在于速度与准确率。"
            } else if pct >= 40.0 {
                "解题完成度中等，通常能拿下前两题。瓶颈多在第三题的难度跨越上，\
                 建议专项练习中等难度题。"
            } else {
                "解题完成度偏低，建议先把简单题做稳，确保前两题快速通过，\
                 再逐步挑战更高难度。"
            };
            v.push(format!("平均每场解出 {:.0}% 的题目。{}", pct, comment));
        }

        if perfect_rounds > 0 {
            v.push(format!(
                "有 {} 场全部解出，说明具备完整解题能力。",
                perfect_rounds
            ));
        }

        // 7. 名次百分位。
        if let Some(p) = average_percentile {
            v.push(format!(
                "平均排名位于参赛者的前 {:.1}%。",
                p
            ));
        }

        v
    }

    /// 生成供 LLM 使用的竞赛分析文本。
    pub fn to_llm_context(&self) -> String {
        if self.attended_count == 0 {
            return "暂无竞赛记录。".to_string();
        }
        let mut s = String::new();
        s.push_str(&format!("竞赛场次：{}\n", self.attended_count));
        if let Some(r) = self.latest_rating {
            s.push_str(&format!("当前 Rating：{:.0}\n", r));
        }
        if let Some(r) = self.average_rating {
            s.push_str(&format!("平均 Rating：{:.0}\n", r));
        }
        s.push_str(&format!(
            "趋势：{}（变化 {:+.0}）\n",
            self.trend.label_zh(),
            self.rating_delta
        ));
        s.push_str(&format!("稳定性：{}\n", self.stability.label_zh()));
        if let Some(p) = self.average_percentile {
            s.push_str(&format!("平均名次百分位：前 {:.1}%\n", p));
        }
        if let Some(r) = self.average_solve_ratio {
            s.push_str(&format!("平均解题率：{:.0}%\n", r * 100.0));
        }
        if let Some((rating, rec)) = &self.peak {
            s.push_str(&format!(
                "最高 Rating：{:.0}（{}）\n",
                rating, rec.title
            ));
        }
        s
    }
}

/// 求均值。空切片返回 0.0（调用方需自行保证非空，或接受 0.0 语义）。
fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// 求样本标准差。少于 2 个数据点时返回 `None`。
///
/// 使用样本标准差（除以 n-1）而非总体标准差：竞赛记录是"抽样"
/// 而非全部可能表现，样本标准差更合适。
fn stddev(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = mean(xs);
    let variance = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    Some(variance.sqrt())
}

/// 计算参赛频率（场/月）。
///
/// 需要至少两场记录。时间跨度为 0 时返回 `None`。
pub fn contests_per_month(records: &[ContestRecord]) -> Option<f64> {
    let mut recs: Vec<&ContestRecord> = records.iter().filter(|r| r.attended).collect();
    if recs.len() < 2 {
        return None;
    }
    recs.sort_by_key(|r| r.start_time);

    let first = recs.first()?.start_time;
    let last = recs.last()?.start_time;
    let span_secs = last - first;
    if span_secs <= 0 {
        return None;
    }

    const SECS_PER_MONTH: f64 = 30.0 * 24.0 * 3600.0;
    let months = span_secs as f64 / SECS_PER_MONTH;
    if months <= 0.0 {
        return None;
    }

    // 场次减 1：跨度覆盖的是 (n-1) 个间隔。
    Some((recs.len() - 1) as f64 / months)
}

/// 找出最长的参赛中断间隔（天）。
pub fn longest_gap_days(records: &[ContestRecord]) -> Option<f64> {
    let mut recs: Vec<&ContestRecord> = records.iter().filter(|r| r.attended).collect();
    if recs.len() < 2 {
        return None;
    }
    recs.sort_by_key(|r| r.start_time);

    let max_gap_secs = recs
        .windows(2)
        .map(|w| (w[1].start_time - w[0].start_time).max(0))
        .max()?;

    Some(max_gap_secs as f64 / 86400.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, start: i64, rating: f64, solved: u32, total: u32) -> ContestRecord {
        ContestRecord {
            title: title.to_string(),
            start_time: start,
            rating,
            ranking: 1000,
            total_participants: 10000,
            problems_solved: solved,
            total_problems: total,
            attended: true,
        }
    }

    #[test]
    fn empty_records_yields_insufficient_and_guidance() {
        let a = ContestAnalysis::analyze(&[], ContestSummary::default());
        assert_eq!(a.attended_count, 0);
        assert_eq!(a.trend, Trend::Insufficient);
        assert_eq!(a.stability, Stability::Insufficient);
        assert_eq!(a.latest_rating, None);
        assert!(!a.insights.is_empty(), "应给出引导性说明");
        assert!(a.insights[0].contains("暂无竞赛记录"));
    }

    #[test]
    fn single_record_has_no_trend_or_stability_judgement() {
        let records = vec![rec("W1", 1000, 1500.0, 3, 4)];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.attended_count, 1);
        assert_eq!(a.trend, Trend::Insufficient, "单场无法判断趋势");
        assert_eq!(a.stability, Stability::Insufficient);
        assert_eq!(a.latest_rating, Some(1500.0));
        assert_eq!(a.average_rating, Some(1500.0));
        assert_eq!(a.rating_stddev, None, "单点无标准差");
        assert_eq!(a.peak.as_ref().unwrap().0, 1500.0);
    }

    #[test]
    fn two_records_still_no_stability_rating() {
        let records = vec![rec("W1", 1000, 1400.0, 2, 4), rec("W2", 2000, 1500.0, 3, 4)];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.stability, Stability::Insufficient, "少于 3 场不评级");
        assert_eq!(a.trend, Trend::Insufficient, "少于 3 场不判趋势");
    }

    #[test]
    fn improving_trend_detected() {
        // 前半段约 1400，后半段约 1700，差值远超 25。
        let records = vec![
            rec("W1", 1000, 1380.0, 2, 4),
            rec("W2", 2000, 1400.0, 2, 4),
            rec("W3", 3000, 1420.0, 3, 4),
            rec("W4", 4000, 1650.0, 3, 4),
            rec("W5", 5000, 1700.0, 4, 4),
            rec("W6", 6000, 1720.0, 4, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.trend, Trend::Improving);
        assert!(a.rating_delta > 25.0);
        assert!(a.insights.iter().any(|s| s.contains("上升趋势")));
    }

    #[test]
    fn declining_trend_detected() {
        let records = vec![
            rec("W1", 1000, 1700.0, 4, 4),
            rec("W2", 2000, 1720.0, 4, 4),
            rec("W3", 3000, 1690.0, 3, 4),
            rec("W4", 4000, 1450.0, 2, 4),
            rec("W5", 5000, 1400.0, 2, 4),
            rec("W6", 6000, 1420.0, 2, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.trend, Trend::Declining);
        assert!(a.rating_delta < -25.0);
        assert!(a.insights.iter().any(|s| s.contains("回落")));
    }

    #[test]
    fn stable_trend_when_change_below_threshold() {
        let records = vec![
            rec("W1", 1000, 1500.0, 3, 4),
            rec("W2", 2000, 1510.0, 3, 4),
            rec("W3", 3000, 1505.0, 3, 4),
            rec("W4", 4000, 1515.0, 3, 4),
            rec("W5", 5000, 1512.0, 3, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.trend, Trend::Stable);
        assert!(a.rating_delta.abs() < 25.0);
        assert!(a.insights.iter().any(|s| s.contains("持平")));
    }

    #[test]
    fn stability_classification_thresholds() {
        // 波动极小 -> Consistent
        let consistent: Vec<ContestRecord> = (0..6)
            .map(|i| rec(&format!("W{i}"), 1000 * i, 1500.0, 3, 4))
            .collect();
        let a = ContestAnalysis::analyze(&consistent, ContestSummary::default());
        assert_eq!(a.stability, Stability::Consistent);
        assert_eq!(a.rating_stddev, Some(0.0));

        // 波动极大 -> Volatile
        let volatile = vec![
            rec("W1", 1000, 1200.0, 1, 4),
            rec("W2", 2000, 1800.0, 4, 4),
            rec("W3", 3000, 1250.0, 1, 4),
            rec("W4", 4000, 1750.0, 4, 4),
            rec("W5", 5000, 1200.0, 1, 4),
        ];
        let b = ContestAnalysis::analyze(&volatile, ContestSummary::default());
        assert_eq!(b.stability, Stability::Volatile);
        assert!(b.rating_stddev.unwrap() > 90.0);
    }

    #[test]
    fn unattended_records_are_filtered_out() {
        let mut records = vec![
            rec("W1", 1000, 1500.0, 3, 4),
            rec("W2", 2000, 1550.0, 3, 4),
        ];
        // 插入一条未参赛记录（不应影响统计）。
        let mut skipped = rec("W-skip", 1500, 9999.0, 0, 4);
        skipped.attended = false;
        records.push(skipped);

        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.attended_count, 2);
        // 未参赛的 9999 分不应污染峰值。
        assert_eq!(a.peak.as_ref().unwrap().0, 1550.0);
        assert_eq!(a.average_rating, Some(1525.0));
    }

    #[test]
    fn peak_calculation_finds_maximum() {
        let records = vec![
            rec("W1", 1000, 1400.0, 2, 4),
            rec("W2", 2000, 1900.0, 4, 4),
            rec("W3", 3000, 1500.0, 3, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        let (rating, r) = a.peak.as_ref().unwrap();
        assert_eq!(*rating, 1900.0);
        assert_eq!(r.title, "W2");
        // 当前低于峰值时应提示差距。
        assert!(a.insights.iter().any(|s| s.contains("距峰值")));
    }

    #[test]
    fn perfect_rounds_counted_only_with_known_total() {
        let records = vec![
            rec("W1", 1000, 1500.0, 4, 4), // 全对
            rec("W2", 2000, 1520.0, 3, 4),
            // total_problems 缺失（为 0）时不应算作全对。
            rec("W3", 3000, 1540.0, 0, 0),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.perfect_rounds, 1);
    }

    #[test]
    fn average_solve_ratio_ignores_unknown_totals() {
        let records = vec![
            rec("W1", 1000, 1500.0, 2, 4), // 50%
            rec("W2", 2000, 1520.0, 4, 4), // 100%
            rec("W3", 3000, 1540.0, 0, 0), // 忽略
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert_eq!(a.average_solve_ratio, Some(0.75));
    }

    #[test]
    fn records_are_sorted_internally() {
        // 传入乱序数据，内部应排序后正确计算。
        let records = vec![
            rec("W3", 3000, 1600.0, 3, 4),
            rec("W1", 1000, 1400.0, 2, 4),
            rec("W2", 2000, 1500.0, 3, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        // 排序后 latest 应为最后一场（W3, 1600）。
        assert_eq!(a.latest_rating, Some(1600.0));
    }

    #[test]
    fn summary_is_passed_through() {
        let records = vec![rec("W1", 1000, 1500.0, 3, 4)];
        let summary = ContestSummary {
            attended_count: 1,
            rating: Some(1500.0),
            global_ranking: Some(50000),
            total_participants: Some(500000),
            top_percentage: Some(10.0),
        };
        let a = ContestAnalysis::analyze(&records, summary.clone());
        assert_eq!(a.summary.global_ranking, Some(50000));
        assert_eq!(a.summary.rating, Some(1500.0));
    }

    #[test]
    fn percentile_computed_across_records() {
        let mut r1 = rec("W1", 1000, 1500.0, 3, 4);
        r1.ranking = 2000;
        r1.total_participants = 10000; // 20%
        let mut r2 = rec("W2", 2000, 1550.0, 3, 4);
        r2.ranking = 1000;
        r2.total_participants = 10000; // 10%

        let a = ContestAnalysis::analyze(&[r1, r2], ContestSummary::default());
        assert_eq!(a.average_percentile, Some(15.0));
        assert!(a.insights.iter().any(|s| s.contains("前 15.0%")));
    }

    #[test]
    fn insights_never_empty_for_nonempty_records() {
        for ratings in [
            vec![1500.0],
            vec![1500.0, 1500.0],
            vec![1200.0, 1800.0, 1200.0, 1800.0],
        ] {
            let records: Vec<ContestRecord> = ratings
                .iter()
                .enumerate()
                .map(|(i, r)| rec(&format!("W{i}"), 1000 * i as i64, *r, 3, 4))
                .collect();
            let a = ContestAnalysis::analyze(&records, ContestSummary::default());
            assert!(!a.insights.is_empty());
        }
    }

    #[test]
    fn llm_context_contains_core_metrics() {
        let records = vec![
            rec("W1", 1000, 1400.0, 2, 4),
            rec("W2", 2000, 1450.0, 3, 4),
            rec("W3", 3000, 1500.0, 3, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        let ctx = a.to_llm_context();
        assert!(ctx.contains("竞赛场次：3"));
        assert!(ctx.contains("当前 Rating：1500"));
        assert!(ctx.contains("趋势"));
        assert!(ctx.contains("稳定性"));
    }

    #[test]
    fn llm_context_for_empty_history() {
        let a = ContestAnalysis::analyze(&[], ContestSummary::default());
        assert_eq!(a.to_llm_context(), "暂无竞赛记录。");
    }

    #[test]
    fn mean_and_stddev_helpers() {
        assert_eq!(mean(&[]), 0.0);
        assert_eq!(mean(&[2.0, 4.0]), 3.0);
        assert_eq!(stddev(&[5.0]), None, "单点无样本标准差");
        assert_eq!(stddev(&[]), None);
        // [2,4,4,4,5,5,7,9] 样本标准差 = 2.138...
        let s = stddev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap();
        assert!((s - 2.1381).abs() < 0.001, "实得 {s}");
        // 完全一致的数据标准差为 0。
        assert_eq!(stddev(&[7.0, 7.0, 7.0]), Some(0.0));
    }

    #[test]
    fn contests_per_month_computation() {
        // 两场相隔 30 天 -> 1 场/月。
        let records = vec![
            rec("W1", 0, 1500.0, 3, 4),
            rec("W2", 30 * 24 * 3600, 1500.0, 3, 4),
        ];
        let f = contests_per_month(&records).unwrap();
        assert!((f - 1.0).abs() < 0.01, "实得 {f}");
    }

    #[test]
    fn contests_per_month_requires_two_records() {
        assert_eq!(contests_per_month(&[]), None);
        assert_eq!(contests_per_month(&[rec("W1", 0, 1500.0, 3, 4)]), None);
    }

    #[test]
    fn contests_per_month_handles_same_timestamp() {
        // 两场时间戳相同 -> 跨度 0，应返回 None 而非除零。
        let records = vec![
            rec("W1", 1000, 1500.0, 3, 4),
            rec("W2", 1000, 1510.0, 3, 4),
        ];
        assert_eq!(contests_per_month(&records), None);
    }

    #[test]
    fn longest_gap_days_finds_max_interval() {
        let day = 86400i64;
        let records = vec![
            rec("W1", 0, 1500.0, 3, 4),
            rec("W2", 7 * day, 1500.0, 3, 4), // 间隔 7 天
            rec("W3", 10 * day, 1500.0, 3, 4), // 间隔 3 天
        ];
        let gap = longest_gap_days(&records).unwrap();
        assert!((gap - 7.0).abs() < 0.01, "实得 {gap}");
    }

    #[test]
    fn longest_gap_requires_two_records() {
        assert_eq!(longest_gap_days(&[]), None);
        assert_eq!(longest_gap_days(&[rec("W1", 0, 1500.0, 3, 4)]), None);
    }

    #[test]
    fn extreme_ratings_do_not_produce_nan() {
        let records = vec![
            rec("W1", 1000, 0.0, 0, 4),
            rec("W2", 2000, 3000.0, 4, 4),
            rec("W3", 3000, 1.0, 0, 4),
        ];
        let a = ContestAnalysis::analyze(&records, ContestSummary::default());
        assert!(a.rating_delta.is_finite());
        assert!(a.average_rating.unwrap().is_finite());
        assert!(a.rating_stddev.unwrap().is_finite());
    }
}
