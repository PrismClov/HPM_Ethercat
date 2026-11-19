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
    ui.horizontal(|ui| {
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
            ui.colored_label(WARN, format!("历史快照 #{} · 实时 #{}", frame.id, live));
        } else {
            ui.colored_label(GOOD, format!("实时周期 #{}", frame.id));
        }
        ui.weak(format!(
            "{:.6} s · WKC {}",
            frame.time.as_secs_f64(),
            frame.wkc
        ));
    }
    if app.watches.is_empty() {
        ui.weak("右键任意 PDO 行加入观察。");
        return;
    }
    let mut groups: Vec<_> = app
        .watches
        .iter()
        .map(|watch| watch.group.clone())
        .collect();
    groups.sort();
    groups.dedup();
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
                for (id, watch) in app
                    .watches
                    .iter_mut()
                    .enumerate()
                    .filter(|(_, w)| w.group == group)
                {
                    ui.push_id(id, |ui| {
                        let value = frame.and_then(|frame| watch.formatted(frame));
                        ui.horizontal_wrapped(|ui| {
                            ui.label(watch.key.caption()).on_hover_text(format!(
                                "{:04X}:{:02X} · PDO 0x{:04X} · {} 位 · 偏移 {}",
                                watch.key.index,
                                watch.key.sub,
                                watch.key.pdo,
                                watch.key.width,
                                watch.key.offset
                            ));
                            if let Some(value) = value {
                                ui.monospace(value).on_hover_text("该周期有效");
                            } else {
                                ui.weak(if watch.id.is_none() && watch.key.source != Source::Wkc {
                                    "未映射"
                                } else {
                                    "无有效值"
                                });
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut watch.plotted, "波形");
                            egui::ComboBox::from_id_salt("format")
                                .width(64.0)
                                .selected_text(display_name(watch.display))
                                .show_ui(ui, |ui| {
                                    for display in [Display::Hex, Display::Decimal, Display::Bits] {
                                        ui.selectable_value(
                                            &mut watch.display,
                                            display,
                                            display_name(display),
                                        );
                                    }
                                });
                            ui.menu_button("分组", |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut watch.group)
                                        .desired_width(110.0),
                                );
                            });
                            if ui.small_button("移除").clicked() {
                                remove = Some(id);
                            }
                        });
                        ui.separator();
                    });
                }
            });
        });
    }
    if let Some(id) = remove {
        app.watches.remove(id);
    }
}

fn display_name(display: Display) -> &'static str {
    match display {
        Display::Hex => "十六进制",
        Display::Decimal => "十进制",
        Display::Bits => "位域",
    }
}

