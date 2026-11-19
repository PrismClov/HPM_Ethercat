use eframe::egui;
use ethercat_monitor::service::{self, Adapter};
use std::{sync::mpsc, thread};

#[derive(Default)]
pub struct AdapterPicker {
    items: Vec<Adapter>,
    pending: Option<mpsc::Receiver<service::Result<Vec<Adapter>>>>,
    loaded: bool,
    manual: bool,
    error: Option<String>,
}

impl AdapterPicker {
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.pending = Some(rx);
        self.error = None;
        thread::spawn(move || {
            let _ = tx.send(service::soem_adapters());
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, target: &mut String) -> bool {
        let mut changed = false;
        if !self.loaded && self.pending.is_none() {
            self.refresh(ui.ctx());
        }
        if let Some(rx) = &self.pending {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("网卡枚举已中断。".into())),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.pending = None;
                self.loaded = true;
                match result {
                    Ok(items) => {
                        if !self.manual
                            && self.items.iter().any(|adapter| adapter.name == *target)
                            && !items.iter().any(|adapter| adapter.name == *target)
                        {
                            target.clear();
                            changed = true;
                        }
                        self.items = items;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        ui.label("网卡");
        if self.manual {
            let hint = if cfg!(windows) {
                r"\Device\NPF_{GUID}"
            } else {
                "例如 enp4s0"
            };
            changed |= ui
                .add(
                    egui::TextEdit::singleline(target)
                        .desired_width(198.0)
                        .hint_text(hint),
                )
                .changed();
        } else {
            let selected = self
                .items
                .iter()
                .find(|adapter| adapter.name == *target)
                .map(label);
            let caption = if self.pending.is_some() {
                "正在枚举…"
            } else if selected.is_some() {
                selected.unwrap()
            } else if !target.is_empty() {
                target.as_str()
            } else if self.items.is_empty() {
                "未列出网卡"
            } else {
                "选择网卡"
            };
            egui::ComboBox::from_id_salt("soem_adapter")
                .width(196.0)
                .truncate()
                .selected_text(caption)
                .show_ui(ui, |ui| {
                    for adapter in &self.items {
                        let response = ui
                            .selectable_label(*target == adapter.name, label(adapter))
                            .on_hover_text(&adapter.name);
                        if response.clicked() && *target != adapter.name {
                            target.clone_from(&adapter.name);
                            changed = true;
                        }
                    }
                });
        }
        if ui
            .add_enabled(self.pending.is_none(), egui::Button::new("刷新"))
            .clicked()
        {
            self.refresh(ui.ctx());
        }
        if ui.selectable_label(self.manual, "手动").clicked() {
            self.manual = !self.manual;
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::WARN, "网卡枚举失败")
                .on_hover_text(error);
        } else if self.loaded && self.items.is_empty() && self.pending.is_none() {
            ui.weak(if cfg!(windows) {
                "未发现 Npcap 网卡"
            } else {
                "未发现网卡"
            });
        }
        changed
    }
}

fn label(adapter: &Adapter) -> &str {
    if adapter.description.is_empty() {
        &adapter.name
    } else {
        &adapter.description
    }
}
