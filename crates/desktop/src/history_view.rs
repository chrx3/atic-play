use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, Subscription, Window, div, prelude::*, px,
    uniform_list,
};
use gpui_m3::{Button, ListItem, Theme};
use time::{Date, Month, OffsetDateTime, UtcOffset, Weekday};

use crate::covers::Covers;
use crate::history::History;
use crate::navigation::{Navigation, with_track_menu};
use crate::playback::Playback;
use crate::ui::{placeholder, section_title, thumbnail};

const THUMB_SIZE: u32 = 120;
/// Day headings and songs share the list's row height.
const ROW_HEIGHT: f32 = 56.;

/// A line of the list: a day heading, or a play (its index in the history).
enum Row {
    Day(String),
    Play(usize),
}

/// The songs played on this computer, newest first, under day headings.
pub struct HistoryView {
    history: Entity<History>,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    rows: Vec<Row>,
    /// "Borrar historial" was clicked once and waits for the confirming click.
    confirm_clear: bool,
    _subscriptions: Vec<Subscription>,
}

impl HistoryView {
    pub fn new(
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let history = History::global(cx);
        let subscriptions = vec![
            cx.observe(&covers, |_, _, cx| cx.notify()),
            cx.observe(&history, |this, _, cx| {
                this.rows = rows(&this.history, cx);
                cx.notify();
            }),
        ];
        HistoryView {
            rows: rows(&history, cx),
            history,
            playback,
            covers,
            navigation,
            confirm_clear: false,
            _subscriptions: subscriptions,
        }
    }

    /// Called when the page opens: the day headings follow the date, and a pending "clear"
    /// is forgotten.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rows = rows(&self.history, cx);
        self.confirm_clear = false;
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = *Theme::of(cx);
        let offset = local_offset();
        range
            .map(|index| {
                let row = match &self.rows[index] {
                    Row::Day(label) => div()
                        .size_full()
                        .flex()
                        .items_end()
                        .px_3()
                        .pb_2()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(s.on_surface_variant)
                        .child(label.clone())
                        .into_any_element(),
                    Row::Play(entry) => {
                        let entry = self.history.read(cx).entries()[*entry].clone();
                        let track = entry.track;
                        let thumb = self.covers.update(cx, |covers, cx| {
                            covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
                        });
                        let mut subtitle = vec![track.artists.clone()];
                        subtitle.extend(track.album.clone());
                        let playback = self.playback.clone();
                        let clicked = track.clone();
                        let item = ListItem::new(("history-play", index), track.title.clone())
                            .subtitle(subtitle.join(" · "))
                            .leading(thumbnail(thumb, 48., 8., cx))
                            .trailing(
                                div()
                                    .text_sm()
                                    .text_color(s.on_surface_variant)
                                    .child(clock_time(entry.played_at, offset)),
                            )
                            .on_click(move |_: &ClickEvent, _, cx| {
                                let track = clicked.clone();
                                playback.update(cx, |playback, cx| {
                                    playback.play_tracks(vec![track], 0, cx)
                                });
                            });
                        with_track_menu(item, &track, &self.navigation).into_any_element()
                    }
                };
                div().h(px(ROW_HEIGHT)).child(row).into_any_element()
            })
            .collect()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let clear = if self.confirm_clear {
            Button::new("history-clear", "Sí, borrar todo")
                .filled()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirm_clear = false;
                    this.history.update(cx, |history, cx| history.clear(cx));
                }))
        } else {
            Button::new("history-clear", "Borrar historial")
                .outlined()
                .icon("trash")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirm_clear = true;
                    cx.notify();
                }))
        };
        div()
            .px_4()
            .pb_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_title("Historial"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child("Lo que escuchaste en este equipo"),
                    ),
            )
            .child(clear)
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.rows.is_empty() {
            return placeholder(
                "Aún no hay historial",
                "Las canciones que escuches aparecerán aquí",
                cx,
            )
            .into_any_element();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(
                uniform_list(
                    "history-rows",
                    self.rows.len(),
                    cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
                )
                .flex_1()
                .px_4()
                .pb_4(),
            )
            .into_any_element()
    }
}

/// The history as list rows, with a heading before each day's first play.
fn rows(history: &Entity<History>, cx: &gpui::App) -> Vec<Row> {
    let offset = local_offset();
    let today = OffsetDateTime::now_utc().to_offset(offset).date();
    let mut rows = Vec::new();
    let mut last_day = None;
    for (index, entry) in history.read(cx).entries().iter().enumerate() {
        let day = local_date_time(entry.played_at, offset).date();
        if last_day != Some(day) {
            last_day = Some(day);
            rows.push(Row::Day(day_label(day, today)));
        }
        rows.push(Row::Play(index));
    }
    rows
}

/// The computer's time zone; UTC if it cannot be read.
fn local_offset() -> UtcOffset {
    UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
}

fn local_date_time(unix_secs: i64, offset: UtcOffset) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(unix_secs)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .to_offset(offset)
}

/// "14:05".
fn clock_time(unix_secs: i64, offset: UtcOffset) -> String {
    let time = local_date_time(unix_secs, offset);
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// "Hoy", "Ayer", the weekday within the last week, then the date.
fn day_label(day: Date, today: Date) -> String {
    let days_ago = (today - day).whole_days();
    match days_ago {
        0 => "Hoy".to_string(),
        1 => "Ayer".to_string(),
        2..=6 => weekday_name(day.weekday()).to_string(),
        _ if day.year() == today.year() => {
            format!("{} de {}", day.day(), month_name(day.month()))
        }
        _ => format!(
            "{} de {} de {}",
            day.day(),
            month_name(day.month()),
            day.year()
        ),
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "Lunes",
        Weekday::Tuesday => "Martes",
        Weekday::Wednesday => "Miércoles",
        Weekday::Thursday => "Jueves",
        Weekday::Friday => "Viernes",
        Weekday::Saturday => "Sábado",
        Weekday::Sunday => "Domingo",
    }
}

fn month_name(month: Month) -> &'static str {
    match month {
        Month::January => "enero",
        Month::February => "febrero",
        Month::March => "marzo",
        Month::April => "abril",
        Month::May => "mayo",
        Month::June => "junio",
        Month::July => "julio",
        Month::August => "agosto",
        Month::September => "septiembre",
        Month::October => "octubre",
        Month::November => "noviembre",
        Month::December => "diciembre",
    }
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    #[test]
    fn days_read_relative_to_today() {
        let today = date!(2026 - 10 - 20);
        assert_eq!(day_label(today, today), "Hoy");
        assert_eq!(day_label(date!(2026 - 10 - 19), today), "Ayer");
        assert_eq!(day_label(date!(2026 - 10 - 16), today), "Viernes");
        assert_eq!(day_label(date!(2026 - 10 - 13), today), "13 de octubre");
        assert_eq!(
            day_label(date!(2025 - 12 - 31), today),
            "31 de diciembre de 2025"
        );
    }

    #[test]
    fn clock_follows_the_offset() {
        // 2026-10-20 12:05 UTC.
        let noon = 1_792_497_900;
        assert_eq!(clock_time(noon, UtcOffset::UTC), "12:05");
        let chile = UtcOffset::from_hms(-3, 0, 0).unwrap();
        assert_eq!(clock_time(noon, chile), "09:05");
    }
}
