use crate::{
    advanced::Advanced,
    trace::{Display, History, Key, Recorded, Source},
    FAULT, GOOD, WARN,
};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Stroke};
use std::time::Duration;

const COLORS: [Color32; 8] = [
    Color32::from_rgb(121, 186, 250),
    Color32::from_rgb(240, 184, 103),
    Color32::from_rgb(119, 212, 162),
    Color32::from_rgb(205, 157, 240),
    Color32::from_rgb(240, 149, 173),
    Color32::from_rgb(115, 208, 221),
    Color32::from_rgb(219, 221, 132),
    Color32::from_rgb(181, 195, 215),
];

pub fn observations(app: &mut Advanced, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.heading("观察变量");
        if ui.small_button("+ WKC").clicked() {
            app.add_watch(Key::wkc(), None, true);
        }
        if app.history.cursor.is_some() && ui.small_button("恢复实时").clicked() {
            app.history.cursor = None;
        }
    });
    let frame = app.history.observed();
    if let Some(frame) = frame {
        let live = app.history.frames.back().map_or(frame.id, |value| value.id);
        if app.history.cursor.is_some() {
            ui.colored_label(WARN, format!("[历史记录] #{}  /  实时 #{}", frame.id, live));
        } else {
            ui.colored_label(GOOD, format!("实时 #{}", frame.id));
        }
        ui.weak(format!(
            "{:.6} s · WKC {}",
            frame.time.as_secs_f64(),
            frame.wkc
        ));
    }
    if app.watches.is_empty() {
        ui.weak("在过程数据表点「+观察」或「+波形」。");
        return;
    }
    let mut groups: Vec<_> = app
        .watches
        .iter()
        .map(|watch| watch.group.clone())
        .collect();
    groups.sort();
    groups.dedup();
    let name_width = (ui.available_width() * 0.25).clamp(54.0, 90.0);
    let value_width = (ui.available_width() * 0.35).clamp(63.0, 120.0);
    let mut remove = None;
    for group in groups {
        ui.push_id(&group, |ui| {
            egui::CollapsingHeader::new(if group.trim().is_empty() {
                "默认"
            } else {
                &group
            })
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new("watch_table")
                    .striped(true)
                    .spacing(vec2(4.0, 2.0))
                    .show(ui, |ui| {
                        for title in ["态", "变量", "值", "格式", ""] {
                            ui.weak(title);
                        }
                        ui.end_row();
                        for (id, watch) in app
                            .watches
                            .iter_mut()
                            .enumerate()
                            .filter(|(_, w)| w.group == group)
                        {
                            let value = frame.and_then(|frame| watch.formatted(frame));
                            let validity = if value.is_some() {
                                "有效"
                            } else if watch.id.is_none() && watch.key.source != Source::Wkc {
                                "未映射"
                            } else {
                                "该周期无有效值"
                            };
                            ui.colored_label(
                                if value.is_some() {
                                    GOOD
                                } else {
                                    ui.visuals().weak_text_color()
                                },
                                "●",
                            )
                            .on_hover_text(validity);
                            let caption = match watch.key.source {
                                Source::Pdo => {
                                    format!("{:04X}:{:02X}", watch.key.index, watch.key.sub)
                                }
                                Source::Gpio => "GPIO".into(),
                                Source::Pdi => "PDI 相位".into(),
                                Source::Wkc => "WKC".into(),
                            };
                            fixed_label(ui, &caption, name_width, false).on_hover_text(format!(
                                "{} · {} 位 · 偏移 {}",
                                watch.key.caption(),
                                watch.key.width,
                                watch.key.offset
                            ));
                            let value = value.unwrap_or_else(|| "—".into());
                            fixed_label(ui, &value, value_width, true).on_hover_text(&value);
                            ui.push_id(id, |ui| {
                                ui.menu_button(display_name(watch.display), |ui| {
                                    for (display, label) in [
                                        (Display::Hex, "十六进制"),
                                        (Display::Decimal, "十进制"),
                                        (Display::Bits, "位域"),
                                    ] {
                                        if ui
                                            .selectable_value(&mut watch.display, display, label)
                                            .clicked()
                                        {
                                            ui.close_menu();
                                        }
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    if wave_toggle(ui, watch.plotted) {
                                        watch.plotted = !watch.plotted;
                                    }
                                    let group_edit = ui.id().with("group_edit");
                                    let menu = ui.menu_button("···", |ui| {
                                        ui.label("分组");
                                        let mut draft = ui
                                            .data_mut(|data| data.get_temp::<String>(group_edit))
                                            .unwrap_or_else(|| watch.group.clone());
                                        ui.add(
                                            egui::TextEdit::singleline(&mut draft)
                                                .desired_width(112.0),
                                        );
                                        if ui.small_button("应用").clicked() {
                                            watch.group = draft.trim().to_owned();
                                            ui.data_mut(|data| data.remove::<String>(group_edit));
                                            ui.close_menu();
                                        } else {
                                            ui.data_mut(|data| data.insert_temp(group_edit, draft));
                                        }
                                        if ui.small_button("移除").clicked() {
                                            remove = Some(id);
                                            ui.close_menu();
                                        }
                                    });
                                    menu.response.on_hover_text("分组或移除");
                                });
                            });
                            ui.end_row();
                        }
                    });
            });
        });
    }
    if let Some(id) = remove {
        app.watches.remove(id);
    }
}

