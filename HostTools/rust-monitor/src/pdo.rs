use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Stroke};

use ethercat_monitor::backend::PdoSample;

const WINDOW: Duration = Duration::from_secs(10);
const MIN_TIME_SPAN: f64 = 0.001;
const GPIO_COLORS: [Color32; 8] = [
    Color32::from_rgb(127, 183, 255),
    Color32::from_rgb(255, 178, 101),
    Color32::from_rgb(174, 225, 119),
    Color32::from_rgb(236, 157, 204),
    Color32::from_rgb(112, 224, 217),
    Color32::from_rgb(229, 217, 116),
    Color32::from_rgb(190, 160, 252),
    Color32::from_rgb(255, 136, 130),
];

#[derive(Clone, Copy)]
struct Axis {
    start: f64,
    span: f64,
}

impl Axis {
    fn at(self, fraction: f64) -> f64 {
        self.start + self.span * fraction
    }

    fn fraction(self, value: f64) -> f32 {
        ((value - self.start) / self.span) as f32
    }

    fn zoom(self, anchor: f64, factor: f64, min_span: f64, max_span: f64) -> Self {
        let span = (self.span * factor).clamp(min_span, max_span);
        Self {
            start: self.at(anchor) - span * anchor,
            span,
        }
    }
}

#[derive(Default)]
struct Viewport {
    time: Option<Axis>,
    phase_y: Option<Axis>,
    gpio_y: Option<Axis>,
}

