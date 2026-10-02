//! Pure derivations from a usage snapshot: what the usage view shows, without
//! any rendering, so it is testable on its own.

use crate::usage::{Bucket, Metric, Ranking, SkillBucket};
use chrono::{Days, NaiveDate};
use std::collections::{HashMap, HashSet};

/// One per theme chart colour; more would have to reuse a colour.
pub const MAX_SELECTED: usize = 5;

const DATE_FORMAT: &str = "%Y-%m-%d";

/// A skill picked for comparison. `slot` picks its chart colour and stays put
/// while other skills come and go, so a colour always means the same skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedSkill {
    pub skill_id: i64,
    pub name: String,
    pub slot: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    skills: Vec<SelectedSkill>,
}

impl Selection {
    pub fn skills(&self) -> &[SelectedSkill] {
        &self.skills
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.skills.len() >= MAX_SELECTED
    }

    pub fn ids(&self) -> Vec<i64> {
        self.skills.iter().map(|skill| skill.skill_id).collect()
    }

    pub fn slot(&self, skill_id: i64) -> Option<usize> {
        self.skills
            .iter()
            .find(|skill| skill.skill_id == skill_id)
            .map(|skill| skill.slot)
    }

    /// The selection with `skill_id` removed if present, otherwise added in
    /// the lowest free colour slot. Adding to a full selection changes nothing.
    pub fn toggled(&self, skill_id: i64, name: &str) -> Self {
        if self.slot(skill_id).is_some() {
            let skills = self
                .skills
                .iter()
                .filter(|skill| skill.skill_id != skill_id)
                .cloned()
                .collect();
            return Self { skills };
        }
        if self.is_full() {
            return self.clone();
        }

        let taken: HashSet<usize> = self.skills.iter().map(|skill| skill.slot).collect();
        let slot = (0..MAX_SELECTED)
            .find(|slot| !taken.contains(slot))
            .unwrap_or_default();
        let mut skills = self.skills.clone();
        skills.push(SelectedSkill {
            skill_id,
            name: name.to_string(),
            slot,
        });
        Self { skills }
    }
}

/// One bar of the daily chart. `segments` splits the total by selected skill
/// (colour slot, value) and is empty when no skills are selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    pub total: u64,
    pub segments: Vec<(usize, u64)>,
}

/// Every calendar day of the range ending `today`, oldest first, with days
/// without activity present at zero so gaps read as gaps.
pub fn days(
    range_days: u8,
    today: NaiveDate,
    daily: &[Bucket],
    by_skill: &[SkillBucket],
    selection: &Selection,
) -> Vec<Day> {
    let totals: HashMap<&str, u64> = daily
        .iter()
        .map(|bucket| (bucket.label.as_str(), bucket.value))
        .collect();

    (0..u64::from(range_days))
        .rev()
        .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
        .map(|date| {
            let label = date.format(DATE_FORMAT).to_string();
            let segments = selection
                .skills()
                .iter()
                .filter_map(|skill| {
                    by_skill
                        .iter()
                        .find(|bucket| bucket.label == label && bucket.skill_id == skill.skill_id)
                        .map(|bucket| (skill.slot, bucket.value))
                })
                .collect();
            Day {
                date,
                total: totals.get(label.as_str()).copied().unwrap_or_default(),
                segments,
            }
        })
        .collect()
}

/// The day with the most activity; the latest wins a tie, being the more
/// relevant. `None` when nothing happened in the range.
pub fn busiest_day(days: &[Day]) -> Option<&Day> {
    days.iter()
        .filter(|day| day.total > 0)
        .max_by_key(|day| (day.total, day.date))
}

/// Rankings shown under the current selection, most used first by `metric`.
/// With nothing selected every ranked skill is shown.
pub fn shown_rankings<'a>(
    rankings: &'a [Ranking],
    selection: &Selection,
    metric: Metric,
) -> Vec<&'a Ranking> {
    let mut shown: Vec<&Ranking> = rankings
        .iter()
        .filter(|ranking| selection.is_empty() || selection.slot(ranking.skill_id).is_some())
        .filter(|ranking| ranking.counts.value(metric) > 0)
        .collect();
    shown.sort_by(|a, b| {
        b.counts
            .value(metric)
            .cmp(&a.counts.value(metric))
            .then_with(|| a.name.cmp(&b.name))
    });
    shown
}

/// Whole-number percentage of `value` in `total`.
pub fn share(value: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    (value * 100 + total / 2) / total
}

/// An installed skill and the usage ids recorded for its copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub name: String,
    pub ids: Vec<i64>,
}

/// Installed skills with no recorded use in the shown rankings, by name.
pub fn unused<'a>(installed: &'a [InstalledSkill], rankings: &[Ranking]) -> Vec<&'a str> {
    let used: HashSet<i64> = rankings
        .iter()
        .filter(|ranking| ranking.counts.activations > 0)
        .map(|ranking| ranking.skill_id)
        .collect();
    let mut names: Vec<&str> = installed
        .iter()
        .filter(|skill| !skill.ids.iter().any(|id| used.contains(id)))
        .map(|skill| skill.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
#[path = "insights_tests.rs"]
mod tests;