fn wave_toggle(ui: &mut egui::Ui, plotted: bool) -> bool {
    let response = ui.add(
        egui::Button::new("")
            .small()
            .selected(plotted)
            .min_size(vec2(20.0, 17.0)),
    );
    let c = response.rect.center();
    let color = if plotted {
        CROSSHAIR
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().add(egui::Shape::line(
        [
            (-6.0, 2.0),
            (-3.0, 2.0),
            (-1.0, -4.0),
            (2.0, 4.0),
            (4.0, 0.0),
            (7.0, 0.0),
        ]
        .into_iter()
        .map(|(x, y)| c + vec2(x, y))
        .collect(),
        Stroke::new(1.2_f32, color),
    ));
    response
        .on_hover_text(if plotted {
            "隐藏波形"
        } else {
            "显示波形"
        })
        .clicked()
}

fn fixed_label(ui: &mut egui::Ui, text: &str, width: f32, right: bool) -> egui::Response {
    let label = egui::Label::new(egui::RichText::new(text).monospace().size(12.0)).truncate();
    ui.allocate_ui_with_layout(
        vec2(width, 17.0),
        if right {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        },
        |ui| ui.add(label),
    )
    .inner
}

fn display_name(display: Display) -> &'static str {
    match display {
        Display::Hex => "HEX",
        Display::Decimal => "DEC",
        Display::Bits => "BIN",
    }
}

#[derive(Default)]
pub struct Timeline {
    past_end: Option<(u64, f64)>,
    hover: Option<(u64, Duration)>,
}

const GUTTER: f32 = 142.0;
const GRID: Color32 = Color32::from_rgb(43, 58, 71);
const CROSSHAIR: Color32 = Color32::from_rgb(114, 173, 190);

struct Plot {
    range: (f64, f64),
    width: f32,
    hover: Option<Duration>,
}

impl Plot {
    fn chart(&self, area: Rect) -> Rect {
        Rect::from_min_max(pos2(area.left() + GUTTER, area.top()), area.right_bottom())
    }

    fn x(&self, chart: Rect, time: f64) -> f32 {
        chart.left()
            + ((time - self.range.0) / (self.range.1 - self.range.0)) as f32 * chart.width()
    }

    fn time_at(&self, chart: Rect, x: f32) -> Duration {
        let fraction = f64::from(((x - chart.left()) / chart.width()).clamp(0.0, 1.0));
        Duration::from_secs_f64(self.range.0 + fraction * (self.range.1 - self.range.0))
    }

    fn ticks(&self) -> impl Iterator<Item = f64> {
        let (start, end) = self.range;
        (0..=6).map(move |index| start + (end - start) * f64::from(index) / 6.0)
    }
}