impl Viewport {
    fn time_axis(&self, latest: f64, oldest: f64) -> Axis {
        let span = WINDOW.as_secs_f64();
        self.time.map_or(
            Axis {
                start: latest - span,
                span,
            },
            |mut axis| {
                axis.start = axis
                    .start
                    .clamp(oldest.min(latest - axis.span), latest - axis.span);
                axis
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HpmInput {
    gpio: u16,
    phase_us: Option<u16>,
}

impl HpmInput {
    fn decode(raw: u32) -> Self {
        let phase = (raw >> 16) as u16;
        Self {
            gpio: raw as u16,
            phase_us: (phase != 0xffff).then_some(phase),
        }
    }
}

pub struct PdoView {
    history: VecDeque<(f64, Option<HpmInput>)>,
    latest: Option<PdoSample>,
    origin: Option<Instant>,
    gpio_bits: u16,
    viewport: Viewport,
}

impl Default for PdoView {
    fn default() -> Self {
        Self {
            history: VecDeque::new(),
            latest: None,
            origin: None,
            gpio_bits: 1,
            viewport: Viewport::default(),
        }
    }
}

impl PdoView {
    pub fn push(&mut self, captured: Instant, sample: PdoSample) {
        let moment = captured
            .duration_since(*self.origin.get_or_insert(captured))
            .as_secs_f64();
        self.latest = Some(sample);
        self.history
            .push_back((moment, sample.raw.map(HpmInput::decode)));
        while self.history.len() > 100_001
            || self
                .history
                .front()
                .is_some_and(|(old, _)| moment - old > WINDOW.as_secs_f64())
        {
            self.history.pop_front();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, active: bool) {
        let Some(sample) = self.latest else {
            ui.weak(if active {
                "等待 PDO 数据…"
            } else {
                "启动采集后显示 PDO 数据"
            });
            return;
        };
        let decoded = sample.raw.map(HpmInput::decode);
        if !active {
            ui.weak("采集已停止，以下为最后一次记录");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(match sample.expected_wkc {
                Some(expected) => format!("WKC {} / {expected}", sample.wkc),
                None => format!("WKC {}", sample.wkc),
            });
            ui.label(match sample.raw {
                Some(raw) => format!("原始值 0x{raw:08X}"),
                None => "输入无效".to_owned(),
            });
            if let Some(input) = decoded {
                ui.label(format!("GPIO 0x{:04X}", input.gpio));
                ui.label(input.phase_us.map_or_else(
                    || "PDI 相位无有效样本".into(),
                    |value| format!("PDI 相位 {value} µs"),
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            let adjusted = self.viewport.time.is_some()
                || self.viewport.phase_y.is_some()
                || self.viewport.gpio_y.is_some();
            if ui
                .add_enabled(adjusted, egui::Button::new("恢复跟随"))
                .clicked()
            {
                self.viewport = Viewport::default();
            }
            ui.weak("滚轮缩放时间轴 · Ctrl+滚轮缩放 Y 轴 · 拖动平移 · 双击恢复");
        });

        let (Some(&(oldest, _)), Some(&(latest, _))) = (self.history.front(), self.history.back())
        else {
            return;
        };
        let phase = Chart::allocate(ui, 194.0);
        let bits: Vec<_> = (0..16)
            .filter(|bit| self.gpio_bits & (1 << bit) != 0)
            .collect();
        let gpio = Chart::allocate(ui, (bits.len().max(5) as f32) * 24.0 + 43.0);
        let gpio_auto = Axis {
            start: 0.0,
            span: bits.len().max(1) as f64,
        };
        let time = self.viewport.time_axis(latest, oldest);
        if phase.response.double_clicked() || gpio.response.double_clicked() {
            self.viewport = Viewport::default();
        } else {
            phase.navigate(
                &mut self.viewport.time,
                time,
                &mut self.viewport.phase_y,
                || Self::phase_axis(&self.history, time),
            );
            gpio.navigate(
                &mut self.viewport.time,
                time,
                &mut self.viewport.gpio_y,
                || gpio_auto,
            );
        }

        let time = self.viewport.time_axis(latest, oldest);
        let phase_y = self
            .viewport
            .phase_y
            .unwrap_or_else(|| Self::phase_axis(&self.history, time));
        let gpio_y = self.viewport.gpio_y.unwrap_or(gpio_auto);
        let hover_time = [&phase, &gpio]
            .iter()
            .find_map(|chart| chart.hover_time(time));

        phase.axes(ui, "PDI 相位 (µs)", time, Some(phase_y), hover_time);
        self.trace(
            ui,
            &phase,
            (time, phase_y),
            Color32::from_rgb(99, 215, 193),
            false,
            |input| input.phase_us.map(f64::from),
        );

        gpio.axes(ui, "GPIO", time, None, hover_time);
        for (lane, &bit) in bits.iter().enumerate() {
            let base = (bits.len() - lane - 1) as f64;
            let color = GPIO_COLORS[bit % GPIO_COLORS.len()];
            let center = gpio.y(gpio_y, base + 0.5);
            if (gpio.rect.top()..=gpio.rect.bottom()).contains(&center) {
                ui.painter().text(
                    pos2(gpio.rect.left() - 7.0, center),
                    Align2::RIGHT_CENTER,
                    format!("GPIO {bit}"),
                    FontId::proportional(11.0),
                    color,
                );
            }
            self.trace(ui, &gpio, (time, gpio_y), color, true, |input| {
                Some(base + 0.22 + 0.56 * f64::from((input.gpio >> bit) & 1))
            });
        }
        if bits.is_empty() {
            ui.painter().text(
                gpio.rect.center(),
                Align2::CENTER_CENTER,
                "选择 GPIO 通道",
                FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
        }
    }

    pub fn variables(&mut self, ui: &mut egui::Ui) {
        let decoded = self
            .latest
            .and_then(|sample| sample.raw)
            .map(HpmInput::decode);
        ui.horizontal(|ui| {
            ui.label("PDI 相位");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.monospace(
                    decoded
                        .and_then(|input| input.phase_us)
                        .map_or("—".into(), |value| format!("{value} µs")),
                );
            });
        });
        ui.add_space(4.0);
        ui.weak("点选 GPIO 通道加入波形");
        egui::Grid::new("gpio_watch").num_columns(4).show(ui, |ui| {
            for bit in 0..16 {
                let mask = 1 << bit;
                let selected = self.gpio_bits & mask != 0;
                let value =
                    decoded.map_or("—", |input| if input.gpio & mask != 0 { "1" } else { "0" });
                let mut label = RichText::new(format!("{bit}·{value}")).monospace();
                if selected {
                    label = label.color(GPIO_COLORS[bit % GPIO_COLORS.len()]);
                }
                if ui.selectable_label(selected, label).clicked() {
                    self.gpio_bits ^= mask;
                    self.viewport.gpio_y = None;
                }
                if bit % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    }

    fn phase_axis(history: &VecDeque<(f64, Option<HpmInput>)>, time: Axis) -> Axis {
        let start = history.partition_point(|(moment, _)| *moment < time.start);
        let max = history
            .iter()
            .skip(start)
            .take_while(|(moment, _)| *moment <= time.at(1.0))
            .filter_map(|(_, input)| input.and_then(|input| input.phase_us))
            .max()
            .unwrap_or(0);
        Axis {
            start: 0.0,
            span: f64::from(max.max(10)) * 1.1,
        }
    }

    fn trace(
        &self,
        ui: &egui::Ui,
        chart: &Chart,
        (x, y): (Axis, Axis),
        color: Color32,
        step: bool,
        value: impl Fn(HpmInput) -> Option<f64>,
    ) {
        if !ui.is_rect_visible(chart.rect) {
            return;
        }
        let painter = ui
            .painter()
            .with_clip_rect(chart.rect.intersect(ui.clip_rect()));
        let mut trace = PlotTrace::new(&painter, color, step);
        let start = self
            .history
            .partition_point(|(moment, _)| *moment < x.start);
        let mut previous = None;
        for &(moment, input) in self.history.iter().skip(start.saturating_sub(1)) {
            if let Some(value) = input.and_then(&value) {
                if previous.is_some_and(|time| moment - time >= 0.1) {
                    trace.finish();
                }
                trace.push(pos2(
                    chart.rect.left() + chart.rect.width() * x.fraction(moment),
                    chart.y(y, value),
                ));
                previous = Some(moment);
            } else {
                trace.finish();
                previous = None;
            }
            if moment > x.at(1.0) {
                break;
            }
        }
        trace.finish();
    }
}

struct Chart {
    area: Rect,
    rect: Rect,
    response: egui::Response,
}

impl Chart {
    fn allocate(ui: &mut egui::Ui, height: f32) -> Self {
        let (id, area) = ui.allocate_space(vec2(ui.available_width(), height));
        let rect = Rect::from_min_max(
            area.min + vec2(57.0_f32.min(area.width() * 0.5), 23.0),
            area.max - vec2(12.0_f32.min(area.width() * 0.25), 20.0),
        );
        let response = ui.interact(rect, id, egui::Sense::click_and_drag());
        Self {
            area,
            rect,
            response,
        }
    }

    fn y(&self, axis: Axis, value: f64) -> f32 {
        self.rect.bottom() - self.rect.height() * axis.fraction(value)
    }

    fn hover_time(&self, time: Axis) -> Option<f64> {
        self.response
            .hover_pos()
            .filter(|pos| self.rect.contains(*pos) && self.rect.width() > 0.0)
            .map(|pos| time.at(f64::from((pos.x - self.rect.left()) / self.rect.width())))
    }

    fn navigate(
        &self,
        x: &mut Option<Axis>,
        time: Axis,
        y: &mut Option<Axis>,
        auto_y: impl Fn() -> Axis,
    ) {
        if self.rect.width() <= 0.0 {
            return;
        }
        if let Some(pos) = self.response.hover_pos() {
            let (wheel, ctrl) = self.response.ctx.input_mut(|input| {
                let wheel = input.raw_scroll_delta.y;
                input.smooth_scroll_delta.y = 0.0;
                (wheel, input.modifiers.ctrl)
            });
            if wheel != 0.0 {
                let factor = f64::from((-wheel / 130.0).clamp(-2.0, 2.0)).exp();
                if ctrl {
                    let anchor = f64::from((self.rect.bottom() - pos.y) / self.rect.height());
                    *y = Some(
                        y.unwrap_or_else(&auto_y)
                            .zoom(anchor, factor, 0.05, 131_072.0),
                    );
                } else {
                    let anchor = f64::from((pos.x - self.rect.left()) / self.rect.width());
                    let zoomed = time.zoom(anchor, factor, MIN_TIME_SPAN, WINDOW.as_secs_f64());
                    if x.is_some() || zoomed.span != time.span {
                        *x = Some(zoomed);
                    }
                }
            }
        }
        let drag = self.response.drag_delta();
        if drag.x != 0.0 {
            let mut axis = x.unwrap_or(time);
            axis.start = time.start - time.span * f64::from(drag.x / self.rect.width());
            *x = Some(axis);
        }
        if drag.y != 0.0 {
            let mut axis = y.unwrap_or_else(auto_y);
            axis.start += axis.span * f64::from(drag.y / self.rect.height());
            *y = Some(axis);
        }
    }

    fn axes(
        &self,
        ui: &egui::Ui,
        label: &str,
        time: Axis,
        values_for_label: Option<Axis>,
        hover: Option<f64>,
    ) {
        let rect = self.rect;
        let painter = ui.painter();
        let font = FontId::proportional(12.0);
        let axis = ui.visuals().weak_text_color();
        let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        painter.text(
            self.area.left_top(),
            Align2::LEFT_TOP,
            label,
            font.clone(),
            ui.visuals().text_color(),
        );

        for ratio in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let x = rect.left() + ratio * rect.width();
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, grid),
            );
            painter.text(
                pos2(x, rect.bottom() + 3.0),
                if ratio == 0.0 {
                    Align2::LEFT_TOP
                } else if ratio == 1.0 {
                    Align2::RIGHT_TOP
                } else {
                    Align2::CENTER_TOP
                },
                format!("{:.3} s", time.at(f64::from(ratio))),
                font.clone(),
                axis,
            );
        }
        for ratio in [0.0, 0.5, 1.0] {
            let y = rect.bottom() - ratio * rect.height();
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(1.0_f32, grid),
            );
            if let Some(values) = values_for_label {
                painter.text(
                    pos2(rect.left() - 7.0, y),
                    Align2::RIGHT_CENTER,
                    format!("{:.2}", values.at(f64::from(ratio))),
                    font.clone(),
                    axis,
                );
            }
        }
        if let Some(moment) = hover {
            let x = rect.left() + rect.width() * time.fraction(moment);
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, axis),
            );
        }
    }
}

struct PlotTrace<'a> {
    painter: &'a egui::Painter,
    stroke: Stroke,
    step: bool,
    column: Option<[egui::Pos2; 4]>,
    line: Vec<egui::Pos2>,
}

impl<'a> PlotTrace<'a> {
    fn new(painter: &'a egui::Painter, color: Color32, step: bool) -> Self {
        Self {
            painter,
            stroke: Stroke::new(1.4_f32, color),
            step,
            column: None,
            line: Vec::new(),
        }
    }

    fn push(&mut self, point: egui::Pos2) {
        if let Some([first, top, bottom, last]) = &mut self.column {
            if first.x.floor() == point.x.floor() {
                if point.y < top.y {
                    *top = point;
                }
                if point.y > bottom.y {
                    *bottom = point;
                }
                *last = point;
                return;
            }
        }
        self.flush_column();
        self.column = Some([point; 4]);
    }

    fn flush_column(&mut self) {
        let Some(mut points) = self.column.take() else {
            return;
        };
        // Keep the first, last and both extrema for each visible pixel column.
        points.sort_by(|left, right| left.x.total_cmp(&right.x));
        for point in points {
            if let Some(last) = self.line.last() {
                if *last == point {
                    continue;
                }
                if self.step && last.y != point.y {
                    self.line.push(pos2(point.x, last.y));
                }
            }
            self.line.push(point);
        }
    }

    fn finish(&mut self) {
        self.flush_column();
        match self.line.as_slice() {
            [] => {}
            [point] => {
                self.painter.circle_filled(*point, 1.5, self.stroke.color);
                self.line.clear();
            }
            _ => {
                self.painter.add(egui::Shape::line(
                    std::mem::take(&mut self.line),
                    self.stroke,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_zooms_only_the_requested_axis_and_history_stays_anchored() {
        let ctx = egui::Context::default();
        let time = Axis {
            start: 10.0,
            span: 10.0,
        };
        let auto_y = Axis {
            start: 0.0,
            span: 10.0,
        };
        let mut viewport = Viewport::default();
        let mut center = egui::Pos2::ZERO;
        let mut frame = |events, modifiers| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    modifiers,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let chart = Chart::allocate(ui, 128.0);
                        center = chart.rect.center();
                        chart.navigate(&mut viewport.time, time, &mut viewport.phase_y, || auto_y);
                    });
                },
            );
            (center, viewport.time, viewport.phase_y)
        };
        let (center, _, _) = frame(vec![], egui::Modifiers::NONE);
        let wheel = |modifiers| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 1.0),
            modifiers,
        };
        let (_, x, y) = frame(
            vec![
                egui::Event::PointerMoved(center),
                wheel(egui::Modifiers::NONE),
            ],
            egui::Modifiers::NONE,
        );
        assert!(y.is_none());
        let x = x.unwrap();
        assert!(x.span < time.span);
        assert!((x.at(0.5) - time.at(0.5)).abs() < 1e-5);

        let (_, new_x, y) = frame(vec![wheel(egui::Modifiers::CTRL)], egui::Modifiers::CTRL);
        assert_eq!(new_x.unwrap().span, x.span);
        assert!(y.unwrap().span < auto_y.span);
        assert_eq!(viewport.time_axis(21.0, 11.0).start, x.start);
        assert_eq!(viewport.time_axis(30.0, 20.0).start, 20.0);
    }

