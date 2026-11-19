use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke};
use ethercat_monitor::backend::{Backend, Slave};

use crate::{al_state, Monitor, FAULT, GOOD, WARN};

const CANVAS: Color32 = Color32::from_rgb(13, 20, 29);
const SURFACE: Color32 = Color32::from_rgb(21, 31, 43);
const BORDER: Color32 = Color32::from_rgb(48, 65, 81);
const TEXT: Color32 = Color32::from_rgb(229, 238, 247);
const SELECTED: Color32 = Color32::from_rgb(23, 67, 77);

pub(crate) fn configure_theme(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = egui::vec2(9.0, 8.0);
        style.spacing.interact_size.y = 28.0;
        style.visuals.panel_fill = SURFACE;
        style.visuals.window_fill = SURFACE;
        style.visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
        style.visuals.extreme_bg_color = CANVAS;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = Stroke::new(1.0_f32, GOOD);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 48, 62);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 64, 81);
        style.visuals.widgets.active.bg_fill = SELECTED;
        for text_style in [
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Monospace,
        ] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = 14.0;
            }
        }
    });
}

impl Monitor {
    pub(super) fn workbench(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("connection")
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::symmetric(20, 12)),
            )
            .show(ctx, |ui| {
                ui.heading(RichText::new("EtherCAT 工作台").size(23.0).strong());
                ui.add_space(3.0);
                self.controls(ui);
            });

        egui::TopBottomPanel::bottom("bus_status")
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .inner_margin(Margin::symmetric(20, 6)),
            )
            .show(ctx, |ui| {
                self.overview(ui);
            });

        egui::SidePanel::left("slave_inspector")
            .default_width(310.0)
            .width_range(270.0..=440.0)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(16, 15)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector")
                    .show(ui, |ui| self.inspector(ui));
            });

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(CANVAS)
                    .inner_margin(Margin::symmetric(20, 15)),
            )
            .show(ctx, |ui| {
                ui.heading(if self.pdo_enabled {
                    "实时波形"
                } else {
                    "总线监视"
                });
                ui.add_space(6.0);
                if self.pdo_enabled {
                    egui::ScrollArea::vertical()
                        .id_salt("waveforms")
                        .show(ui, |ui| {
                            Frame::new()
                                .fill(SURFACE)
                                .stroke(Stroke::new(1.0_f32, BORDER))
                                .corner_radius(8)
                                .inner_margin(Margin::same(14))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    self.pdo_view
                                        .show(ui, self.session.is_some() && !self.stopping);
                                });
                        });
                } else {
                    self.scan_results(ui);
                }
            });
    }

    fn scan_results(&self, ui: &mut egui::Ui) {
        let Some(snapshot) = &self.snapshot else {
            ui.weak("连接主站后显示扫描结果");
            return;
        };
        if snapshot.slaves.is_empty() {
            ui.weak("当前没有从站");
            return;
        }
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("scan_results")
                .striped(true)
                .spacing(egui::vec2(16.0, 8.0))
                .show(ui, |ui| {
                    for label in [
                        "位置",
                        "从站",
                        "厂商 ID",
                        "产品代码",
                        "修订号",
                        "AL 状态",
                        "AL 状态码",
                    ] {
                        ui.strong(label);
                    }
                    ui.end_row();
                    for slave in &snapshot.slaves {
                        let (state, color) = al_state(slave.state);
                        ui.monospace(slave.position.to_string());
                        ui.label(slave_name(slave));
                        ui.monospace(format!("0x{:08X}", slave.vendor));
                        ui.monospace(format!("0x{:08X}", slave.product));
                        ui.monospace(format!("0x{:08X}", slave.revision));
                        ui.colored_label(color, state);
                        ui.monospace(
                            slave
                                .al_code
                                .map_or("—".into(), |code| format!("0x{code:04X}")),
                        );
                        ui.end_row();
                    }
                });
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("从站");
        let Some(snapshot) = &self.snapshot else {
            ui.weak("连接主站后显示从站");
            return;
        };
        if snapshot.slaves.is_empty() {
            ui.weak("当前没有从站");
            self.selected_slave = None;
            return;
        }
        if !snapshot
            .slaves
            .iter()
            .any(|slave| Some(slave.position) == self.selected_slave)
        {
            self.selected_slave = snapshot.slaves.first().map(|slave| slave.position);
        }
        for slave in &snapshot.slaves {
            ui.horizontal(|ui| {
                let (state, color) = al_state(slave.state);
                let name = format!("{:02}  {}", slave.position, slave_name(slave));
                let label = egui::Button::new(name)
                    .selected(self.selected_slave == Some(slave.position))
                    .truncate();
                if ui
                    .add_sized([ui.available_width() - 85.0, 30.0], label)
                    .on_hover_text(slave_name(slave))
                    .clicked()
                {
                    self.selected_slave = Some(slave.position);
                }
                ui.colored_label(color, state);
            });
        }

        let Some(slave) = snapshot
            .slaves
            .iter()
            .find(|slave| Some(slave.position) == self.selected_slave)
        else {
            return;
        };
        ui.add_space(8.0);
        ui.separator();
        ui.collapsing("从站信息", |ui| slave_details(ui, slave));

        let is_hpm = (slave.vendor, slave.product) == (0x0002_4121, 1)
            || (self.backend == Backend::Demo && slave.position == 0);
        if self.pdo_enabled && is_hpm {
            ui.separator();
            egui::CollapsingHeader::new("过程变量")
                .default_open(true)
                .show(ui, |ui| {
                    ui.monospace("TxPDO  0x6000:00  UINT32");
                    self.pdo_view.variables(ui);
                    ui.add_space(5.0);
                    ui.monospace("RxPDO  0x7010:00  UINT32");
                    ui.weak(if snapshot.pdo.is_some() {
                        "固定零值输出"
                    } else {
                        "等待 PDO 数据"
                    });
                });
        }
    }
}

fn slave_name(slave: &Slave) -> &str {
    if slave.name.is_empty() {
        "未命名从站"
    } else {
        &slave.name
    }
}

fn slave_details(ui: &mut egui::Ui, slave: &Slave) {
    let (state, color) = al_state(slave.state);
    egui::Grid::new("slave_metadata")
        .striped(true)
        .show(ui, |ui| {
            for (label, value) in [
                ("位置", slave.position.to_string()),
                ("厂商 ID", format!("0x{:08X}", slave.vendor)),
                ("产品代码", format!("0x{:08X}", slave.product)),
                ("修订号", format!("0x{:08X}", slave.revision)),
            ] {
                ui.weak(label);
                ui.monospace(value);
                ui.end_row();
            }
            ui.weak("AL 状态");
            ui.colored_label(color, state);
            ui.end_row();
            ui.weak("AL 状态码");
            let color = match slave.al_code {
                Some(0) => GOOD,
                Some(_) => FAULT,
                None => WARN,
            };
            ui.colored_label(
                color,
                slave
                    .al_code
                    .map_or("—".into(), |code| format!("0x{code:04X}")),
            );
            ui.end_row();
        });
}