impl Timeline {
    pub fn show(&mut self, app: &mut Advanced, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("时间轴与事件");
            if ui
                .add_enabled(
                    app.history.cursor.is_none() && !app.history.frames.is_empty(),
                    egui::Button::new("暂停观察"),
                )
                .clicked()
            {
                if let Some(last) = app.history.frames.back() {
                    app.history.select(last.id);
                }
            }
            if ui
                .add_enabled(app.history.cursor.is_some(), egui::Button::new("恢复实时"))
                .clicked()
            {
                app.history.cursor = None;
                self.past_end = None;
            }
            if let Some(frame) = &app.history.cursor {
                ui.colored_label(
                    WARN,
                    format!("游标 #{} · {:.6} s", frame.id, frame.time.as_secs_f64()),
                );
            }
            if let Some((id, time)) = self.hover {
                ui.colored_label(
                    CROSSHAIR,
                    format!("悬停 #{id} · {:.6} s", time.as_secs_f64()),
                );
            }
        });
        let Some(last) = app.history.frames.back() else {
            self.past_end = None;
            self.hover = None;
            ui.weak("启动 PDO 后记录逐周期数据。");
            return;
        };
        let latest = last.time.as_secs_f64();
        let end = if let Some(cursor) = &app.history.cursor {
            let end = self
                .past_end
                .filter(|(id, _)| *id == cursor.id)
                .map_or(latest.max(cursor.time.as_secs_f64()), |(_, end)| end);
            let selected = cursor.time.as_secs_f64();
            let end = if selected < end - 3.0 {
                (selected + 1.5).min(latest)
            } else {
                end
            };
            self.past_end = Some((cursor.id, end));
            end
        } else {
            self.past_end = None;
            latest
        };
        let range = ((end - 3.0).max(0.0), end.max(0.05));
        let mut hit = None;
        let station = app.selected.unwrap_or(0);
        let fault_indices = app.fault_indices();
        let width = ui.available_width();
        let mut hover = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((width - 230.0).max(240.0));
                let mut plot = Plot {
                    range,
                    width: ui.available_width() - 12.0,
                    hover: None,
                };
                let region = Rect::from_min_size(
                    ui.next_widget_position(),
                    vec2(plot.width, ui.available_height()),
                );
                let active = plot.chart(region);
                if ui.rect_contains_pointer(active) {
                    if let Some(frame) = ui
                        .input(|input| input.pointer.hover_pos())
                        .and_then(|p| nearest(&app.history, plot.time_at(active, p.x)))
                    {
                        hover = Some((frame.id, frame.time));
                        plot.hover = Some(frame.time);
                    }
                }
                hit = draw_axis(ui, &app.history, &plot).or(hit);
                egui::ScrollArea::vertical()
                    .id_salt("trace_lanes")
                    .show(ui, |ui| {
                        hit = draw_lane(
                            ui,
                            &app.history,
                            &plot,
                            "WKC",
                            GOOD,
                            Some((0.0, f64::from(last.expected.max(last.wkc).max(1)))),
                            |f| u32::try_from(f.wkc).ok().map(f64::from),
                        )
                        .or(hit);
                        for watch in app
                            .watches
                            .iter()
                            .filter(|watch| watch.plotted && watch.key.source != Source::Wkc)
                        {
                            hit = draw_lane(
                                ui,
                                &app.history,
                                &plot,
                                &watch.key.caption(),
                                COLORS[watch.color % COLORS.len()],
                                None,
                                |f| watch.value(f).map(|value| value as f64),
                            )
                            .or(hit);
                        }
                        hit = draw_lane(
                            ui,
                            &app.history,
                            &plot,
                            &format!("AL {station:02}（轮询）"),
                            WARN,
                            Some((0.0, 8.0)),
                            |f| {
                                f.al.iter()
                                    .find(|al| al.station == station)?
                                    .state
                                    .map(f64::from)
                            },
                        )
                        .or(hit);
                        if !fault_indices.is_empty() {
                            hit = draw_lane(
                                ui,
                                &app.history,
                                &plot,
                                "CiA402 故障",
                                FAULT,
                                Some((0.0, 1.0)),
                                |f| {
                                    f.valid.then(|| {
                                        f64::from(u8::from(fault_indices.iter().any(|&id| {
                                            f.values
                                                .get(id)
                                                .copied()
                                                .flatten()
                                                .is_some_and(|word| word & 8 != 0)
                                        })))
                                    })
                                },
                            )
                            .or(hit);
                        }
                        hit = draw_lane(
                            ui,
                            &app.history,
                            &plot,
                            "周期偏差 µs",
                            COLORS[5],
                            None,
                            |f| f.jitter_us,
                        )
                        .or(hit);
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.heading("事件");
                ui.set_width(208.0);
                egui::ScrollArea::vertical()
                    .id_salt("trace_events")
                    .show(ui, |ui| {
                        if app.history.markers.is_empty() {
                            ui.weak("当前记录无异常");
                        }
                        for marker in app.history.markers.iter().rev().take(100) {
                            let is_selected = app
                                .history
                                .cursor
                                .as_ref()
                                .is_some_and(|cursor| cursor.id == marker.id);
                            let text =
                                egui::RichText::new(format!("#{}  {}", marker.id, marker.text))
                                    .color(if marker.alarm { FAULT } else { WARN });
                            if ui.selectable_label(is_selected, text).clicked() {
                                hit = Some((
                                    marker.id,
                                    (marker.time.as_secs_f64() + 1.5).min(latest),
                                ));
                            }
                        }
                    });
            });
        });
        if self.hover != hover {
            self.hover = hover;
            ui.ctx().request_repaint();
        }
        if let Some((id, end)) = hit {
            app.history.select(id);
            self.past_end = Some((id, end));
        }
    }
}

