use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Stroke};

use ethercat_monitor::backend::PdoSample;

const WINDOW: Duration = Duration::from_secs(10);

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

#[derive(Default)]
pub struct PdoView {
    history: VecDeque<(Instant, Option<HpmInput>)>,
    latest: Option<PdoSample>,
    gpio_bit: u8,
}

impl PdoView {
    pub fn push(&mut self, captured: Instant, sample: PdoSample) {
        self.latest = Some(sample);
        self.history
            .push_back((captured, sample.raw.map(HpmInput::decode)));
        while self.history.len() > 100_001
            || self
                .history
                .front()
                .is_some_and(|(old, _)| captured.duration_since(*old) > WINDOW)
        {
            self.history.pop_front();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, active: bool) {
        ui.heading("HPM5E31-KIT · 输入 PDO 0x6000:00");
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
            ui.label("GPIO 位");
            for bit in 0..16 {
                let label = match decoded {
                    Some(input) => format!("{bit}:{}", (input.gpio >> bit) & 1),
                    None => format!("{bit}:—"),
                };
                if ui.selectable_label(bit == self.gpio_bit, label).clicked() {
                    self.gpio_bit = bit;
                }
            }
        });

        let phase_max = self
            .history
            .iter()
            .filter_map(|(_, input)| input.and_then(|input| input.phase_us))
            .max()
            .unwrap_or(0);
        let phase_max = f32::from(phase_max).max(10.0) * 1.1;
        self.plot(
            ui,
            "PDI 相位 (µs)",
            phase_max,
            None,
            Color32::from_rgb(99, 215, 193),
        );
        self.plot(
            ui,
            &format!("GPIO {}", self.gpio_bit),
            1.0,
            Some(self.gpio_bit),
            Color32::from_rgb(127, 183, 255),
        );
    }

    fn plot(&self, ui: &mut egui::Ui, label: &str, max: f32, bit: Option<u8>, color: Color32) {
        let (area, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 128.0), egui::Sense::hover());
        let rect = Rect::from_min_max(area.min + vec2(57.0, 23.0), area.max - vec2(12.0, 20.0));
        let painter = ui.painter();
        let font = FontId::proportional(12.0);
        let axis = ui.visuals().weak_text_color();
        let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
        painter.text(
            area.left_top(),
            Align2::LEFT_TOP,
            label,
            font.clone(),
            color,
        );

        for ratio in [0.0, 0.5, 1.0] {
            let x = rect.left() + ratio * rect.width();
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, grid),
            );
            painter.text(
                pos2(x, rect.bottom() + 3.0),
                Align2::CENTER_TOP,
                format!("{:.0} s", (ratio - 1.0) * WINDOW.as_secs_f32()),
                font.clone(),
                axis,
            );
        }
        for ratio in [0.0, 1.0] {
            let y = rect.bottom() - ratio * rect.height();
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(1.0_f32, grid),
            );
            painter.text(
                pos2(rect.left() - 7.0, y),
                Align2::RIGHT_CENTER,
                format!("{:.0}", ratio * max),
                font.clone(),
                axis,
            );
        }

        let Some((latest, _)) = self.history.back() else {
            return;
        };
        let painter = painter.with_clip_rect(rect.expand(1.0));
        let mut trace = PlotTrace::new(&painter, color, bit.is_some());
        let mut previous = None;
        for (timestamp, input) in &self.history {
            let value = input.and_then(|input| match bit {
                Some(bit) => Some((input.gpio >> bit) & 1),
                None => input.phase_us,
            });
            let Some(value) = value else {
                trace.finish();
                previous = None;
                continue;
            };
            if previous
                .is_some_and(|time| timestamp.duration_since(time) >= Duration::from_millis(100))
            {
                trace.finish();
            }
            let age = latest.duration_since(*timestamp).as_secs_f32();
            let point = pos2(
                rect.right() - rect.width() * age / WINDOW.as_secs_f32(),
                rect.bottom() - rect.height() * f32::from(value) / max,
            );
            trace.push(point);
            previous = Some(*timestamp);
        }
        trace.finish();
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
