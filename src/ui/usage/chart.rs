//! The daily bar chart: one bar per calendar day, stacked by selected skill.

use super::insights::{Day, Selection};
use super::labels;
use crate::usage::Metric;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const PLOT_HEIGHT: f32 = 160.;
/// Surface gap between neighbouring bars and stacked segments.
const MARK_GAP: f32 = 2.;
/// Rounding on a bar's data end; its base stays square on the baseline.
const BAR_RADIUS: f32 = 4.;
/// Keeps a small non-zero day visible next to a busy one.
const MIN_BAR_HEIGHT: f32 = 2.;
/// Roughly how many dates the axis names, whatever the range.
const AXIS_LABELS: usize = 6;

/// Theme colour for a selected skill's slot, in fixed order.
pub fn slot_color(slot: usize, cx: &App) -> Hsla {
    let theme = cx.theme();
    [
        theme.chart_1,
        theme.chart_2,
        theme.chart_3,
        theme.chart_4,
        theme.chart_5,
    ][slot % 5]
}

/// A small colour key for a skill, beside its name.
pub fn swatch(slot: usize, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .size(px(10.))
        .rounded(px(3.))
        .bg(slot_color(slot, cx))
}

/// The value a bar stands at: the selected skills' sum when stacked.
fn bar_value(day: &Day, stacked: bool) -> u64 {
    if stacked {
        return day.segments.iter().map(|(_, value)| value).sum();
    }
    day.total
}

pub fn bar_height(value: u64, maximum: u64) -> f32 {
    if maximum == 0 || value == 0 {
        return 0.;
    }
    (PLOT_HEIGHT * (value as f64 / maximum as f64) as f32).max(MIN_BAR_HEIGHT)
}

pub fn render(days: &[Day], metric: Metric, selection: &Selection, cx: &App) -> impl IntoElement {
    let stacked = !selection.is_empty();
    let maximum = days
        .iter()
        .map(|day| bar_value(day, stacked))
        .max()
        .unwrap_or_default();
    let label_every = days.len().div_ceil(AXIS_LABELS).max(1);

    v_flex()
        .gap_2()
        .child(
            h_flex()
                .h(px(PLOT_HEIGHT))
                .gap_2()
                .child(
                    // A single recessive reference: the scale's top value.
                    v_flex()
                        .w(px(28.))
                        .h_full()
                        .items_end()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(maximum.to_string())
                        .child(div().flex_1())
                        .child("0"),
                )
                .child(
                    h_flex()
                        .id("usage-daily-chart")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_end()
                        .gap(px(MARK_GAP))
                        .border_t_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .children(days.iter().enumerate().map(|(index, day)| {
                            render_bar(index, day, maximum, metric, selection, cx)
                        })),
                ),
        )
        .child(
            h_flex()
                .pl(px(36.))
                .gap(px(MARK_GAP))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(days.iter().enumerate().map(|(index, day)| {
                    let labelled = index % label_every == 0 || index + 1 == days.len();
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .when(labelled, |this| {
                            this.child(labels::short_day_label(day.date))
                        })
                })),
        )
}

fn render_bar(
    index: usize,
    day: &Day,
    maximum: u64,
    metric: Metric,
    selection: &Selection,
    cx: &App,
) -> impl IntoElement {
    let stacked = !selection.is_empty();
    let tooltip = bar_tooltip(day, metric, selection);
    let column = v_flex()
        .id(("usage-day", index))
        .flex_1()
        .min_w_0()
        .h_full()
        .justify_end()
        .gap(px(MARK_GAP))
        .rounded_t(px(BAR_RADIUS))
        .hover(|this| this.bg(cx.theme().list_hover))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .test_support();

    if !stacked {
        return column.child(
            div()
                .w_full()
                .h(px(bar_height(day.total, maximum)))
                .rounded_t(px(BAR_RADIUS))
                .bg(cx.theme().primary),
        );
    }

    // Slot order, last on top, so a skill sits at the same level every day.
    let mut segments = day.segments.clone();
    segments.sort_by_key(|(slot, _)| std::cmp::Reverse(*slot));
    let top = segments.first().map(|(slot, _)| *slot);
    column.children(segments.into_iter().map(|(slot, value)| {
        div()
            .w_full()
            .h(px(bar_height(value, maximum)))
            .when(Some(slot) == top, |this| this.rounded_t(px(BAR_RADIUS)))
            .bg(slot_color(slot, cx))
    }))
}

fn bar_tooltip(day: &Day, metric: Metric, selection: &Selection) -> SharedString {
    let date = labels::day_label(day.date);
    if selection.is_empty() {
        return format!("{date} · {}", labels::metric_count(metric, day.total)).into();
    }
    let parts = selection
        .skills()
        .iter()
        .map(|skill| {
            let value = day
                .segments
                .iter()
                .find(|(slot, _)| *slot == skill.slot)
                .map_or(0, |(_, value)| *value);
            format!("{} {value}", skill.name)
        })
        .collect::<Vec<_>>()
        .join(" · ");
    format!("{date} · {parts}").into()
}

/// Names each selected skill beside its colour.
pub fn legend(selection: &Selection, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_wrap()
        .gap_x_3()
        .gap_y_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .children(selection.skills().iter().map(|skill| {
            h_flex()
                .gap_1p5()
                .items_center()
                .child(swatch(skill.slot, cx))
                .child(skill.name.clone())
        }))
}