fn nearest(history: &History, time: Duration) -> Option<&Recorded> {
    let idx = history.frames.partition_point(|frame| frame.time < time);
    let before = idx.checked_sub(1).and_then(|i| history.frames.get(i));
    let after = history.frames.get(idx);
    match (before, after) {
        (Some(a), Some(b)) if time - a.time <= b.time - time => Some(a),
        (_, Some(b)) => Some(b),
        (a, None) => a,
    }
}

fn hit_cursor(
    response: &egui::Response,
    history: &History,
    plot: &Plot,
    chart: Rect,
) -> Option<(u64, f64)> {
    if !response.clicked() && !response.dragged() {
        return None;
    }
    let pos = response
        .interact_pointer_pos()
        .filter(|pos| chart.contains(*pos))?;
    if response.clicked() {
        let distance = |time: Duration| (plot.x(chart, time.as_secs_f64()) - pos.x).abs();
        if let Some(marker) = history
            .markers
            .iter()
            .filter(|marker| (plot.range.0..=plot.range.1).contains(&marker.time.as_secs_f64()))
            .min_by(|a, b| distance(a.time).total_cmp(&distance(b.time)))
            .filter(|marker| distance(marker.time) <= 7.0)
        {
            return Some((marker.id, plot.range.1));
        }
    }
    nearest(history, plot.time_at(chart, pos.x)).map(|frame| (frame.id, plot.range.1))
}

fn paint_cursors(painter: &egui::Painter, plot: &Plot, chart: Rect, history: &History) {
    for (time, color) in [
        (plot.hover, CROSSHAIR),
        (history.observed().map(|frame| frame.time), Color32::WHITE),
    ] {
        if let Some(time) = time {
            let x = plot.x(chart, time.as_secs_f64());
            if x >= chart.left() && x <= chart.right() {
                let x = x.clamp(chart.left() + 0.5, chart.right() - 0.5);
                painter.line_segment(
                    [pos2(x, chart.top()), pos2(x, chart.bottom())],
                    Stroke::new(1.0_f32, color),
                );
            }
        }
    }
}

