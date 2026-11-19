use crate::{
    advanced::{number, Advanced},
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
const WATCH_ROW_HEIGHT: f32 = 24.0;
const WATCH_COLUMN_GAP: f32 = 4.0;
const WATCH_DIRECTION_WIDTH: f32 = 22.0;
const WATCH_FORMAT_WIDTH: f32 = 36.0;
const WATCH_ACTION_WIDTH: f32 = 20.0;
const WATCH_CELL_PADDING: i8 = 2;

pub(super) struct WatchEdit {
    key: Key,
    draft: String,
    focus: bool,
}

pub fn observations(app: &mut Advanced, ui: &mut egui::Ui) {
    let mut edit = app.watch_edit.take();
    ui.horizontal_wrapped(|ui| {
        ui.heading("观察变量");
        if ui.small_button("+ WKC").clicked() {
            app.add_watch(Key::wkc(), None, false);
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
        ui.weak("启动 PDO 后会自动列出从站变量。");
        return;
    }
    let split_keys: Vec<_> = app
        .watches
        .iter()
        .filter(|watch| watch.id.is_some() && app.can_split(&watch.key))
        .map(|watch| watch.key.clone())
        .collect();
    let mut groups: Vec<_> = app
        .watches
        .iter()
        .map(|watch| watch.group.clone())
        .collect();
    groups.sort();
    groups.dedup();
    let writable = app.writable();
    let mut next_edit = None;
    let mut write = None;
    let mut remove = None;
    let mut split = None;
    for group in groups {
        ui.push_id(&group, |ui| {
            egui::CollapsingHeader::new(if group.trim().is_empty() {
                "默认"
            } else {
                &group
            })
            .default_open(true)
            .show(ui, |ui| {
                let actions_width = 2.0 * WATCH_ACTION_WIDTH + WATCH_COLUMN_GAP;
                let data_width = ui.available_width()
                    - WATCH_DIRECTION_WIDTH
                    - WATCH_FORMAT_WIDTH
                    - actions_width
                    - 4.0 * WATCH_COLUMN_GAP;
                let name_width = (data_width * 0.42).round().clamp(20.0, 96.0);
                let value_width = (data_width - name_width).floor().clamp(32.0, 124.0);
                ui.spacing_mut().item_spacing = vec2(WATCH_COLUMN_GAP, 4.0);
                ui.horizontal(|ui| {
                    for (title, width, padding) in [
                        ("I/O", WATCH_DIRECTION_WIDTH, 0),
                        ("变量", name_width, WATCH_CELL_PADDING),
                        ("值", value_width, WATCH_CELL_PADDING),
                        ("格式", WATCH_FORMAT_WIDTH, 0),
                    ] {
                        watch_cell(ui, width, padding, |ui| {
                            ui.add(egui::Label::new(egui::RichText::new(title).weak()).truncate())
                        });
                    }
                });
                let mut rows: Vec<_> = app
                    .watches
                    .iter_mut()
                    .enumerate()
                    .filter(|(_, w)| w.group == group)
                    .map(|(id, watch)| {
                        let section = match watch.key.source {
                            Source::Wkc => 2,
                            _ if watch.key.output => 0,
                            _ => 1,
                        };
                        (section, id, watch)
                    })
                    .collect();
                rows.sort_by_key(|(section, _, _)| *section);
                let mut previous_section = None;
                for (row, (section, id, watch)) in rows.into_iter().enumerate() {
                    if previous_section != Some(section) {
                        if previous_section.is_some() {
                            ui.add_space(4.0);
                        }
                        let (title, color) = [
                            ("Rx · 从站接收", COLORS[0]),
                            ("Tx · 从站发送", GOOD),
                            ("总线状态", ui.visuals().weak_text_color()),
                        ][section];
                        ui.label(egui::RichText::new(title).size(12.0).strong().color(color));
                        previous_section = Some(section);
                    }
                    let stripe = ui.painter().add(egui::Shape::Noop);
                    let response = ui
                        .push_id(id, |ui| {
                            ui.horizontal(|ui| {
                                let value = frame.and_then(|frame| watch.formatted(frame));
                                let validity = if value.is_some() {
                                    "有效"
                                } else if watch.id.is_none() && watch.key.source != Source::Wkc {
                                    "未映射"
                                } else {
                                    "该周期无有效值"
                                };
                                let (direction, description, color) = match watch.key.source {
                                    Source::Wkc => ("●", "总线状态", GOOD),
                                    _ if watch.key.output => {
                                        ("Rx", "主站 → 从站（从站接收）", COLORS[0])
                                    }
                                    _ => ("Tx", "从站 → 主站（从站发送）", GOOD),
                                };
                                watch_cell(ui, WATCH_DIRECTION_WIDTH, 0, |ui| {
                                    let color = value
                                        .as_ref()
                                        .map_or_else(|| ui.visuals().weak_text_color(), |_| color);
                                    ui.label(
                                        egui::RichText::new(direction)
                                            .monospace()
                                            .size(12.0)
                                            .color(color),
                                    )
                                })
                                .on_hover_text(format!("{description} · {validity}"));
                                let caption = match watch.key.source {
                                    Source::Pdo => {
                                        format!("{:04X}:{:02X}", watch.key.index, watch.key.sub)
                                    }
                                    Source::Gpio => "GPIO".into(),
                                    Source::Pdi => "PDI 相位".into(),
                                    Source::Wkc => "WKC".into(),
                                };
                                fixed_label(ui, &caption, name_width, egui::Sense::hover())
                                    .on_hover_text(format!(
                                        "{} · {}PDO 0x{:04X} · {} 位 · 偏移 {}",
                                        watch.key.caption(),
                                        if watch.key.output { "Rx" } else { "Tx" },
                                        watch.key.pdo,
                                        watch.key.width,
                                        watch.key.offset
                                    ));
                                let editable = writable
                                    && value.is_some()
                                    && watch.key.source == Source::Pdo
                                    && watch.key.output
                                    && (1..=64).contains(&watch.key.width)
                                    && watch.id.is_some();
                                let value = value.unwrap_or_else(|| "—".into());
                                let sense = if editable {
                                    egui::Sense::click()
                                } else {
                                    egui::Sense::hover()
                                };
                                if let Some(mut active) =
                                    edit.take_if(|active| editable && active.key == watch.key)
                                {
                                    let editor_id = ui.id().with(("watch_value", watch.id));
                                    let focused = ui.memory(|memory| memory.has_focus(editor_id));
                                    let (submit, cancel) = if focused {
                                        ui.input_mut(|input| {
                                            (
                                                input.consume_key(
                                                    egui::Modifiers::NONE,
                                                    egui::Key::Enter,
                                                ),
                                                input.consume_key(
                                                    egui::Modifiers::NONE,
                                                    egui::Key::Escape,
                                                ),
                                            )
                                        })
                                    } else {
                                        (false, false)
                                    };
                                    let parsed = number(&active.draft).filter(|value| {
                                        value.checked_shr(u32::from(watch.key.width)).unwrap_or(0)
                                            == 0
                                    });
                                    let editor = egui::TextEdit::singleline(&mut active.draft)
                                        .id(editor_id)
                                        .font(egui::FontId::monospace(12.0))
                                        .vertical_align(egui::Align::Center)
                                        .margin(egui::Margin::symmetric(WATCH_CELL_PADDING, 0))
                                        .min_size(vec2(value_width, WATCH_ROW_HEIGHT));
                                    let editor = if parsed.is_none() {
                                        editor.text_color(FAULT)
                                    } else {
                                        editor
                                    };
                                    let response =
                                        watch_cell(ui, value_width, 0, |ui| ui.add(editor))
                                            .on_hover_text(
                                                "支持十进制或 0x 十六进制 · Enter 写入 · Esc 取消",
                                            );
                                    if active.focus {
                                        response.request_focus();
                                        if let Some(mut state) =
                                            egui::TextEdit::load_state(ui.ctx(), editor_id)
                                        {
                                            state.cursor.set_char_range(Some(
                                                egui::text::CCursorRange::two(
                                                    egui::text::CCursor::new(0),
                                                    egui::text::CCursor::new(
                                                        active.draft.chars().count(),
                                                    ),
                                                ),
                                            ));
                                            state.store(ui.ctx(), editor_id);
                                        }
                                        active.focus = false;
                                    }
                                    if submit && !cancel && parsed.is_some() {
                                        write = parsed.map(|value| (active.key, value));
                                    } else if !cancel && response.has_focus() && next_edit.is_none()
                                    {
                                        next_edit = Some(active);
                                    }
                                } else if editable {
                                    let cell = fixed_label(ui, &value, value_width, sense);
                                    if cell
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(format!("{value}\n双击编辑输出值"))
                                        .double_clicked()
                                    {
                                        let draft = if watch.display == Display::Bits {
                                            frame
                                                .and_then(|frame| watch.value(frame))
                                                .map_or_else(String::new, |value| value.to_string())
                                        } else {
                                            value
                                        };
                                        next_edit = Some(WatchEdit {
                                            key: watch.key.clone(),
                                            draft,
                                            focus: true,
                                        });
                                    }
                                } else {
                                    fixed_label(ui, &value, value_width, sense)
                                        .on_hover_text(&value);
                                }
                                watch_cell(ui, WATCH_FORMAT_WIDTH, 0, |ui| {
                                    let format = egui::Button::new(display_name(watch.display))
                                        .small()
                                        .min_size(vec2(WATCH_FORMAT_WIDTH, WATCH_ROW_HEIGHT));
                                    egui::menu::menu_custom_button(ui, format, |ui| {
                                        for (display, label) in [
                                            (Display::Hex, "十六进制"),
                                            (Display::Decimal, "十进制"),
                                            (Display::Bits, "位域"),
                                        ] {
                                            if ui
                                                .selectable_value(
                                                    &mut watch.display,
                                                    display,
                                                    label,
                                                )
                                                .clicked()
                                            {
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                });
                                watch_cell(ui, actions_width, 0, |ui| {
                                    ui.spacing_mut().item_spacing.x = WATCH_COLUMN_GAP;
                                    ui.spacing_mut().button_padding.x = 2.0;
                                    if wave_toggle(
                                        ui,
                                        watch.plotted,
                                        watch.key.source != Source::Wkc,
                                    ) {
                                        watch.plotted = !watch.plotted;
                                    }
                                    let group_edit = ui.id().with("group_edit");
                                    let more = egui::Button::new("···")
                                        .small()
                                        .min_size(vec2(WATCH_ACTION_WIDTH, WATCH_ROW_HEIGHT));
                                    let menu = egui::menu::menu_custom_button(ui, more, |ui| {
                                        if watch.key.source == Source::Pdo && watch.key.width <= 64
                                        {
                                            if let Some(value) =
                                                frame.and_then(|frame| watch.value(frame))
                                            {
                                                ui.label("位图（高位在左）");
                                                ui.monospace(format!(
                                                    "{value:0width$b}",
                                                    width = usize::from(watch.key.width)
                                                ));
                                            }
                                        }
                                        if split_keys.contains(&watch.key) {
                                            for (label, source) in [
                                                ("GPIO 加入波形", Source::Gpio),
                                                ("PDI 相位加入波形", Source::Pdi),
                                            ] {
                                                if ui.button(label).clicked() {
                                                    let mut key = watch.key.clone();
                                                    key.source = source;
                                                    split = Some((key, watch.id));
                                                    ui.close_menu();
                                                }
                                            }
                                        }
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
                                        if (watch.key.source != Source::Pdo || watch.id.is_none())
                                            && ui.small_button("移除").clicked()
                                        {
                                            remove = Some(id);
                                            ui.close_menu();
                                        }
                                    });
                                    menu.response.on_hover_text("分组或移除");
                                });
                            })
                            .response
                        })
                        .inner;
                    if row % 2 == 0 {
                        ui.painter().set(
                            stripe,
                            egui::Shape::rect_filled(
                                response.rect,
                                2.0,
                                ui.visuals().faint_bg_color,
                            ),
                        );
                    }
                }
            });
        });
    }
    if let Some(id) = remove {
        let removed = app.watches.remove(id);
        if next_edit
            .as_ref()
            .is_some_and(|active| active.key == removed.key)
        {
            next_edit = None;
        }
    }
    app.watch_edit = next_edit;
    if let Some((key, id)) = split {
        app.add_watch(key, id, true);
    }
    if let Some((key, value)) = write {
        app.write_observed_output(&key, value);
    }
}

fn wave_toggle(ui: &mut egui::Ui, plotted: bool, enabled: bool) -> bool {
    let plotted = plotted && enabled;
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .small()
            .selected(plotted)
            .min_size(vec2(WATCH_ACTION_WIDTH, WATCH_ROW_HEIGHT)),
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

fn watch_cell<R>(
    ui: &mut egui::Ui,
    width: f32,
    padding: i8,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, WATCH_ROW_HEIGHT), egui::Sense::hover());
    let inner = rect.shrink2(vec2(f32::from(padding), 0.0));
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(response.id)
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    cell.set_clip_rect(cell.clip_rect().intersect(rect));
    contents(&mut cell)
}

fn fixed_label(ui: &mut egui::Ui, text: &str, width: f32, sense: egui::Sense) -> egui::Response {
    let label = egui::Label::new(egui::RichText::new(text).monospace().size(12.0))
        .truncate()
        .sense(sense)
        .halign(egui::Align::LEFT);
    let label = if sense.senses_click() {
        label.selectable(false)
    } else {
        label
    };
    watch_cell(ui, width, WATCH_CELL_PADDING, |ui| {
        // Reserve the same space for the displayed value and the inline editor.
        ui.add_sized(ui.available_size(), label)
    })
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

const GUTTER: f32 = 42.0;
const GRID: Color32 = Color32::from_rgb(43, 58, 71);
const CROSSHAIR: Color32 = Color32::from_rgb(114, 173, 190);

struct Plot {
    range: (f64, f64),
    width: f32,
    hover: Option<Duration>,
}

impl Plot {
    fn chart(&self, area: Rect) -> Rect {
        Rect::from_min_max(
            pos2(area.left() + GUTTER.min(area.width() * 0.40), area.top()),
            area.right_bottom(),
        )
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
        if app.history.cursor.is_some()
            && app.watch_edit.is_none()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            app.history.cursor = None;
            self.past_end = None;
        }
        ui.horizontal_wrapped(|ui| {
            ui.heading("波形");
            if let Some(frame) = &app.history.cursor {
                ui.colored_label(
                    WARN,
                    format!("游标 #{} · {:.6} s", frame.id, frame.time.as_secs_f64()),
                )
                .on_hover_text("按 Esc 返回实时波形");
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
        let watches = || {
            app.watches
                .iter()
                .filter(|watch| watch.plotted && watch.key.source != Source::Wkc)
        };
        ui.horizontal_wrapped(|ui| {
            for watch in watches() {
                ui.colored_label(COLORS[watch.color % COLORS.len()], watch.key.caption());
            }
            ui.weak(if watches().next().is_none() {
                "点右栏变量的波形按钮添加曲线。"
            } else {
                "各通道独立归一化"
            });
        });

        let mut plot = Plot {
            range,
            width: ui.available_width().max(1.0),
            hover: None,
        };
        let region = Rect::from_min_size(
            ui.next_widget_position(),
            vec2(plot.width, ui.available_height()),
        );
        let active = plot.chart(region);
        let hover = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|_| ui.rect_contains_pointer(active))
            .and_then(|p| nearest(&app.history, plot.time_at(active, p.x)))
            .map(|frame| (frame.id, frame.time));
        plot.hover = hover.map(|(_, time)| time);

        let axis_hit = draw_axis(ui, &app.history, &plot);
        let (area, response) = ui.allocate_exact_size(
            vec2(plot.width, ui.available_height().max(1.0)),
            egui::Sense::click_and_drag(),
        );
        let chart = plot.chart(area);
        let painter = ui.painter();
        let trace = painter.with_clip_rect(chart.intersect(ui.clip_rect()));
        trace.rect_filled(chart, 2.0, ui.visuals().extreme_bg_color);
        for tick in plot.ticks() {
            let x = plot.x(chart, tick);
            trace.line_segment(
                [pos2(x, chart.top()), pos2(x, chart.bottom())],
                Stroke::new(1.0_f32, GRID),
            );
        }
        for percent in [0, 25, 50, 75, 100] {
            let y = normalized_y(chart, f64::from(percent) / 100.0);
            trace.line_segment(
                [pos2(chart.left(), y), pos2(chart.right(), y)],
                Stroke::new(1.0_f32, GRID),
            );
            painter.text(
                pos2(chart.left() - 5.0, y),
                Align2::RIGHT_CENTER,
                format!("{percent}%"),
                FontId::monospace(9.0),
                ui.visuals().weak_text_color(),
            );
        }

        let history = &app.history;
        for watch in watches() {
            draw_series(
                &trace,
                history,
                &plot,
                chart,
                COLORS[watch.color % COLORS.len()],
                None,
                |f| watch.value(f).map(|value| value as f64),
            );
        }
        paint_cursors(&trace, &plot, chart, history);
        let hit = hit_cursor(&response, history, &plot, chart).or(axis_hit);
        if self.hover != hover {
            self.hover = hover;
            ui.ctx().request_repaint();
        }
        if let Some((id, end)) = hit {
            app.history.select(id);
            self.past_end = Some((id, end));
        }
    }

    pub fn events(&mut self, app: &mut Advanced, ui: &mut egui::Ui) {
        ui.set_min_height(ui.available_height());
        let mut hit = None;
        ui.heading("事件");
        egui::ScrollArea::vertical()
            .id_salt("trace_events")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.spacing_mut().interact_size.y = 20.0;
                let mut rows: Vec<_> = app
                    .logs
                    .iter()
                    .map(|entry| (entry.time, entry.text.as_str(), entry.error, None))
                    .chain(app.history.markers.iter().rev().take(100).map(|marker| {
                        (
                            app.session_started + marker.time,
                            marker.text.as_str(),
                            marker.alarm,
                            Some(marker),
                        )
                    }))
                    .collect();
                rows.sort_by_key(|row| row.0);
                if rows.is_empty() {
                    ui.weak("暂无事件");
                }
                for (time, message, error, marker) in rows.into_iter().rev() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            vec2(82.0, 18.0),
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{time:.3} s",
                                    time = time.as_secs_f64()
                                ))
                                .monospace()
                                .weak(),
                            ),
                        );
                        let (level, color) = if error {
                            ("故障", FAULT)
                        } else if marker.is_some() {
                            ("状态", WARN)
                        } else {
                            ("信息", ui.visuals().weak_text_color())
                        };
                        ui.add_sized(
                            vec2(36.0, 18.0),
                            egui::Label::new(egui::RichText::new(level).color(color)),
                        );
                        if let Some(marker) = marker {
                            let selected = app
                                .history
                                .cursor
                                .as_ref()
                                .is_some_and(|cursor| cursor.id == marker.id);
                            let mut text =
                                egui::RichText::new(format!("#{}  {message}", marker.id))
                                    .color(color);
                            if selected {
                                text = text.background_color(ui.visuals().selection.bg_fill);
                            }
                            if ui
                                .add(
                                    egui::Label::new(text)
                                        .wrap()
                                        .selectable(false)
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                hit = Some((marker.id, marker.time.as_secs_f64()));
                            }
                        } else {
                            let text = egui::RichText::new(message);
                            ui.add(
                                egui::Label::new(if error { text.color(color) } else { text })
                                    .wrap(),
                            );
                        }
                    });
                }
            });
        if let Some((id, time)) = hit {
            app.history.select(id);
            if let Some(last) = app.history.frames.back() {
                self.past_end = Some((id, (time + 1.5).min(last.time.as_secs_f64())));
            }
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

fn normalized_y(chart: Rect, value: f64) -> f32 {
    chart.bottom() - 3.0 - value as f32 * (chart.height() - 6.0).max(0.0)
}

fn draw_series(
    trace: &egui::Painter,
    history: &History,
    plot: &Plot,
    chart: Rect,
    color: Color32,
    baseline: Option<(f64, f64)>,
    value: impl Fn(&Recorded) -> Option<f64>,
) {
    let range = plot.range;
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
    let y = |v: f64| normalized_y(chart, (v - low) / span);
    let stroke = Stroke::new(1.8_f32, color);
    // Keep each pixel column's extrema and join successive available samples.
    struct PixelColumn {
        x: f32,
        first: f64,
        min: f64,
        max: f64,
        last: f64,
    }
    let mut bucket = None;
    let mut prev = None;
    let flush = |bucket: &mut Option<PixelColumn>, prev: &mut Option<egui::Pos2>| {
        if let Some(column) = bucket.take() {
            let x = column.x;
            let point = pos2(x, y(column.last));
            if column.min != column.max {
                trace.line_segment([pos2(x, y(column.min)), pos2(x, y(column.max))], stroke);
            }
            if let Some(before) = prev.replace(point) {
                trace.line_segment([before, pos2(x, y(column.first))], stroke);
            } else if column.min == column.max {
                trace.circle_filled(point, 1.0, color);
            }
        }
    };
    let mut last_id = None;
    for frame in visible() {
        if last_id.is_some_and(|id| frame.id != id + 1) {
            flush(&mut bucket, &mut prev);
        }
        last_id = Some(frame.id);
        let Some(v) = value(frame) else {
            flush(&mut bucket, &mut prev);
            continue;
        };
        let x = xpos(frame.time.as_secs_f64()).floor();
        if let Some(column) = &mut bucket {
            if column.x == x {
                column.min = column.min.min(v);
                column.max = column.max.max(v);
                column.last = v;
                continue;
            }
        }
        flush(&mut bucket, &mut prev);
        bucket = Some(PixelColumn {
            x,
            first: v,
            min: v,
            max: v,
            last: v,
        });
    }
    flush(&mut bucket, &mut prev);
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
        app.history.layout(std::slice::from_ref(&hpm));
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
                    egui::TopBottomPanel::bottom("test_events")
                        .default_height(120.0)
                        .show(ctx, |ui| timeline.events(app, ui));
                    egui::CentralPanel::default().show(ctx, |ui| {
                        timeline.show(app, ui);
                    });
                },
            )
        };
        let frame = render(&mut app, &mut timeline, vec![]);
        let pdo = app
            .watches
            .iter()
            .find(|watch| watch.key.source == Source::Pdo)
            .unwrap();
        let color = COLORS[pdo.color % COLORS.len()];
        assert!(
            frame.shapes.iter().any(|item| {
                matches!(&item.shape, egui::Shape::LineSegment { points, stroke }
                if stroke.color == color && stroke.width >= 1.8 && points[1].x - points[0].x > 10.0)
            }),
            "valid samples on either side of an invalid cycle should be linked"
        );
        for label in ["00  6000:00", "00 GPIO", "00 PDI 相位 (µs)"] {
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
