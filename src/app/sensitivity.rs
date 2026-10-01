//! Device sensitivity panel (pointer / scroll).

use super::*;
use crate::devices::PhysDevice;

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
        ui.label(
            RichText::new(
                "Hardware DPI and polling rate are under Settings → Advanced (needs libratbag).",
            )
            .color(c.muted)
            .small(),
        );
    }
}
