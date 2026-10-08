//! Device sensitivity panel (pointer / scroll).

use super::*;
use crate::devices::PhysDevice;
use crate::razer;

impl App {
    pub(crate) fn ui_sensitivity(&mut self, ui: &mut egui::Ui, d: &PhysDevice) {
        let c = self.colors;
        self.managed_banner(ui, d);
        // Use the first pointer node's settings as the source of truth, apply to all.
        let ptr_nodes: Vec<(String, String)> = d
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    devices::DeviceKind::Mouse | devices::DeviceKind::Combo
                )
            })
            .map(|n| (n.name.clone(), n.phys.clone()))
            .collect();
        let cur = self
            .cfg
            .devices
            .iter()
            .find(|s| ptr_nodes.iter().any(|(n, p)| *n == s.name && *p == s.phys))
            .cloned()
            .unwrap_or_default();
        let (mut sp, mut sc, mut inv) = (cur.pointer_speed, cur.scroll_speed, cur.invert_scroll);
        let mut changed = false;
        egui::Grid::new("sens")
            .num_columns(2)
            .spacing([24.0, 18.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Pointer speed").strong());
                changed |= ui
                    .add(
                        egui::Slider::new(&mut sp, 0.1..=5.0)
                            .custom_formatter(|v, _| format!("{v:.2}×")),
                    )
                    .changed();
                ui.end_row();
                ui.label(RichText::new("Scroll speed").strong());
                changed |= ui
                    .add(
                        egui::Slider::new(&mut sc, 0.25..=5.0)
                            .custom_formatter(|v, _| format!("{v:.2}×")),
                    )
                    .changed();
                ui.end_row();
                ui.label(RichText::new("Natural scrolling").strong());
                changed |= toggle(ui, &mut inv).changed();
                ui.end_row();
            });
        if ui.button("Reset").clicked() {
            (sp, sc, inv) = (1.0, 1.0, false);
            changed = true;
        }
        if changed {
            self.set_phys_enabled(d, true);
            for s in self.cfg.devices.iter_mut() {
                if ptr_nodes.iter().any(|(n, p)| *n == s.name && *p == s.phys) {
                    s.pointer_speed = sp;
                    s.scroll_speed = sc;
                    s.invert_scroll = inv;
                }
            }
        }
        ui.add_space(12.0);

        // Hardware DPI for Razer Naga V2 HyperSpeed
        if d.name.contains("Razer") && d.name.contains("Naga V2 HyperSpeed") {
            ui.separator();
            ui.add_space(8.0);
            ui.label(RichText::new("Hardware DPI").strong());
            ui.add_space(4.0);

            if self.naga_dpi.is_none() && self.naga_dpi_status.is_empty() {
                self.read_naga_dpi();
            }
            let mut hw_dpi = self.naga_dpi.unwrap_or(1600);
            let mut apply = false;

            ui.horizontal(|ui| {
                let r = ui.add_enabled(
                    self.naga_dpi.is_some(),
                    egui::Slider::new(&mut hw_dpi, razer::DPI_MIN..=razer::DPI_MAX)
                        .step_by(50.0)
                        .suffix(" DPI"),
                );
                // Each write is a USB round trip; send once when the drag ends.
                apply |= r.drag_stopped() || (r.changed() && !r.dragged());
                for v in [800, 1600, 3200] {
                    if ui
                        .add_enabled(self.naga_dpi.is_some(), egui::Button::new(v.to_string()))
                        .clicked()
                    {
                        hw_dpi = v;
                        apply = true;
                    }
                }
                if ui
                    .button("⟳")
                    .on_hover_text(
                        "Read the DPI from the mouse again (e.g. after using its DPI buttons)",
                    )
                    .clicked()
                {
                    self.read_naga_dpi();
                }
            });
            if self.naga_dpi.is_some() {
                self.naga_dpi = Some(hw_dpi);
            }

            if apply {
                match razer::set_naga_dpi(hw_dpi) {
                    Ok(now) => {
                        self.naga_dpi = Some(now);
                        self.naga_dpi_status = format!("Mouse reports {now} DPI");
                    }
                    Err(e) => self.naga_dpi_status = format!("Could not set DPI: {e:#}"),
                }
            }

            ui.add_space(4.0);
            ui.label(RichText::new(&self.naga_dpi_status).color(c.muted).small());
        } else {
            ui.label(
                RichText::new(
                    "Hardware DPI and polling rate are under Settings → Advanced (needs libratbag).",
                )
                .color(c.muted)
                .small(),
            );
        }
    }

    fn read_naga_dpi(&mut self) {
        match razer::naga_dpi() {
            Ok(v) => {
                self.naga_dpi = Some(v);
                self.naga_dpi_status = format!("Mouse reports {v} DPI");
            }
            Err(e) => {
                self.naga_dpi = None;
                self.naga_dpi_status = format!("Could not read DPI: {e:#}");
            }
        }
    }
}