    #[test]
    fn decodes_gpio_and_phase_independently() {
        assert_eq!(
            HpmInput::decode(u32::from_le_bytes([0xa5, 0x80, 0x34, 0x12])),
            HpmInput {
                gpio: 0x80a5,
                phase_us: Some(0x1234)
            }
        );
        assert_eq!(
            HpmInput::decode(0xffff_5555),
            HpmInput {
                gpio: 0x5555,
                phase_us: None
            }
        );
        assert_eq!(
            HpmInput::decode(0),
            HpmInput {
                gpio: 0,
                phase_us: Some(0)
            }
        );
    }
    #[test]
    fn chart_renders_valid_and_missing_samples() {
        let now = Instant::now();
        let mut view = PdoView::default();
        for (offset, raw) in [
            Some(0x00ab_0001),
            None,
            Some(0xffff_0000),
            Some(0x00ac_0001),
        ]
        .into_iter()
        .enumerate()
        {
            view.push(
                now + Duration::from_millis(offset as u64 * 5),
                PdoSample {
                    raw,
                    wkc: if raw.is_some() { 3 } else { 0 },
                    expected_wkc: Some(3),
                },
            );
        }
        assert!(view.history[1].1.is_none());
        assert_eq!(view.history[2].1.unwrap().phase_us, None);
        assert_eq!(view.history[2].1.unwrap().gpio, 0);
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 620.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| view.show(ui, true))
                    .inner
            },
        );
        assert!(output.shapes.len() > 20);
    }
}
