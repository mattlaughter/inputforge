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

            let mut hw_dpi = self.naga_dpi.unwrap_or(1600);
            let mut dpi_changed = false;

            ui.horizontal(|ui| {
                ui.label("DPI:");
                dpi_changed = ui
                    .add(
                        egui::Slider::new(&mut hw_dpi, 100..=30000)
                            .step_by(50.0)
                            .suffix(" DPI"),
                    )
                    .changed();

                if ui.button("800").clicked() {
                    hw_dpi = 800;
                    dpi_changed = true;
                }
                if ui.button("1600").clicked() {
                    hw_dpi = 1600;
                    dpi_changed = true;
                }
                if ui.button("3200").clicked() {
                    hw_dpi = 3200;
                    dpi_changed = true;
                }
            });

            if dpi_changed {
                self.naga_dpi = Some(hw_dpi);
                match razer::set_naga_v2_hyperspeed_dpi(hw_dpi) {
                    Ok(()) => {
                        self.rb_status = format!("DPI set to {hw_dpi}");
                    }
                    Err(e) => {
                        self.rb_status = format!("DPI error: {e}");
                    }
                }
            }

            ui.add_space(4.0);
            ui.label(
                RichText::new("Stored on the device; survives power-off.")
                    .color(c.muted)
                    .small(),
            );
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
}
