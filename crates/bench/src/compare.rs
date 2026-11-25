//! `ibc-bench balance --compare OLD.json NEW.json`: which metrics moved by more than their intervals.
//! This is the loop after a change to `rules/*.toml`: run `balance` before and after, compare, and only
//! talk about the metrics listed as moved.

use crate::balance::{format_metric, is_fraction, BalanceReport};
use crate::stats::Est;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Both runs have an interval and the difference is larger than the two combined.
    Moved,
    /// The value differs but at least one run has no interval, so nothing can be said about noise.
    ChangedNoInterval,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    pub key: String,
    pub kind: ChangeKind,
    pub old: Option<Est>,
    pub new: Option<Est>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Comparison {
    pub old_ruleset: String,
    pub new_ruleset: String,
    /// Differences in what was run (bots, maps, seeds, backend). Non-empty means the runs are not like for like.
    pub setup_differences: Vec<String>,
    pub changes: Vec<Change>,
    /// Metrics that differ by less than their combined intervals.
    pub within_noise: u64,
    pub identical: u64,
}

/// A difference is real when it exceeds the root-sum-square of the two half-widths, i.e. the 95 %
/// interval of the difference of two independent estimates.
fn moved(old: &Est, new: &Est) -> Option<ChangeKind> {
    let delta = (new.value - old.value).abs();
    if delta <= 1e-12 {
        return None;
    }
    // Rates have their intervals clamped to 0..=1, so take the wider side as the half-width.
    let half = |e: &Est| Some((e.hi? - e.value).max(e.value - e.lo?));
    match (half(old), half(new)) {
        (Some(a), Some(b)) => (delta > (a * a + b * b).sqrt()).then_some(ChangeKind::Moved),
        _ => Some(ChangeKind::ChangedNoInterval),
    }
}

pub fn compare(old: &BalanceReport, new: &BalanceReport) -> Comparison {
    let mut setup = Vec::new();
    let mut differ = |what: &str, a: String, b: String| {
        if a != b {
            setup.push(format!("{what}: {a} -> {b}"));
        }
    };
    differ("bots", old.bots.join(","), new.bots.join(","));
    differ("maps", old.maps.join(","), new.maps.join(","));
    differ("seeds", old.seeds.to_string(), new.seeds.to_string());
    differ("backend", old.backend.clone(), new.backend.clone());
    let (mut changes, mut within_noise, mut identical) = (Vec::new(), 0, 0);
    for (key, o) in &old.metrics {
        match new.metrics.get(key) {
            None => changes.push(Change {
                key: key.clone(),
                kind: ChangeKind::Removed,
                old: Some(*o),
                new: None,
            }),
            Some(n) => match moved(o, n) {
                Some(kind) => changes.push(Change {
                    key: key.clone(),
                    kind,
                    old: Some(*o),
                    new: Some(*n),
                }),
                None if (n.value - o.value).abs() <= 1e-12 => identical += 1,
                None => within_noise += 1,
            },
        }
    }
    for (key, n) in &new.metrics {
        if !old.metrics.contains_key(key) {
            changes.push(Change {
                key: key.clone(),
                kind: ChangeKind::Added,
                old: None,
                new: Some(*n),
            });
        }
    }
    Comparison {
        old_ruleset: old.ruleset_hash.clone(),
        new_ruleset: new.ruleset_hash.clone(),
        setup_differences: setup,
        changes,
        within_noise,
        identical,
    }
}

pub fn render(c: &Comparison) -> String {
    let mut s = if c.old_ruleset == c.new_ruleset {
        format!(
            "ruleset unchanged ({}): any difference comes from bots, maps or seeds\n",
            c.old_ruleset
        )
    } else {
        format!("ruleset {} -> {}\n", c.old_ruleset, c.new_ruleset)
    };
    for d in &c.setup_differences {
        s.push_str(&format!("WARNING: the runs are not like for like: {d}\n"));
    }
    let show = |key: &str, e: &Option<Est>| {
        e.as_ref()
            .map(|e| format_metric(key, e))
            .unwrap_or_else(|| "-".into())
    };
    let mut section = |title: &str, kind: ChangeKind| {
        let rows: Vec<&Change> = c.changes.iter().filter(|x| x.kind == kind).collect();
        if rows.is_empty() {
            return;
        }
        s.push_str(&format!("{title} ({}):\n", rows.len()));
        for x in rows {
            let delta = match (&x.old, &x.new) {
                (Some(o), Some(n)) if is_fraction(&x.key) => {
                    format!("  ({:+.1} pts)", (n.value - o.value) * 100.0)
                }
                (Some(o), Some(n)) => format!("  ({:+.2})", n.value - o.value),
                _ => String::new(),
            };
            s.push_str(&format!(
                "  {:<44} {} -> {}{delta}\n",
                x.key,
                show(&x.key, &x.old),
                show(&x.key, &x.new)
            ));
        }
    };
    section("MOVED beyond the noise", ChangeKind::Moved);
    section(
        "changed, but no interval to judge it by",
        ChangeKind::ChangedNoInterval,
    );
    section("only in NEW", ChangeKind::Added);
    section("only in OLD", ChangeKind::Removed);
    if c.changes.is_empty() {
        s.push_str("no metric moved beyond its interval\n");
    }
    s.push_str(&format!(
        "{} metrics differ within the noise, {} are identical\n",
        c.within_noise, c.identical
    ));
    s
}