fn draw_axis(ui: &mut egui::Ui, history: &History, plot: &Plot) -> Option<(u64, f64)> {
    let (area, response) =
        ui.allocate_exact_size(vec2(plot.width, 26.0), egui::Sense::click_and_drag());
    let chart = plot.chart(area);
    let painter = ui.painter();
    painter.text(
        pos2(area.left(), area.center().y),
        Align2::LEFT_CENTER,
        "时间 / s",
        FontId::proportional(11.0),
        ui.visuals().weak_text_color(),
    );
    for (index, tick) in plot.ticks().enumerate() {
        let x = plot.x(chart, tick);
        let anchor = match index {
            0 => Align2::LEFT_TOP,
            6 => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        painter.text(
            pos2(x, chart.top()),
            anchor,
            format!("{tick:.2}"),
            FontId::monospace(10.0),
            ui.visuals().weak_text_color(),
        );
        painter.line_segment(
            [pos2(x, chart.bottom() - 7.0), pos2(x, chart.bottom())],
            Stroke::new(1.0_f32, GRID),
        );
    }
    let layer = painter.with_clip_rect(chart.intersect(ui.clip_rect()));
    for marker in history
        .markers
        .iter()
        .filter(|m| (plot.range.0..=plot.range.1).contains(&m.time.as_secs_f64()))
    {
        let x = plot.x(chart, marker.time.as_secs_f64());
        let y = chart.bottom() - 5.0;
        layer.add(egui::Shape::convex_polygon(
            vec![
                pos2(x, y - 5.0),
                pos2(x + 5.0, y),
                pos2(x, y + 5.0),
                pos2(x - 5.0, y),
            ],
            if marker.alarm { FAULT } else { WARN },
            Stroke::NONE,
        ));
    }
    paint_cursors(&layer, plot, chart, history);
    hit_cursor(&response, history, plot, chart)
}

fn draw_lane(
    ui: &mut egui::Ui,
    history: &History,
    plot: &Plot,
    label: &str,
    color: Color32,
    baseline: Option<(f64, f64)>,
    value: impl Fn(&Recorded) -> Option<f64>,
) -> Option<(u64, f64)> {
    let range = plot.range;
    let (area, response) =
        ui.allocate_exact_size(vec2(plot.width, 36.0), egui::Sense::click_and_drag());
    let chart = plot.chart(area);
    if !ui.is_rect_visible(area) || chart.width() <= 1.0 {
        return None;
    }
    let painter = ui.painter();
    painter.text(
        pos2(area.left(), area.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.0),
        color,
    );
    painter.rect_filled(chart, 2.0, ui.visuals().extreme_bg_color);
    let xpos = |t| plot.x(chart, t);
    let begin = history
        .frames
        .partition_point(|f| f.time.as_secs_f64() < range.0);
    let end = history
        .frames
        .partition_point(|f| f.time.as_secs_f64() <= range.1);
    let visible = || history.frames.range(begin..end);
    let (mut low, mut high) = baseline.unwrap_or((f64::INFINITY, f64::NEG_INFINITY));
    for frame in visible() {
        if let Some(v) = value(frame) {
            low = low.min(v);
            high = high.max(v);
        }
    }
    if !low.is_finite() || !high.is_finite() {
        low = 0.0;
        high = 1.0;
    }
    if high == low {
        low -= 0.5;
        high += 0.5;
    }
    let span = high - low;
    let y = |v: f64| chart.bottom() - 3.0 - ((v - low) / span) as f32 * (chart.height() - 6.0);
    let trace = painter.with_clip_rect(chart.intersect(ui.clip_rect()));
    for tick in plot.ticks() {
        let x = xpos(tick);
        trace.line_segment(
            [pos2(x, chart.top()), pos2(x, chart.bottom())],
            Stroke::new(1.0_f32, GRID),
        );
    }
    for level in [low, (low + high) / 2.0, high] {
        let yy = y(level);
        trace.line_segment(
            [pos2(chart.left(), yy), pos2(chart.right(), yy)],
            Stroke::new(1.0_f32, GRID),
        );
    }
    let format_tick = |value: f64| {
        if value.abs() >= 10_000.0 {
            format!("{value:.2e}")
        } else if value.fract() == 0.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.2}")
        }
    };
    for level in [low, high] {
        painter.text(
            pos2(chart.left() - 5.0, y(level)),
            Align2::RIGHT_CENTER,
            format_tick(level),
            FontId::monospace(9.0),
            ui.visuals().weak_text_color(),
        );
    }
    let stroke = Stroke::new(1.2_f32, color);
    // Keep each pixel column's extrema, and break at invalid samples.
    let mut bucket: Option<(f32, f64, f64, f64)> = None;
    let mut prev = None;
    let flush = |bucket: &mut Option<(f32, f64, f64, f64)>, prev: &mut Option<egui::Pos2>| {
        if let Some((x, min, max, last)) = bucket.take() {
            let point = pos2(x, y(last));
            trace.line_segment([pos2(x, y(min)), pos2(x, y(max))], stroke);
            if let Some(before) = prev.replace(point) {
                trace.line_segment([before, point], stroke);
            } else if min == max {
                trace.circle_filled(point, 1.0, color);
            }
        }
    };
    let mut last_id = None;
    for frame in visible() {
        if last_id.is_some_and(|id| frame.id != id + 1) {
            flush(&mut bucket, &mut prev);
            prev = None;
        }
        last_id = Some(frame.id);
        let Some(v) = value(frame) else {
            flush(&mut bucket, &mut prev);
            prev = None;
            continue;
        };
        let x = xpos(frame.time.as_secs_f64()).floor();
        if let Some((column, min, max, last)) = &mut bucket {
            if *column == x {
                *min = min.min(v);
                *max = max.max(v);
                *last = v;
                continue;
            }
        }
        flush(&mut bucket, &mut prev);
        bucket = Some((x, v, v, v));
    }
    flush(&mut bucket, &mut prev);
    paint_cursors(&trace, plot, chart, history);
    if let Some(pos) = response.hover_pos().filter(|pos| chart.contains(*pos)) {
        trace.line_segment(
            [pos2(chart.left(), pos.y), pos2(chart.right(), pos.y)],
            Stroke::new(1.0_f32, CROSSHAIR.gamma_multiply(0.55)),
        );
    }
    hit_cursor(&response, history, plot, chart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};
    use std::sync::Arc;

    #[test]
    fn marker_and_event_row_freeze_watch_while_capture_advances() {
        let ctx = egui::Context::default();
        let mut app = Advanced::default();
        let mut timeline = Timeline::default();
        app.add_watch(Key::wkc(), None, true);
        let hpm = ethercat_monitor::service::process::Entry {
            station: 0,
            output: false,
            pdo: 0x1a00,
            index: 0x6000,
            sub: 0,
            width: 32,
            offset: 32,
        };
        for source in [Source::Pdo, Source::Gpio, Source::Pdi] {
            app.add_watch(Key::entry(&hpm, source), Some(0), true);
        }
        let al = Arc::new(Vec::new());
        let record = |id, wkc| Recorded {
            id,
            time: Duration::from_millis(id * 10),
            values: if wkc == 0 {
                Box::new([])
            } else {
                Box::new([Some(0x0001_0000 | id)])
            },
            valid: wkc != 0,
            wkc,
            expected: 3,
            jitter_us: None,
            al: al.clone(),
        };
        for frame in [record(1, 3), record(2, 0), record(3, 3)] {
            app.history.push(frame);
        }
        let render = |app: &mut Advanced, timeline: &mut Timeline, events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        timeline.show(app, ui);
                    });
                },
            )
        };
        let frame = render(&mut app, &mut timeline, vec![]);
        for label in ["WKC", "00  6000:00", "00 GPIO", "00 PDI 相位 (µs)"] {
            assert!(
                frame.shapes.iter().any(|item| {
                    matches!(&item.shape, egui::Shape::Text(text) if text.galley.text() == label)
                }),
                "lane {label} should be visible"
            );
        }
        let marker = frame
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                egui::Shape::Path(shape) if shape.fill == FAULT && shape.points.len() == 4 => {
                    Some(shape.points[0].lerp(shape.points[2], 0.5))
                }
                _ => None,
            })
            .expect("red WKC diamond on the time axis");
        let row = frame
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                egui::Shape::Text(shape) if shape.galley.text().starts_with("#2  WKC") => {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .expect("WKC marker row");
        render(&mut app, &mut timeline, vec![Event::PointerMoved(marker)]);
        assert_eq!(timeline.hover, Some((2, Duration::from_millis(20))));
        assert!(
            app.history.cursor.is_none(),
            "hover must not freeze observation"
        );
        for pos in [marker, row] {
            app.history.cursor = None;
            render(&mut app, &mut timeline, vec![Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                render(
                    &mut app,
                    &mut timeline,
                    vec![Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    }],
                );
            }
            assert_eq!(app.history.observed().unwrap().id, 2);
        }
        app.history.push(record(4, 3));
        let observed = app.history.observed().unwrap();
        assert_eq!(observed.id, 2);
        assert_eq!(app.watches[0].value(observed), Some(0));
        assert!(app.watches[1..]
            .iter()
            .all(|watch| watch.value(observed).is_none()));
        assert_eq!(app.history.frames.back().unwrap().id, 4);
        assert_eq!(app.history.frames.back().unwrap().wkc, 3);
    }
}