#[derive(Default)]
pub struct Timeline {
    past_end: Option<(u64, f64)>,
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
        });
        let Some(last) = app.history.frames.back() else {
            self.past_end = None;
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
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((width - 230.0).max(240.0));
                ui.weak(format!(
                    "{:.3}–{:.3} s · 事件标记和游标共用周期时钟",
                    range.0, range.1
                ));
                egui::ScrollArea::vertical()
                    .id_salt("trace_lanes")
                    .show(ui, |ui| {
                        hit =
                            draw_lane(ui, &app.history, range, "WKC", GOOD, |f| Some(f.wkc as f64))
                                .or(hit);
                        for watch in app
                            .watches
                            .iter()
                            .filter(|watch| watch.plotted && watch.key.source != Source::Wkc)
                        {
                            hit = draw_lane(
                                ui,
                                &app.history,
                                range,
                                &watch.key.caption(),
                                COLORS[watch.color % COLORS.len()],
                                |f| watch.value(f).map(|value| value as f64),
                            )
                            .or(hit);
                        }
                        hit = draw_lane(
                            ui,
                            &app.history,
                            range,
                            &format!("AL {station:02}（轮询）"),
                            WARN,
                            |f| {
                                f.al.iter()
                                    .find(|al| al.station == station)?
                                    .state
                                    .map(f64::from)
                            },
                        )
                        .or(hit);
                        if !fault_indices.is_empty() {
                            hit = draw_lane(ui, &app.history, range, "CiA402 故障", FAULT, |f| {
                                f.valid.then(|| {
                                    f64::from(u8::from(fault_indices.iter().any(|&id| {
                                        f.values
                                            .get(id)
                                            .copied()
                                            .flatten()
                                            .is_some_and(|word| word & 8 != 0)
                                    })))
                                })
                            })
                            .or(hit);
                        }
                        hit = draw_lane(
                            ui,
                            &app.history,
                            range,
                            "周期偏差 µs",
                            COLORS[5],
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
        if let Some((id, end)) = hit {
            app.history.select(id);
            self.past_end = Some((id, end));
        }
    }
}

fn draw_lane(
    ui: &mut egui::Ui,
    history: &History,
    range: (f64, f64),
    label: &str,
    color: Color32,
    value: impl Fn(&Recorded) -> Option<f64>,
) -> Option<(u64, f64)> {
    let (area, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), 34.0),
        egui::Sense::click_and_drag(),
    );
    let chart = Rect::from_min_max(
        pos2(area.left() + 112.0, area.top() + 2.0),
        area.right_bottom(),
    );
    if !ui.is_rect_visible(area) || chart.width() <= 0.0 {
        return None;
    }
    let painter = ui.painter();
    painter.text(
        pos2(area.left(), area.top() + 11.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(11.0),
        color,
    );
    painter.rect_filled(chart, 3.0, ui.visuals().extreme_bg_color);
    let xpos = |t: f64| chart.left() + ((t - range.0) / (range.1 - range.0)) as f32 * chart.width();
    let begin = history
        .frames
        .partition_point(|f| f.time.as_secs_f64() < range.0);
    let end = history
        .frames
        .partition_point(|f| f.time.as_secs_f64() <= range.1);
    let visible = || history.frames.range(begin..end);
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for frame in visible() {
        if let Some(v) = value(frame) {
            low = low.min(v);
            high = high.max(v);
        }
    }
    let span = (high - low).max(1.0);
    let y = |v: f64| chart.bottom() - 5.0 - ((v - low) / span) as f32 * (chart.height() - 10.0);
    let trace = painter.with_clip_rect(chart.intersect(ui.clip_rect()));
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
    for marker in history
        .markers
        .iter()
        .filter(|marker| (range.0..=range.1).contains(&marker.time.as_secs_f64()))
    {
        let x = xpos(marker.time.as_secs_f64());
        trace.circle_filled(
            pos2(x, chart.top() + 4.0),
            2.6,
            if marker.alarm { FAULT } else { WARN },
        );
    }
    if let Some(cursor) = &history.cursor {
        let x = xpos(cursor.time.as_secs_f64());
        trace.line_segment(
            [pos2(x, chart.top()), pos2(x, chart.bottom())],
            Stroke::new(1.0_f32, Color32::WHITE),
        );
    }
    if response.clicked() || response.dragged() {
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|pos| chart.contains(*pos))
        {
            if response.clicked() {
                if let Some(marker) = history
                    .markers
                    .iter()
                    .filter(|m| (range.0..=range.1).contains(&m.time.as_secs_f64()))
                    .min_by(|left, right| {
                        (xpos(left.time.as_secs_f64()) - pos.x)
                            .abs()
                            .total_cmp(&(xpos(right.time.as_secs_f64()) - pos.x).abs())
                    })
                    .filter(|mark| (xpos(mark.time.as_secs_f64()) - pos.x).abs() <= 5.0)
                {
                    return Some((marker.id, range.1));
                }
            }
            let t = Duration::from_secs_f64(
                range.0 + f64::from((pos.x - chart.left()) / chart.width()) * (range.1 - range.0),
            );
            let idx = history.frames.partition_point(|f| f.time < t);
            let before = idx.checked_sub(1).and_then(|i| history.frames.get(i));
            let after = history.frames.get(idx);
            let nearest = match (before, after) {
                (Some(a), Some(b)) if t - a.time <= b.time - t => Some(a),
                (_, Some(b)) => Some(b),
                (a, None) => a,
            };
            return nearest.map(|frame| (frame.id, range.1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};
    use std::sync::Arc;

    #[test]
    fn event_click_freezes_watch_at_event_cycle_while_capture_advances() {
        let ctx = egui::Context::default();
        let mut app = Advanced::default();
        let mut timeline = Timeline::default();
        app.add_watch(Key::wkc(), None, true);
        let al = Arc::new(Vec::new());
        let record = |id, wkc| Recorded {
            id,
            time: Duration::from_millis(id * 10),
            values: Box::new([]),
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
        let pos = frame
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                egui::Shape::Text(shape) if shape.galley.text().starts_with("#2  WKC") => {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .expect("WKC marker row");
        for pressed in [true, false] {
            render(
                &mut app,
                &mut timeline,
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        app.history.push(record(4, 3));
        let observed = app.history.observed().unwrap();
        assert_eq!(observed.id, 2);
        assert_eq!(app.watches[0].value(observed), Some(0));
        assert_eq!(app.history.frames.back().unwrap().id, 4);
        assert_eq!(app.history.frames.back().unwrap().wkc, 3);
    }
}
