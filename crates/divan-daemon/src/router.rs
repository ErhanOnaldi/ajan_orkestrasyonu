//! Cost Router (K9, spec §5.2, impl plan §5.5, §F3.3).
//!
//! Deterministic, zero-LLM (K2): a YAML rule file (`match`/`prefer`/`exclude`)
//! scores the registered agents for a task and picks one. v1 inputs: task kind,
//! spec length, agent cost_class/skills/multi_turn, and `different_vendor_than`
//! (e.g. pick a reviewer from a different vendor than the author). The decision
//! (winner + matched rule ids + reasons) is traceable and explained via
//! `divan router explain`.

use divan_core::{AgentCard, AgentTool, TaskKind};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("router rules parse error: {0}")]
    Parse(String),
    #[error("no eligible agent for {kind:?} (all candidates excluded)")]
    NoCandidate { kind: TaskKind },
}

/// A parsed rule file: `rules: [ {match, prefer?, exclude?}, ... ]`.
#[derive(Debug, Clone, Deserialize)]
pub struct RouterRules {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(rename = "match", default)]
    pub match_: MatchCond,
    #[serde(default)]
    pub prefer: Option<PreferCond>,
    #[serde(default)]
    pub exclude: Option<ExcludeCond>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchCond {
    #[serde(default)]
    pub kind: Option<TaskKind>,
    #[serde(default)]
    pub multi_turn: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PreferCond {
    /// e.g. `"<=2"`, `">=4"`, `"=3"`.
    #[serde(default)]
    pub cost_class: Option<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    /// `"author"` => prefer a different vendor than the task's author.
    #[serde(default)]
    pub different_vendor_than: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExcludeCond {
    #[serde(default)]
    pub tool: Option<AgentTool>,
    #[serde(default)]
    pub multi_turn: Option<bool>,
}

/// The task facts the router scores against.
#[derive(Debug, Clone)]
pub struct RouteTask {
    pub kind: TaskKind,
    /// True if the task needs multiple turns (excludes single-shot agents).
    pub multi_turn: bool,
}

/// The router's decision (for tracing + `router explain`).
#[derive(Debug, Clone)]
pub struct Decision {
    pub agent: divan_core::AgentId,
    /// Indices of the rules that matched the task (rule "ids").
    pub matched_rules: Vec<usize>,
    pub reasons: Vec<String>,
}

impl MatchCond {
    fn applies(&self, task: &RouteTask) -> bool {
        if let Some(k) = self.kind {
            if k != task.kind {
                return false;
            }
        }
        if let Some(mt) = self.multi_turn {
            if mt != task.multi_turn {
                return false;
            }
        }
        // An all-empty match applies to every task.
        true
    }
}

/// Parse a `cost_class` constraint like `"<=2"` and test `value` against it.
fn cost_matches(constraint: &str, value: u8) -> bool {
    let c = constraint.trim();
    let (op, num) = if let Some(n) = c.strip_prefix("<=") {
        ("<=", n)
    } else if let Some(n) = c.strip_prefix(">=") {
        (">=", n)
    } else if let Some(n) = c.strip_prefix('<') {
        ("<", n)
    } else if let Some(n) = c.strip_prefix('>') {
        (">", n)
    } else if let Some(n) = c.strip_prefix('=') {
        ("=", n)
    } else {
        ("=", c)
    };
    let Ok(n) = num.trim().parse::<u8>() else {
        return false;
    };
    match op {
        "<=" => value <= n,
        ">=" => value >= n,
        "<" => value < n,
        ">" => value > n,
        _ => value == n,
    }
}

/// The cost router.
#[derive(Debug, Clone)]
pub struct CostRouter {
    rules: Vec<Rule>,
}

impl CostRouter {
    pub fn from_yaml(s: &str) -> Result<Self, RouterError> {
        let parsed: RouterRules =
            serde_yaml::from_str(s).map_err(|e| RouterError::Parse(e.to_string()))?;
        Ok(Self {
            rules: parsed.rules,
        })
    }

    /// The spec §5.2 default rules, used when no rule file is configured.
    pub fn default_rules() -> Self {
        Self::from_yaml(DEFAULT_RULES_YAML).expect("built-in default rules parse")
    }

    /// Pick the best agent for `task` from `candidates`, honoring rule
    /// `exclude`/`prefer` and `different_vendor_than: author`. Deterministic:
    /// ties break by (score desc, cost_class asc, id asc). Errors if none remain.
    pub fn pick(
        &self,
        task: &RouteTask,
        candidates: &[AgentCard],
        author: Option<&AgentCard>,
    ) -> Result<Decision, RouterError> {
        let matched: Vec<usize> = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.match_.applies(task))
            .map(|(i, _)| i)
            .collect();

        // 1. Hard filters: a multi-turn task needs a multi-turn-capable agent
        //    (spec §3.5: agy multi_turn=false), plus any rule `exclude`s.
        let mut pool: Vec<&AgentCard> = candidates
            .iter()
            .filter(|c| !task.multi_turn || c.multi_turn)
            .collect();
        for &i in &matched {
            if let Some(ex) = &self.rules[i].exclude {
                pool.retain(|c| {
                    let tool_excluded = ex.tool.map(|t| t == c.tool).unwrap_or(false);
                    let mt_excluded = ex.multi_turn.map(|m| m == c.multi_turn).unwrap_or(false);
                    !(tool_excluded || mt_excluded)
                });
            }
        }
        if pool.is_empty() {
            return Err(RouterError::NoCandidate { kind: task.kind });
        }

        // 2. Score by `prefer` constraints (soft); higher = better.
        let mut reasons = Vec::new();
        let scored: Vec<(i32, &AgentCard)> = pool
            .iter()
            .map(|c| (self.score(c, task, author, &matched, &mut reasons), *c))
            .collect();

        // 3. Deterministic winner: score desc, then cost_class asc, then id asc.
        let winner = scored
            .iter()
            .max_by(|a, b| {
                a.0.cmp(&b.0)
                    .then(b.1.cost_class.cmp(&a.1.cost_class))
                    .then(b.1.id.cmp(&a.1.id))
            })
            .map(|(_, c)| *c)
            .ok_or(RouterError::NoCandidate { kind: task.kind })?;

        Ok(Decision {
            agent: winner.id.clone(),
            matched_rules: matched,
            reasons,
        })
    }

    fn score(
        &self,
        c: &AgentCard,
        _task: &RouteTask,
        author: Option<&AgentCard>,
        matched: &[usize],
        reasons: &mut Vec<String>,
    ) -> i32 {
        let mut score = 0;
        for &i in matched {
            let Some(pref) = &self.rules[i].prefer else {
                continue;
            };
            if let Some(cc) = &pref.cost_class {
                if cost_matches(cc, c.cost_class) {
                    score += 2;
                    reasons.push(format!("rule {i}: {} cost_class {cc}", c.id));
                }
            }
            if let Some(skills) = &pref.skills {
                if skills.iter().any(|s| c.skills.iter().any(|cs| cs == s)) {
                    score += 2;
                    reasons.push(format!("rule {i}: {} has preferred skill", c.id));
                }
            }
            if let Some(dv) = &pref.different_vendor_than {
                if dv == "author" {
                    if let Some(a) = author {
                        if a.tool != c.tool {
                            score += 3;
                            reasons.push(format!(
                                "rule {i}: {} is a different vendor than author {}",
                                c.id, a.id
                            ));
                        } else {
                            score -= 3; // same vendor as author: strongly disfavored
                        }
                    }
                }
            }
        }
        score
    }
}

/// Default rule set (spec §5.2 example).
pub const DEFAULT_RULES_YAML: &str = r#"
rules:
  - match: { kind: test }
    prefer: { cost_class: "<=2" }
  - match: { kind: review }
    prefer: { skills: [review], different_vendor_than: author }
  - match: { kind: plan }
    prefer: { cost_class: ">=4" }
  - match: { multi_turn: true }
    exclude: { tool: agy }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{AgentId, AgentStatus, Capability, DeliveryKind};

    fn agent(id: &str, tool: AgentTool, cost: u8, skills: &[&str], multi_turn: bool) -> AgentCard {
        AgentCard {
            id: AgentId::new(id),
            tool,
            display_name: None,
            capabilities: vec![Capability::Read],
            cost_class: cost,
            skills: skills.iter().map(|s| s.to_string()).collect(),
            delivery: vec![DeliveryKind::Mcp],
            multi_turn,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 0,
        }
    }

    #[test]
    fn default_rules_parse() {
        let r = CostRouter::default_rules();
        assert_eq!(r.rules.len(), 4);
    }

    #[test]
    fn cost_constraint_parsing() {
        assert!(cost_matches("<=2", 2));
        assert!(!cost_matches("<=2", 3));
        assert!(cost_matches(">=4", 5));
        assert!(cost_matches("=3", 3));
        assert!(cost_matches("<3", 2));
    }

    #[test]
    fn review_prefers_different_vendor_than_author() {
        // F3.3 acceptance: review task -> reviewer from a different vendor.
        let router = CostRouter::default_rules();
        let claude = agent("claude-1", AgentTool::Claude, 5, &["implement"], true);
        let codex = agent("codex-1", AgentTool::Codex, 4, &["review"], true);
        let task = RouteTask {
            kind: TaskKind::Review,
            multi_turn: false,
        };
        let d = router
            .pick(&task, &[claude.clone(), codex.clone()], Some(&claude))
            .unwrap();
        assert_eq!(
            d.agent,
            AgentId::new("codex-1"),
            "different vendor than author"
        );
    }

    #[test]
    fn multi_turn_task_excludes_agy() {
        // F3.3 acceptance: multi-turn task is not assigned to agy.
        let router = CostRouter::default_rules();
        let agy = agent("agy-1", AgentTool::Agy, 2, &[], false);
        let codex = agent("codex-1", AgentTool::Codex, 4, &["review"], true);
        let task = RouteTask {
            kind: TaskKind::Implement,
            multi_turn: true,
        };
        let d = router.pick(&task, &[agy, codex], None).unwrap();
        assert_eq!(
            d.agent,
            AgentId::new("codex-1"),
            "agy excluded for multi-turn"
        );
    }

    #[test]
    fn no_candidate_is_deterministic_error() {
        let router = CostRouter::default_rules();
        let agy = agent("agy-1", AgentTool::Agy, 2, &[], false);
        let task = RouteTask {
            kind: TaskKind::Implement,
            multi_turn: true,
        };
        // Only agy available, but multi-turn excludes it.
        assert!(matches!(
            router.pick(&task, &[agy], None),
            Err(RouterError::NoCandidate { .. })
        ));
    }

    #[test]
    fn test_kind_prefers_cheap_agent() {
        let router = CostRouter::default_rules();
        let cheap = agent("cheap", AgentTool::Codex, 2, &[], true);
        let pricey = agent("pricey", AgentTool::Claude, 5, &[], true);
        let task = RouteTask {
            kind: TaskKind::Test,
            multi_turn: false,
        };
        let d = router.pick(&task, &[pricey, cheap], None).unwrap();
        assert_eq!(d.agent, AgentId::new("cheap"));
    }
}
