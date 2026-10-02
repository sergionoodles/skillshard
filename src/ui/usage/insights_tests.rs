use super::*;
use crate::usage::Counts;

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
}

fn bucket(label: &str, value: u64) -> Bucket {
    Bucket {
        label: label.into(),
        value,
    }
}

fn ranking(skill_id: i64, name: &str, activations: u64) -> Ranking {
    Ranking {
        skill_id,
        name: name.into(),
        identity: String::new(),
        counts: Counts {
            activations,
            ..Counts::default()
        },
    }
}

#[test]
fn days_cover_the_whole_range_with_quiet_days_at_zero() {
    let days = days(
        3,
        date(30),
        &[bucket("2026-09-28", 4), bucket("2026-09-30", 1)],
        &[],
        &Selection::default(),
    );

    let summary: Vec<(NaiveDate, u64)> = days.iter().map(|day| (day.date, day.total)).collect();
    assert_eq!(summary, vec![(date(28), 4), (date(29), 0), (date(30), 1)]);
    assert!(days.iter().all(|day| day.segments.is_empty()));
}

#[test]
fn selected_skills_split_each_day_by_their_colour_slot() {
    let selection = Selection::default().toggled(7, "alpha").toggled(9, "beta");
    let by_skill = [
        SkillBucket {
            label: "2026-09-30".into(),
            skill_id: 9,
            value: 2,
        },
        SkillBucket {
            label: "2026-09-30".into(),
            skill_id: 7,
            value: 3,
        },
    ];

    let days = days(
        1,
        date(30),
        &[bucket("2026-09-30", 5)],
        &by_skill,
        &selection,
    );

    assert_eq!(days[0].segments, vec![(0, 3), (1, 2)]);
}

#[test]
fn a_skill_keeps_its_colour_when_another_is_deselected() {
    let selection = Selection::default()
        .toggled(1, "alpha")
        .toggled(2, "beta")
        .toggled(3, "gamma");

    let without_alpha = selection.toggled(1, "alpha");
    let refilled = without_alpha.toggled(4, "delta");

    assert_eq!(without_alpha.slot(2), Some(1));
    assert_eq!(without_alpha.slot(3), Some(2));
    assert_eq!(refilled.slot(4), Some(0));
}

#[test]
fn selection_stops_at_one_skill_per_chart_colour() {
    let full = (0..MAX_SELECTED as i64).fold(Selection::default(), |selection, id| {
        selection.toggled(id, "skill")
    });

    let attempted = full.toggled(99, "extra");

    assert!(full.is_full());
    assert_eq!(attempted, full);
}

#[test]
fn busiest_day_prefers_the_latest_of_equal_days_and_skips_empty_ranges() {
    let days = days(
        3,
        date(30),
        &[bucket("2026-09-28", 4), bucket("2026-09-29", 4)],
        &[],
        &Selection::default(),
    );

    assert_eq!(busiest_day(&days).map(|day| day.date), Some(date(29)));
    assert_eq!(busiest_day(&days[2..]), None);
}

#[test]
fn shown_rankings_follow_the_selection_and_metric_order() {
    let rankings = [
        ranking(1, "alpha", 2),
        ranking(2, "beta", 5),
        ranking(3, "idle", 0),
    ];

    let all = shown_rankings(&rankings, &Selection::default(), Metric::Activations);
    let selected = shown_rankings(
        &rankings,
        &Selection::default().toggled(1, "alpha"),
        Metric::Activations,
    );

    let names = |shown: Vec<&Ranking>| shown.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(all), vec!["beta", "alpha"]);
    assert_eq!(names(selected), vec!["alpha"]);
}

#[test]
fn share_rounds_to_whole_percent_and_handles_no_total() {
    assert_eq!(share(1, 3), 33);
    assert_eq!(share(2, 3), 67);
    assert_eq!(share(5, 0), 0);
}

#[test]
fn unused_lists_installed_skills_without_any_used_copy() {
    let installed = [
        InstalledSkill {
            name: "alpha".into(),
            ids: vec![1, 10],
        },
        InstalledSkill {
            name: "never-tracked".into(),
            ids: Vec::new(),
        },
        InstalledSkill {
            name: "quiet".into(),
            ids: vec![3],
        },
    ];
    let rankings = [ranking(10, "alpha", 2), ranking(3, "quiet", 0)];

    assert_eq!(
        unused(&installed, &rankings),
        vec!["never-tracked", "quiet"]
    );
}
