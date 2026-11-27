use crate::client::CrimpClient;
use crate::layout_store::{
    SavedLayout, ScrollMode, arr_to_pos, arr_to_vec, compute_stable_keys, load_saved_layout,
    pos_to_arr, save_layout, vec_to_arr,
};
use crate::theme;
use crimp_proto::config::{AliasRule, Config, DenyRule, NodeMatcher, RouteRule};
use crimp_proto::graph::{GraphSnapshot, NodeSnapshot, PortInfo};
use crimp_proto::ipc::{DefaultDeviceKind, IpcRequest};
use crimp_proto::telemetry::{AudioHealthSnapshot, NodeActivityState, NodeVolumeState};
use egui::epaint::{CubicBezierShape, PathStroke, Shadow};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

const HEADER_HEIGHT: f32 = 44.0;
const PORT_ROW_HEIGHT: f32 = 24.0;
const PORT_PIN_RADIUS: f32 = 5.4;
const ZOOM_MIN: f32 = 0.35;
const ZOOM_MAX: f32 = 2.0;
const MAX_UNDO_HISTORY: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragOrigin {
    Output,
    Input,
}

#[derive(Debug, Clone, Copy)]
struct PendingDragLink {
    origin: DragOrigin,
    node_id: u32,
    port_id: u32,
    is_click_mode: bool,
}

#[derive(Debug, Clone, Copy)]
enum UndoLinkOp {
    Create {
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
    },
    Destroy {
        output_port_id: u32,
        input_port_id: u32,
    },
}

#[derive(Debug, Clone)]
struct UndoEntry {
    label: String,
    prev_config: Option<Config>,
    next_config: Option<Config>,
    undo_links: Vec<UndoLinkOp>,
    redo_links: Vec<UndoLinkOp>,
}

/// Stav interaktivního audio Patchbay plátna s trvalým ukládáním layoutu a aliasů.
pub struct PatchbayState {
    positions: HashMap<u32, Pos2>,
    saved: SavedLayout,
    pan: Vec2,
    zoom: f32,
    needs_fit: bool,
    hide_internal_and_midi: bool,
    scroll_mode: ScrollMode,
    persist_topology: bool,
    stereo_link_mode: bool,
    last_wheel_was_point: bool,
    pending_zoom_step: Option<f32>,
    layout_dirty_since: Option<Instant>,
    dragging_link: Option<PendingDragLink>,
    selected_node_id: Option<u32>,
    selected_link_id: Option<u32>,
    alias_input: String,
    alias_for_node_id: Option<u32>,
    filter_query: String,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
}

impl Default for PatchbayState {
    fn default() -> Self {
        let saved = load_saved_layout();
        let has_saved_positions = !saved.node_positions.is_empty();
        let pan = arr_to_vec(saved.pan);
        let zoom = saved.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let hide_internal_and_midi = saved.hide_internal_and_midi;
        let scroll_mode = saved.scroll_mode;
        let persist_topology = saved.persist_topology;
        Self {
            positions: HashMap::new(),
            saved,
            pan,
            zoom,
            needs_fit: !has_saved_positions,
            hide_internal_and_midi,
            scroll_mode,
            persist_topology,
            stereo_link_mode: false,
            last_wheel_was_point: true,
            pending_zoom_step: None,
            layout_dirty_since: None,
            dragging_link: None,
            selected_node_id: None,
            selected_link_id: None,
            alias_input: String::new(),
            alias_for_node_id: None,
            filter_query: String::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}

impl PatchbayState {
    /// Okamžitě zapíše aktuální pozice uzlů, pan a zoom do `~/.config/crimp/ui-layout.json`.
    pub fn flush_saved_layout(&mut self, graph: Option<&GraphSnapshot>) {
        if let Some(g) = graph {
            let refs: Vec<&NodeSnapshot> = g.nodes.iter().collect();
            let stable_keys = compute_stable_keys(&refs);
            for (node_id, pos) in &self.positions {
                if let Some(key) = stable_keys.get(node_id) {
                    self.saved
                        .node_positions
                        .insert(key.clone(), pos_to_arr(*pos));
                }
            }
        }
        self.saved.pan = vec_to_arr(self.pan);
        self.saved.zoom = self.zoom;
        self.saved.hide_internal_and_midi = self.hide_internal_and_midi;
        self.saved.scroll_mode = self.scroll_mode;
        self.saved.persist_topology = self.persist_topology;
        save_layout(&self.saved);
        self.layout_dirty_since = None;
    }

    /// Znovu načte uložené pozice uzlů, zoom a pan z `~/.config/crimp/ui-layout.json`.
    pub fn reload_layout_from_disk(&mut self) {
        self.saved = load_saved_layout();
        self.positions.clear();
        self.pan = arr_to_vec(self.saved.pan);
        self.zoom = self.saved.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.hide_internal_and_midi = self.saved.hide_internal_and_midi;
        self.scroll_mode = self.saved.scroll_mode;
        self.persist_topology = self.saved.persist_topology;
        self.needs_fit = self.saved.node_positions.is_empty();
        self.layout_dirty_since = None;
    }

    fn apply_zoom_at(&mut self, canvas_rect: Rect, target_zoom: f32, anchor: Option<Pos2>) {
        let new_zoom = target_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (new_zoom - self.zoom).abs() < 0.0001 {
            return;
        }
        let anchor_screen = anchor
            .filter(|p| canvas_rect.contains(*p))
            .unwrap_or_else(|| canvas_rect.center());
        let local_ptr = anchor_screen - canvas_rect.min;
        let world_before = (local_ptr - self.pan) / self.zoom;
        self.zoom = new_zoom;
        self.pan = local_ptr - world_before * self.zoom;
        self.mark_dirty();
    }

    fn push_undo(&mut self, entry: UndoEntry) {
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(entry);
        self.redo_stack.clear();
    }

    fn perform_undo(&mut self, graph: &GraphSnapshot, client: &CrimpClient) {
        let Some(entry) = self.undo_stack.pop() else {
            return;
        };
        if let Some(prev_cfg) = &entry.prev_config {
            client.send_command(IpcRequest::SaveConfig {
                config: prev_cfg.clone(),
            });
        }
        apply_undo_link_ops(&entry.undo_links, graph, client);
        client.push_feedback(false, format!("Vráceno zpět: {}", entry.label));
        self.redo_stack.push(entry);
    }

    fn perform_redo(&mut self, graph: &GraphSnapshot, client: &CrimpClient) {
        let Some(entry) = self.redo_stack.pop() else {
            return;
        };
        if let Some(next_cfg) = &entry.next_config {
            client.send_command(IpcRequest::SaveConfig {
                config: next_cfg.clone(),
            });
        }
        apply_undo_link_ops(&entry.redo_links, graph, client);
        client.push_feedback(false, format!("Znovu provedeno: {}", entry.label));
        self.undo_stack.push(entry);
    }

    fn record_and_destroy_link(
        &mut self,
        link_id: u32,
        graph: &GraphSnapshot,
        config: Option<&Config>,
        client: &CrimpClient,
    ) {
        if let Some(link) = graph.links.iter().find(|l| l.id == link_id) {
            self.push_undo(UndoEntry {
                label: format!("Odpojení {} -> {}", link.output_label, link.input_label),
                prev_config: self.persist_topology.then(|| config.cloned()).flatten(),
                next_config: None,
                undo_links: vec![UndoLinkOp::Create {
                    output_node_id: link.output_node_id,
                    output_port_id: link.output_port_id,
                    input_node_id: link.input_node_id,
                    input_port_id: link.input_port_id,
                }],
                redo_links: vec![UndoLinkOp::Destroy {
                    output_port_id: link.output_port_id,
                    input_port_id: link.input_port_id,
                }],
            });
        }
        client.send_command(IpcRequest::DestroyLink {
            link_id,
            persist: self.persist_topology,
        });
    }

    fn record_and_create_links(
        &mut self,
        label: String,
        pairs: &[(u32, u32, u32, u32)],
        config: Option<&Config>,
        client: &CrimpClient,
    ) {
        if pairs.is_empty() {
            return;
        }
        self.push_undo(UndoEntry {
            label,
            prev_config: self.persist_topology.then(|| config.cloned()).flatten(),
            next_config: None,
            undo_links: pairs
                .iter()
                .map(|&(_, out_p, _, in_p)| UndoLinkOp::Destroy {
                    output_port_id: out_p,
                    input_port_id: in_p,
                })
                .collect(),
            redo_links: pairs
                .iter()
                .map(|&(out_n, out_p, in_n, in_p)| UndoLinkOp::Create {
                    output_node_id: out_n,
                    output_port_id: out_p,
                    input_node_id: in_n,
                    input_port_id: in_p,
                })
                .collect(),
        });
        for &(out_n, out_p, in_n, in_p) in pairs {
            client.send_command(IpcRequest::CreateLink {
                output_node_id: out_n,
                output_port_id: out_p,
                input_node_id: in_n,
                input_port_id: in_p,
                replace_input: true,
                persist: self.persist_topology,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: Option<&GraphSnapshot>,
        config: Option<&Config>,
        health: &AudioHealthSnapshot,
        volumes: &BTreeMap<u32, NodeVolumeState>,
        client: &CrimpClient,
    ) {
        let Some(graph) = graph else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Čekám na PipeWire audio graf (spusťte Crimp daemon).")
                        .color(theme::TEXT_MUTED)
                        .size(14.0),
                );
            });
            return;
        };

        // Pravidelné debounced uložení změněného rozvržení
        if let Some(since) = self.layout_dirty_since
            && since.elapsed() >= Duration::from_millis(550)
        {
            self.flush_saved_layout(Some(graph));
        }

        // Klávesové zkratky Patchbaye (Ctrl+Z Undo, Ctrl+Y Redo, Delete odpojí kabel, Esc zruší, +/- zoom, F/0 fit)
        if !ui.ctx().text_edit_focused() {
            let undo_pressed = ui.input(|i| {
                (i.modifiers.ctrl || i.modifiers.command)
                    && !i.modifiers.shift
                    && i.key_pressed(egui::Key::Z)
            });
            let redo_pressed = ui.input(|i| {
                (i.modifiers.ctrl || i.modifiers.command)
                    && (i.key_pressed(egui::Key::Y)
                        || (i.modifiers.shift && i.key_pressed(egui::Key::Z)))
            });
            if undo_pressed {
                self.perform_undo(graph, client);
            } else if redo_pressed {
                self.perform_redo(graph, client);
            }

            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if self.dragging_link.is_some() {
                    self.dragging_link = None;
                } else {
                    self.selected_link_id = None;
                    self.selected_node_id = None;
                }
            }
            if let Some(link_id) = self.selected_link_id
                && ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                })
            {
                self.record_and_destroy_link(link_id, graph, config, client);
                self.selected_link_id = None;
            }
            if ui.input(|i| {
                !i.modifiers.ctrl
                    && !i.modifiers.command
                    && (i.key_pressed(egui::Key::F) || i.key_pressed(egui::Key::Num0))
            }) {
                self.needs_fit = true;
                self.mark_dirty();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
                self.pending_zoom_step = Some(1.16);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.pending_zoom_step = Some(1.0 / 1.16);
            }
        }

        // Horní ovládací pruh patchbaye
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_query)
                    .desired_width(185.0)
                    .hint_text("Hledat uzel / aplikaci..."),
            );
            if !self.filter_query.is_empty()
                && ui
                    .small_button("×")
                    .on_hover_text("Vyčistit vyhledávání")
                    .clicked()
            {
                self.filter_query.clear();
            }
            if ui
                .button("Auto-layout")
                .on_hover_text("Vyresetuje uložené pozice a znovu srovná uzly do sloupců")
                .clicked()
            {
                self.positions.clear();
                self.saved.node_positions.clear();
                self.needs_fit = true;
                self.mark_dirty();
            }
            if ui
                .button("Vycentrovat")
                .on_hover_text("Přizpůsobí pohled tak, aby byly vidět všechny uzly (zkratka: F nebo dvojklik na plátno)")
                .clicked()
            {
                self.needs_fit = true;
                self.mark_dirty();
            }
            if ui
                .checkbox(
                    &mut self.hide_internal_and_midi,
                    "Jen Audio",
                )
                .on_hover_text(
                    "Skryje interní PipeWire drivery a prázdné MIDI mosty pro přehlednější graf",
                )
                .changed()
            {
                self.mark_dirty();
            }
            if ui
                .checkbox(&mut self.persist_topology, "Pamatovat odpojení")
                .on_hover_text(
                    "Když je zapnuto, každé rozpojení drátu uloží trvalé [[deny]] pravidlo (WirePlumber ho po restartu PC ani aplikací znovu nepřipojí) a každé zapojení uloží neexkluzivní [[route]] do rules.toml.",
                )
                .changed()
            {
                self.mark_dirty();
            }
            if ui
                .selectable_label(self.stereo_link_mode, "Stereo L+R")
                .on_hover_text(
                    "Když je zapnuto (nebo držíš Shift při tahání kabelu), jedním tahem propojíš celý stereo pár (FL->FL + FR->FR) najednou.",
                )
                .clicked()
            {
                self.stereo_link_mode = !self.stereo_link_mode;
            }
            let undo_tip = self
                .undo_stack
                .last()
                .map_or_else(|| "Nic k vrácení (Ctrl+Z)".into(), |e| format!("Zpět (Ctrl+Z): {}", e.label));
            if ui
                .add_enabled(!self.undo_stack.is_empty(), egui::Button::new("Zpět"))
                .on_hover_text(undo_tip)
                .clicked()
            {
                self.perform_undo(graph, client);
            }
            let redo_tip = self
                .redo_stack
                .last()
                .map_or_else(|| "Nic k obnovení (Ctrl+Y)".into(), |e| format!("Znovu (Ctrl+Y): {}", e.label));
            if ui
                .add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Znovu"))
                .on_hover_text(redo_tip)
                .clicked()
            {
                self.perform_redo(graph, client);
            }
            if ui
                .button(self.scroll_mode.label())
                .on_hover_text(
                    "Kliknutím přepneš chování kolečka / touchpadu:\n• Auto: 2 prsty na touchpadu posouvají do všech stran, Ctrl + 2 prsty nebo kolečko myši zoomuje\n• Posun: každý scroll posouvá (zoom přes Ctrl + scroll)\n• Zoom: vertikální scroll zoomuje (posun přes Shift + scroll)",
                )
                .clicked()
            {
                self.scroll_mode = self.scroll_mode.next();
                self.mark_dirty();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("+").min_size(egui::vec2(26.0, 22.0)))
                    .on_hover_text("Přiblížit (+ nebo Ctrl + 2 prsty)")
                    .clicked()
                {
                    self.pending_zoom_step = Some(1.16);
                }
                if ui
                    .add(
                        egui::Button::new(format!("{:.0}%", self.zoom * 100.0))
                            .min_size(egui::vec2(46.0, 22.0)),
                    )
                    .on_hover_text("Kliknutím vycentruješ a přizpůsobíš graf na okno (F / 0)")
                    .clicked()
                {
                    self.needs_fit = true;
                    self.mark_dirty();
                }
                if ui
                    .add(egui::Button::new("−").min_size(egui::vec2(26.0, 22.0)))
                    .on_hover_text("Oddálit (- nebo Ctrl + 2 prsty)")
                    .clicked()
                {
                    self.pending_zoom_step = Some(1.0 / 1.16);
                }
                theme::spec_divider(ui);
                ui.label(
                    egui::RichText::new(format!(
                        "{} uzlů  ·  {} linků",
                        graph.nodes.len(),
                        graph.links.len(),
                    ))
                    .color(theme::TEXT_MUTED)
                    .size(12.0),
                );
            });
        });
        ui.add_space(6.0);

        let inspector_open = self.selected_node_id.is_some() || self.selected_link_id.is_some();
        if inspector_open {
            egui::Panel::right("patchbay_inspector")
                .default_size(310.0)
                .min_size(260.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::BG_SECONDARY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.draw_inspector(ui, graph, config, health, volumes, client);
                    });
                });
        }

        let live_ids: HashSet<u32> = graph.nodes.iter().map(|n| n.id).collect();
        self.positions.retain(|id, _| live_ids.contains(id));

        let visible_nodes: Vec<&NodeSnapshot> = graph
            .nodes
            .iter()
            .filter(|node| self.matches_filter(node))
            .collect();

        self.ensure_layout(graph, &visible_nodes);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_OBSIDIAN)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .corner_radius(CornerRadius::same(10)),
            )
            .show(ui, |ui| {
                self.draw_canvas(ui, graph, &visible_nodes, config, health, volumes, client);
            });
    }

    fn mark_dirty(&mut self) {
        self.layout_dirty_since = Some(Instant::now());
    }

    fn matches_filter(&self, node: &NodeSnapshot) -> bool {
        if self.hide_internal_and_midi && is_internal_or_midi_node(node) {
            return false;
        }
        let q = self.filter_query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return true;
        }
        node.label.to_ascii_lowercase().contains(&q)
            || node
                .node_name
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
            || node
                .application_name
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
            || node
                .media_class
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
    }

    fn ensure_layout(&mut self, graph: &GraphSnapshot, visible_nodes: &[&NodeSnapshot]) {
        let all_refs: Vec<&NodeSnapshot> = graph.nodes.iter().collect();
        let stable_keys = compute_stable_keys(&all_refs);

        // Nejprve doplníme pozice ze SavedLayout pro nově připojené / probuzené uzly.
        for &node in visible_nodes {
            if self.positions.contains_key(&node.id) {
                continue;
            }
            if let Some(key) = stable_keys.get(&node.id)
                && let Some(&saved_xy) = self.saved.node_positions.get(key)
            {
                self.positions.insert(node.id, arr_to_pos(saved_xy));
            }
        }

        // Pro zbylé (zcela nové) uzly najdeme volné místo pod stávajícími uzly v příslušném sloupci.
        let mut col_max_y = [0.0f32; 3];
        for &node in visible_nodes {
            if let Some(pos) = self.positions.get(&node.id) {
                let col = classify_column(node);
                col_max_y[col] = col_max_y[col].max(pos.y + node_height(node) + 32.0);
            }
        }

        let mut placed_new = false;
        for &node in visible_nodes {
            if self.positions.contains_key(&node.id) {
                continue;
            }
            let col = classify_column(node);
            let x = col as f32 * 390.0;
            let y = col_max_y[col];
            self.positions.insert(node.id, Pos2::new(x, y));
            col_max_y[col] += node_height(node) + 32.0;
            placed_new = true;
        }

        if placed_new {
            self.mark_dirty();
        }
    }

    fn fit_to_canvas(&mut self, canvas_rect: Rect, nodes: &[&NodeSnapshot]) {
        if nodes.is_empty() {
            self.pan = Vec2::new(40.0, 40.0);
            self.zoom = 1.0;
            self.needs_fit = false;
            return;
        }

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for &node in nodes {
            if let Some(&pos) = self.positions.get(&node.id) {
                let w = node_width(node);
                let h = node_height(node);
                min_x = min_x.min(pos.x);
                min_y = min_y.min(pos.y);
                max_x = max_x.max(pos.x + w);
                max_y = max_y.max(pos.y + h);
            }
        }

        let content_w = (max_x - min_x).max(200.0);
        let content_h = (max_y - min_y).max(120.0);
        let avail_w = (canvas_rect.width() - 80.0).max(200.0);
        let avail_h = (canvas_rect.height() - 80.0).max(160.0);

        self.zoom = (avail_w / content_w)
            .min(avail_h / content_h)
            .clamp(ZOOM_MIN.max(0.40), 1.05);
        let scaled_w = content_w * self.zoom;
        let scaled_h = content_h * self.zoom;
        self.pan = Vec2::new(
            (canvas_rect.width() - scaled_w) * 0.5 - min_x * self.zoom,
            (canvas_rect.height() - scaled_h) * 0.5 - min_y * self.zoom,
        );
        self.needs_fit = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_canvas(
        &mut self,
        ui: &mut egui::Ui,
        graph: &GraphSnapshot,
        visible_nodes: &[&NodeSnapshot],
        config: Option<&Config>,
        health: &AudioHealthSnapshot,
        volumes: &BTreeMap<u32, NodeVolumeState>,
        client: &CrimpClient,
    ) {
        let canvas_rect = ui.available_rect_before_wrap();
        if self.needs_fit && !visible_nodes.is_empty() {
            self.fit_to_canvas(canvas_rect, visible_nodes);
        }

        let canvas_resp = ui.allocate_rect(canvas_rect, Sense::click_and_drag());

        if let Some(step) = self.pending_zoom_step.take() {
            self.apply_zoom_at(canvas_rect, self.zoom * step, None);
        }

        let latest_pos = ui.input(|i| i.pointer.latest_pos());
        let pointer_in_canvas =
            latest_pos.is_some_and(|p| canvas_rect.contains(p)) || canvas_resp.hovered();

        if pointer_in_canvas {
            ui.input(|i| {
                for ev in &i.events {
                    if let egui::Event::MouseWheel { unit, .. } = ev {
                        self.last_wheel_was_point = *unit == egui::MouseWheelUnit::Point;
                    }
                }
            });

            // Ctrl + 2 prsty na touchpadu, Ctrl + kolečko myši, případně platformní Pinch
            let pinch_zoom = ui.input(|i| i.zoom_delta());
            if (pinch_zoom - 1.0).abs() > 0.0005 {
                self.apply_zoom_at(canvas_rect, self.zoom * pinch_zoom, latest_pos);
            }

            // Touchpad 2-prstý posun (Point) vs kolečko myši (Line)
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            if scroll != Vec2::ZERO {
                let shift = ui.input(|i| i.modifiers.shift);
                let should_pan = match self.scroll_mode {
                    ScrollMode::Auto => self.last_wheel_was_point || shift,
                    ScrollMode::Pan => true,
                    ScrollMode::Zoom => shift,
                };
                if should_pan {
                    self.pan += scroll;
                    self.mark_dirty();
                } else if scroll.x.abs() > scroll.y.abs() * 1.5 {
                    self.pan.x += scroll.x;
                    self.mark_dirty();
                } else if scroll.y.abs() > 0.1 {
                    let factor = (scroll.y * 0.0022).exp().clamp(0.75, 1.35);
                    self.apply_zoom_at(canvas_rect, self.zoom * factor, latest_pos);
                }
            }
        }

        if canvas_resp.dragged_by(egui::PointerButton::Middle)
            || (canvas_resp.dragged_by(egui::PointerButton::Primary)
                && self.dragging_link.is_none())
        {
            self.pan += canvas_resp.drag_delta();
            self.mark_dirty();
        }

        if canvas_resp.double_clicked() {
            self.needs_fit = true;
            self.mark_dirty();
        } else if canvas_resp.clicked() && self.dragging_link.is_none() {
            self.selected_link_id = None;
        }

        let painter = ui.painter_at(canvas_rect);
        draw_subtle_grid(&painter, canvas_rect, self.pan, self.zoom);

        let mut port_screen_pos: HashMap<u32, Pos2> = HashMap::new();
        let mut in_pin_rects: Vec<(u32, u32, Pos2, Rect)> = Vec::new();
        let mut out_pin_rects: Vec<(u32, u32, Pos2, Rect)> = Vec::new();
        let mut mute_btn_rects: Vec<(u32, bool, Rect)> = Vec::new();
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let mut dragging_any_node = false;
        let mut hovering_any_node = false;

        // Kreslení karet uzlů
        for &node in visible_nodes {
            let world_pos = self.positions.get(&node.id).copied().unwrap_or(Pos2::ZERO);
            let w = node_width(node) * self.zoom;
            let h = node_height(node) * self.zoom;
            let top_left = canvas_rect.min + self.pan + world_pos.to_vec2() * self.zoom;
            let rect = Rect::from_min_size(top_left, Vec2::new(w, h));

            let is_selected = self.selected_node_id == Some(node.id);
            let accent = node_accent_color(node);
            let rounding = CornerRadius::same((10.0 * self.zoom).round().clamp(5.0, 14.0) as u8);

            let shadow = Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 135),
            };
            painter.add(shadow.as_shape(rect, rounding));

            let border_color = if is_selected {
                theme::ACCENT_BLUE
            } else {
                theme::BORDER_SUBTLE
            };
            painter.rect(
                rect,
                rounding,
                theme::SURFACE,
                Stroke::new(if is_selected { 1.6 } else { 1.0 }, border_color),
                egui::StrokeKind::Inside,
            );

            let header_rect =
                Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT * self.zoom));
            let header_rounding = CornerRadius {
                nw: rounding.nw,
                ne: rounding.ne,
                sw: 0,
                se: 0,
            };
            painter.rect_filled(header_rect, header_rounding, theme::SURFACE_HEADER);
            painter.line_segment(
                [header_rect.left_bottom(), header_rect.right_bottom()],
                Stroke::new(1.0, theme::BORDER_SUBTLE),
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 10.0, rect.top() + 1.0),
                    Pos2::new(rect.right() - 10.0, rect.top() + 1.0),
                ],
                Stroke::new(1.0, theme::SPECULAR_TOP),
            );

            let pill_rect = Rect::from_min_size(
                Pos2::new(
                    header_rect.left() + 10.0 * self.zoom,
                    header_rect.top() + 10.0 * self.zoom,
                ),
                Vec2::new(3.0 * self.zoom, (HEADER_HEIGHT - 20.0) * self.zoom),
            );
            painter.rect_filled(pill_rect, CornerRadius::same(2), accent);

            // Celá karta uzlu je uchopitelná pro rychlé přesouvání (výstupní piny registrované níže mají přednost).
            let card_id = ui.scope_id().with(("node_drag", node.id));
            let card_resp = ui.interact(rect, card_id, Sense::click_and_drag());
            if self.dragging_link.is_none() {
                if card_resp.drag_started() || card_resp.clicked() {
                    self.selected_node_id = Some(node.id);
                    self.selected_link_id = None;
                }
                if card_resp.dragged() {
                    dragging_any_node = true;
                    if let Some(pos) = self.positions.get_mut(&node.id) {
                        *pos += card_resp.drag_delta() / self.zoom;
                        self.mark_dirty();
                    }
                } else if card_resp.hovered() {
                    hovering_any_node = true;
                }
            }

            let title_x = header_rect.left() + 20.0 * self.zoom;
            let vol_state = volumes.get(&node.id).copied();
            let metrics = health.per_node.get(&node.id);
            let title_max_chars = if vol_state.is_some() { 20 } else { 28 };
            painter.text(
                Pos2::new(title_x, header_rect.top() + 14.0 * self.zoom),
                Align2::LEFT_CENTER,
                truncate_label(&node.label, title_max_chars),
                FontId::proportional((13.0 * self.zoom).clamp(9.0, 17.0)),
                theme::TEXT_PRIMARY,
            );

            let subtitle = node_subtitle(node);
            painter.text(
                Pos2::new(title_x, header_rect.top() + 30.0 * self.zoom),
                Align2::LEFT_CENTER,
                subtitle,
                FontId::proportional((10.5 * self.zoom).clamp(8.0, 14.0)),
                if node.is_virtual_crimp {
                    theme::ECONOMY_GREEN
                } else {
                    theme::TEXT_SUBTLE
                },
            );

            // Rychlé ztlumení + procenta hlasitosti vpravo v hlavičce karty
            if let Some(vol) = vol_state
                && self.zoom >= 0.50
            {
                let badge_w = 50.0 * self.zoom;
                let badge_h = 20.0 * self.zoom;
                let badge_rect = Rect::from_min_size(
                    Pos2::new(
                        header_rect.right() - badge_w - 8.0 * self.zoom,
                        header_rect.center().y - badge_h * 0.5,
                    ),
                    Vec2::new(badge_w, badge_h),
                );
                let hovered_mute = pointer_pos.is_some_and(|p| badge_rect.contains(p));
                let fill = if vol.muted {
                    Color32::from_rgba_unmultiplied(245, 21, 21, if hovered_mute { 75 } else { 48 })
                } else if hovered_mute {
                    theme::SURFACE_HOVER
                } else {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 10)
                };
                let stroke_col = if vol.muted {
                    theme::DANGER_RED
                } else {
                    theme::BORDER_SUBTLE
                };
                painter.rect(
                    badge_rect,
                    CornerRadius::same((5.0 * self.zoom).round().clamp(3.0, 7.0) as u8),
                    fill,
                    Stroke::new(1.0, stroke_col),
                    egui::StrokeKind::Inside,
                );
                let txt = if vol.muted {
                    "MUTE".to_string()
                } else {
                    format!("{:.0}%", vol.volume * 100.0)
                };
                painter.text(
                    badge_rect.center(),
                    Align2::CENTER_CENTER,
                    txt,
                    FontId::proportional((10.0 * self.zoom).clamp(8.0, 13.0)),
                    if vol.muted {
                        theme::DANGER_RED
                    } else {
                        theme::TEXT_PRIMARY
                    },
                );
                mute_btn_rects.push((node.id, vol.muted, badge_rect));
            }

            // Živý VU / aktivní proužek na spodní hraně hlavičky karty (pw-top state + xruns)
            if let Some(m) = metrics {
                let bar_h = (2.6 * self.zoom).clamp(1.5, 3.5);
                let bar_y = header_rect.bottom() - bar_h;
                let is_muted = vol_state.is_some_and(|v| v.muted);
                if m.err_count > 0 {
                    let full_bar = Rect::from_min_max(
                        Pos2::new(header_rect.left() + 6.0 * self.zoom, bar_y),
                        Pos2::new(header_rect.right() - 6.0 * self.zoom, header_rect.bottom()),
                    );
                    painter.rect_filled(full_bar, CornerRadius::same(1), theme::DANGER_RED);
                } else if m.state == NodeActivityState::Running && !is_muted {
                    let frac = (0.24 + m.busy_ratio * 6.5).clamp(0.24, 1.0);
                    let usable_w = (header_rect.width() - 12.0 * self.zoom).max(4.0);
                    let active_bar = Rect::from_min_max(
                        Pos2::new(header_rect.left() + 6.0 * self.zoom, bar_y),
                        Pos2::new(
                            header_rect.left() + 6.0 * self.zoom + usable_w * frac,
                            header_rect.bottom(),
                        ),
                    );
                    painter.rect_filled(active_bar, CornerRadius::same(1), theme::ECONOMY_GREEN);
                }
            }

            let in_ports = sorted_ports(&node.input_ports);
            let out_ports = sorted_ports(&node.output_ports);
            let has_both = !in_ports.is_empty() && !out_ports.is_empty();
            let max_port_chars = if has_both { 15 } else { 28 };
            let rows = in_ports.len().max(out_ports.len());
            let half_row = PORT_ROW_HEIGHT * 0.5 * self.zoom;
            let in_reach = (if has_both { 112.0 } else { 132.0 }) * self.zoom;
            let out_reach = (if has_both { 86.0 } else { 96.0 }) * self.zoom;

            for row in 0..rows {
                let row_y = header_rect.bottom() + (row as f32 + 0.5) * PORT_ROW_HEIGHT * self.zoom;

                if let Some(in_port) = in_ports.get(row) {
                    let pin_pos = Pos2::new(rect.left(), row_y);
                    port_screen_pos.insert(in_port.id, pin_pos);
                    let hit_rect = Rect::from_min_max(
                        Pos2::new(pin_pos.x - 20.0 * self.zoom, row_y - half_row),
                        Pos2::new(pin_pos.x + in_reach, row_y + half_row),
                    );
                    in_pin_rects.push((node.id, in_port.id, pin_pos, hit_rect));

                    let pin_hover = match self.dragging_link {
                        None => pointer_pos.is_some_and(|p| hit_rect.contains(p)),
                        Some(d) => {
                            d.origin == DragOrigin::Output
                                && d.node_id != node.id
                                && pointer_pos.is_some_and(|p| hit_rect.contains(p))
                        }
                    };
                    if pin_hover {
                        let glow_rect = Rect::from_min_max(
                            Pos2::new(rect.left() + 4.0 * self.zoom, row_y - half_row + 1.5),
                            Pos2::new(
                                pin_pos.x + in_reach - 6.0 * self.zoom,
                                row_y + half_row - 1.5,
                            ),
                        );
                        painter.rect_filled(
                            glow_rect,
                            CornerRadius::same(5),
                            Color32::from_rgba_unmultiplied(10, 103, 229, 44),
                        );
                    }

                    let r = (PORT_PIN_RADIUS + if pin_hover { 2.2 } else { 0.0 }) * self.zoom;
                    painter.circle_filled(pin_pos, r, theme::BG_OBSIDIAN);
                    painter.circle_stroke(
                        pin_pos,
                        r,
                        Stroke::new(if pin_hover { 2.5 } else { 1.9 }, theme::ACCENT_BLUE),
                    );
                    painter.text(
                        Pos2::new(pin_pos.x + 12.0 * self.zoom, row_y),
                        Align2::LEFT_CENTER,
                        truncate_label(&format_port_name(in_port), max_port_chars),
                        FontId::proportional((11.5 * self.zoom).clamp(8.0, 15.0)),
                        if pin_hover {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        },
                    );
                }

                if let Some(out_port) = out_ports.get(row) {
                    let pin_pos = Pos2::new(rect.right(), row_y);
                    port_screen_pos.insert(out_port.id, pin_pos);
                    let hit_rect = Rect::from_min_max(
                        Pos2::new(pin_pos.x - out_reach, row_y - half_row),
                        Pos2::new(pin_pos.x + 22.0 * self.zoom, row_y + half_row),
                    );
                    out_pin_rects.push((node.id, out_port.id, pin_pos, hit_rect));

                    let pin_hover = match self.dragging_link {
                        None => pointer_pos.is_some_and(|p| hit_rect.contains(p)),
                        Some(d) => {
                            d.origin == DragOrigin::Input
                                && d.node_id != node.id
                                && pointer_pos.is_some_and(|p| hit_rect.contains(p))
                        }
                    };
                    if pin_hover {
                        let glow_rect = Rect::from_min_max(
                            Pos2::new(
                                pin_pos.x - out_reach + 6.0 * self.zoom,
                                row_y - half_row + 1.5,
                            ),
                            Pos2::new(rect.right() - 4.0 * self.zoom, row_y + half_row - 1.5),
                        );
                        painter.rect_filled(
                            glow_rect,
                            CornerRadius::same(5),
                            Color32::from_rgba_unmultiplied(33, 222, 0, 32),
                        );
                    }

                    let r = (PORT_PIN_RADIUS + if pin_hover { 2.0 } else { 0.0 }) * self.zoom;
                    painter.circle_filled(pin_pos, r, theme::BG_OBSIDIAN);
                    painter.circle_stroke(
                        pin_pos,
                        r,
                        Stroke::new(if pin_hover { 2.4 } else { 1.9 }, theme::ECONOMY_GREEN),
                    );
                    painter.text(
                        Pos2::new(pin_pos.x - 12.0 * self.zoom, row_y),
                        Align2::RIGHT_CENTER,
                        truncate_label(&format_port_name(out_port), max_port_chars),
                        FontId::proportional((11.5 * self.zoom).clamp(8.0, 15.0)),
                        if pin_hover {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        },
                    );
                }
            }
        }

        for &(node_id, currently_muted, badge_rect) in &mute_btn_rects {
            let resp = ui.interact(
                badge_rect,
                ui.scope_id().with(("mute_btn", node_id)),
                Sense::click(),
            );
            if resp.clicked() {
                client.set_node_mute(node_id, !currently_muted);
            }
        }

        // Tahání nebo kliknutí z libovolného konce (zelený Output pin i modrý Input pin):
        // na touchpadu stačí 1× kliknout na jeden pin a pak kliknout na opačný pin bez držení prstu.
        let mut hovering_pin = false;
        let mut started_click_mode_this_frame = false;
        let completing_from_input = self
            .dragging_link
            .is_some_and(|d| d.is_click_mode && d.origin == DragOrigin::Input);
        let completing_from_output = self
            .dragging_link
            .is_some_and(|d| d.is_click_mode && d.origin == DragOrigin::Output);

        for &(out_node_id, out_port_id, _, hit_rect) in &out_pin_rects {
            let resp = ui.interact(
                hit_rect,
                ui.scope_id().with(("out_pin", out_port_id)),
                Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                hovering_pin = true;
            }
            if completing_from_input {
                continue;
            }
            if resp.drag_started() {
                self.dragging_link = Some(PendingDragLink {
                    origin: DragOrigin::Output,
                    node_id: out_node_id,
                    port_id: out_port_id,
                    is_click_mode: false,
                });
            } else if resp.clicked() {
                let already_selected = self.dragging_link.is_some_and(|d| {
                    d.is_click_mode && d.origin == DragOrigin::Output && d.port_id == out_port_id
                });
                if already_selected {
                    self.dragging_link = None;
                } else {
                    self.dragging_link = Some(PendingDragLink {
                        origin: DragOrigin::Output,
                        node_id: out_node_id,
                        port_id: out_port_id,
                        is_click_mode: true,
                    });
                    started_click_mode_this_frame = true;
                }
            }
        }

        for &(in_node_id, in_port_id, _, hit_rect) in &in_pin_rects {
            let resp = ui.interact(
                hit_rect,
                ui.scope_id().with(("in_pin", in_port_id)),
                Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                hovering_pin = true;
            }
            if completing_from_output {
                continue;
            }
            if resp.drag_started() {
                self.dragging_link = Some(PendingDragLink {
                    origin: DragOrigin::Input,
                    node_id: in_node_id,
                    port_id: in_port_id,
                    is_click_mode: false,
                });
            } else if resp.clicked() {
                let already_selected = self.dragging_link.is_some_and(|d| {
                    d.is_click_mode && d.origin == DragOrigin::Input && d.port_id == in_port_id
                });
                if already_selected {
                    self.dragging_link = None;
                } else {
                    self.dragging_link = Some(PendingDragLink {
                        origin: DragOrigin::Input,
                        node_id: in_node_id,
                        port_id: in_port_id,
                        is_click_mode: true,
                    });
                    started_click_mode_this_frame = true;
                }
            }
        }

        if let Some(drag) = self.dragging_link
            && let Some(&origin_pos) = port_screen_pos.get(&drag.port_id)
        {
            let ring_color = match drag.origin {
                DragOrigin::Output => theme::ECONOMY_GREEN,
                DragOrigin::Input => theme::ACCENT_BLUE,
            };
            painter.circle_stroke(
                origin_pos,
                (PORT_PIN_RADIUS + 4.0) * self.zoom,
                Stroke::new(2.0, ring_color),
            );
        }

        if self.dragging_link.is_some() || hovering_pin {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if dragging_any_node {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovering_any_node {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        // Vyhledání přesně jednoho nejbližšího kabelu pod kurzorem (i při více linkách do jednoho pinu)
        let hovered_link_id = if self.dragging_link.is_some() {
            None
        } else {
            pointer_pos.and_then(|ptr| {
                graph
                    .links
                    .iter()
                    .filter_map(|link| {
                        let (p0, p3) = (
                            *port_screen_pos.get(&link.output_port_id)?,
                            *port_screen_pos.get(&link.input_port_id)?,
                        );
                        let dist = bezier_min_distance(p0, p3, ptr);
                        (dist <= 12.0).then_some((link.id, dist))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(id, _)| id)
            })
        };

        let primary_clicked = ui.input(|i| i.pointer.primary_clicked());
        let secondary_clicked = ui.input(|i| i.pointer.secondary_clicked());

        if let Some(link_id) = hovered_link_id {
            if primary_clicked || secondary_clicked {
                self.selected_link_id = Some(link_id);
                self.selected_node_id = None;
            }
            if secondary_clicked {
                self.record_and_destroy_link(link_id, graph, config, client);
            } else if let Some(link) = graph.links.iter().find(|l| l.id == link_id)
                && let (Some(&p0), Some(&p3)) = (
                    port_screen_pos.get(&link.output_port_id),
                    port_screen_pos.get(&link.input_port_id),
                )
            {
                painter.circle_stroke(
                    p0,
                    (PORT_PIN_RADIUS + 3.0) * self.zoom,
                    Stroke::new(2.0, theme::ECONOMY_GREEN),
                );
                painter.circle_stroke(
                    p3,
                    (PORT_PIN_RADIUS + 3.0) * self.zoom,
                    Stroke::new(2.0, theme::ACCENT_BLUE),
                );
                egui::Tooltip::always_open(
                    ui.ctx().clone(),
                    ui.layer_id(),
                    ui.scope_id().with("link_hover_tip"),
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  ->  {}",
                            link.output_label, link.input_label
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Pravý klik nebo Delete: odpojit")
                            .size(11.0)
                            .color(theme::TEXT_MUTED),
                    );
                });
            }
        }

        for link in &graph.links {
            let (Some(&p0), Some(&p3)) = (
                port_screen_pos.get(&link.output_port_id),
                port_screen_pos.get(&link.input_port_id),
            ) else {
                continue;
            };

            let hovered = hovered_link_id == Some(link.id);
            let selected = self.selected_link_id == Some(link.id);

            let color = if selected {
                Color32::WHITE
            } else if hovered {
                theme::ECONOMY_GREEN
            } else if link.enforced_by_rule.is_some() {
                theme::ACCENT_BLUE
            } else {
                Color32::from_rgb(100, 116, 139)
            };
            let width = if selected || hovered {
                2.8
            } else if link.enforced_by_rule.is_some() {
                2.4
            } else {
                1.9
            };

            draw_cable(&painter, p0, p3, theme::BG_OBSIDIAN, width + 2.4);
            draw_cable(&painter, p0, p3, color, width);
        }

        if let Some(drag) = self.dragging_link {
            let stereo_active = self.stereo_link_mode || ui.input(|i| i.modifiers.shift);
            let target_pins = match drag.origin {
                DragOrigin::Output => &in_pin_rects,
                DragOrigin::Input => &out_pin_rects,
            };
            let snap_target = pointer_pos.and_then(|ptr| {
                target_pins
                    .iter()
                    .filter(|(target_node_id, _, _, hit)| {
                        *target_node_id != drag.node_id && hit.contains(ptr)
                    })
                    .min_by(|(_, _, p_a, _), (_, _, p_b, _)| {
                        p_a.distance_sq(ptr).total_cmp(&p_b.distance_sq(ptr))
                    })
                    .copied()
            });

            let mut drew_stereo_preview = false;
            if stereo_active && let Some((target_node_id, _, _, _)) = snap_target {
                let (src_id, dst_id) = match drag.origin {
                    DragOrigin::Output => (drag.node_id, target_node_id),
                    DragOrigin::Input => (target_node_id, drag.node_id),
                };
                if let (Some(src_n), Some(dst_n)) = (
                    graph.nodes.iter().find(|n| n.id == src_id),
                    graph.nodes.iter().find(|n| n.id == dst_id),
                ) {
                    let pairs = stereo_port_pairs(src_n, dst_n);
                    for &(_, out_p, _, in_p) in &pairs {
                        if let (Some(&p0), Some(&p3)) =
                            (port_screen_pos.get(&out_p), port_screen_pos.get(&in_p))
                        {
                            draw_cable(&painter, p0, p3, theme::BG_OBSIDIAN, 4.6);
                            draw_cable(&painter, p0, p3, theme::ACCENT_BLUE, 2.5);
                            drew_stereo_preview = true;
                        }
                    }
                }
            }

            if !drew_stereo_preview
                && let Some(&origin_pos) = port_screen_pos.get(&drag.port_id)
                && let Some(other_pos) = snap_target.map(|(_, _, p, _)| p).or(pointer_pos)
            {
                let (p0, p3, cable_color) = match drag.origin {
                    DragOrigin::Output => (origin_pos, other_pos, theme::ECONOMY_GREEN),
                    DragOrigin::Input => (other_pos, origin_pos, theme::ACCENT_BLUE),
                };
                draw_cable(&painter, p0, p3, theme::BG_OBSIDIAN, 4.6);
                draw_cable(&painter, p0, p3, cable_color, 2.5);
            }

            let mut finish_target: Option<(u32, u32)> = None;
            let mut cancel_drag = false;

            if drag.is_click_mode {
                if !started_click_mode_this_frame {
                    if primary_clicked {
                        if let Some((target_node_id, target_port_id, _, _)) = snap_target {
                            finish_target = Some((target_node_id, target_port_id));
                        }
                        cancel_drag = true;
                    } else if secondary_clicked {
                        cancel_drag = true;
                    }
                }
            } else if ui.input(|i| i.pointer.any_released()) {
                if let Some((target_node_id, target_port_id, _, _)) = snap_target {
                    finish_target = Some((target_node_id, target_port_id));
                }
                cancel_drag = true;
            }

            if let Some((target_node_id, target_port_id)) = finish_target {
                let (output_node_id, output_port_id, input_node_id, input_port_id) =
                    match drag.origin {
                        DragOrigin::Output => {
                            (drag.node_id, drag.port_id, target_node_id, target_port_id)
                        }
                        DragOrigin::Input => {
                            (target_node_id, target_port_id, drag.node_id, drag.port_id)
                        }
                    };
                if stereo_active
                    && let (Some(src_n), Some(dst_n)) = (
                        graph.nodes.iter().find(|n| n.id == output_node_id),
                        graph.nodes.iter().find(|n| n.id == input_node_id),
                    )
                {
                    let pairs = stereo_port_pairs(src_n, dst_n);
                    if !pairs.is_empty() {
                        self.record_and_create_links(
                            format!("Stereo {} -> {}", src_n.label, dst_n.label),
                            &pairs,
                            config,
                            client,
                        );
                    }
                } else {
                    self.record_and_create_links(
                        format!("Propojení #{output_node_id} -> #{input_node_id}"),
                        &[(output_node_id, output_port_id, input_node_id, input_port_id)],
                        config,
                        client,
                    );
                }
            }
            if cancel_drag {
                self.dragging_link = None;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_inspector(
        &mut self,
        ui: &mut egui::Ui,
        graph: &GraphSnapshot,
        config: Option<&Config>,
        health: &AudioHealthSnapshot,
        volumes: &BTreeMap<u32, NodeVolumeState>,
        client: &CrimpClient,
    ) {
        ui.horizontal(|ui| {
            theme::section_label(
                ui,
                if self.selected_link_id.is_some() {
                    "Detail propojení"
                } else {
                    "Detail audio uzlu"
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Zavřít").clicked() {
                    self.selected_link_id = None;
                    self.selected_node_id = None;
                }
            });
        });
        ui.add_space(8.0);

        if let Some(link_id) = self.selected_link_id
            && let Some(link) = graph.links.iter().find(|l| l.id == link_id)
        {
            theme::card_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("Link #{}", link.id))
                        .strong()
                        .size(15.0),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{}  ->  {}", link.output_label, link.input_label))
                        .color(theme::TEXT_MUTED),
                );
                if let Some(rule) = &link.enforced_by_rule {
                    ui.add_space(4.0);
                    ui.colored_label(theme::ACCENT_BLUE, format!("Vynuceno: {rule}"));
                }
                ui.add_space(10.0);
                if ui.button("Rozpojit tento link").clicked() {
                    self.record_and_destroy_link(link.id, graph, config, client);
                    self.selected_link_id = None;
                }
                if let Some(cfg) = config {
                    let src_node = graph.nodes.iter().find(|n| n.id == link.output_node_id);
                    let dst_node = graph.nodes.iter().find(|n| n.id == link.input_node_id);
                    if let (Some(src), Some(dst)) = (src_node, dst_node) {
                        ui.add_space(6.0);
                        if ui.button("+ Zakázat pravidlem ([[deny]])").clicked() {
                            let mut next = cfg.clone();
                            next.deny.push(DenyRule {
                                name: format!("Block {} -> {}", src.label, dst.label),
                                enabled: true,
                                source: node_to_matcher(src),
                                target: node_to_matcher(dst),
                            });
                            client.send_command(IpcRequest::SaveConfig { config: next });
                        }
                        if ui.button("+ Vynutit routu ([[route]])").clicked() {
                            let mut next = cfg.clone();
                            next.route.push(RouteRule {
                                name: format!("Route {} -> {}", src.label, dst.label),
                                enabled: true,
                                exclusive: true,
                                source: node_to_matcher(src),
                                target: node_to_matcher(dst),
                            });
                            client.send_command(IpcRequest::SaveConfig { config: next });
                        }
                    }
                }
            });
            return;
        }

        if let Some(node_id) = self.selected_node_id
            && let Some(node) = graph.nodes.iter().find(|n| n.id == node_id)
        {
            if self.alias_for_node_id != Some(node.id) {
                self.alias_for_node_id = Some(node.id);
                self.alias_input = if node.has_custom_alias {
                    node.label.clone()
                } else {
                    String::new()
                };
            }

            theme::card_frame().show(ui, |ui| {
                ui.label(egui::RichText::new(&node.label).strong().size(15.0));
                ui.label(
                    egui::RichText::new(node_subtitle(node))
                        .size(11.5)
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(8.0);
                egui::Grid::new("node_meta_grid")
                    .num_columns(2)
                    .spacing([12.0, 5.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("ID").color(theme::TEXT_MUTED));
                        ui.label(node.id.to_string());
                        ui.end_row();
                        ui.label(egui::RichText::new("node.name").color(theme::TEXT_MUTED));
                        ui.label(truncate_label(node.node_name.as_deref().unwrap_or("-"), 22));
                        ui.end_row();
                        ui.label(egui::RichText::new("app.name").color(theme::TEXT_MUTED));
                        ui.label(node.application_name.as_deref().unwrap_or("-"));
                        ui.end_row();
                        ui.label(egui::RichText::new("media.class").color(theme::TEXT_MUTED));
                        ui.label(node.media_class.as_deref().unwrap_or("-"));
                        ui.end_row();
                        if let Some(m) = health.per_node.get(&node.id) {
                            ui.label(egui::RichText::new("Signál").color(theme::TEXT_MUTED));
                            ui.label(
                                egui::RichText::new(m.state.label_cs()).color(
                                    if m.state == NodeActivityState::Running {
                                        theme::ECONOMY_GREEN
                                    } else {
                                        theme::TEXT_MUTED
                                    },
                                ),
                            );
                            ui.end_row();
                            ui.label(egui::RichText::new("Xruns / DSP").color(theme::TEXT_MUTED));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} xruns · {:.1}%",
                                    m.err_count,
                                    m.busy_ratio * 100.0
                                ))
                                .color(if m.err_count > 0 {
                                    theme::DANGER_RED
                                } else {
                                    theme::TEXT_PRIMARY
                                }),
                            );
                            ui.end_row();
                        }
                    });

                // Hlasitost & Ztlumení přímo v inspektoru
                let is_audio_node = node
                    .media_class
                    .as_deref()
                    .is_some_and(|c| c.contains("Audio"));
                if is_audio_node || volumes.contains_key(&node.id) {
                    let cur_vol = volumes.get(&node.id).copied().unwrap_or_default();
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Hlasitost & Ztlumení")
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    );
                    ui.horizontal(|ui| {
                        let mute_btn = if cur_vol.muted {
                            egui::Button::new(
                                egui::RichText::new("Ztlumeno").color(Color32::WHITE),
                            )
                            .fill(theme::DANGER_RED)
                            .corner_radius(CornerRadius::same(6))
                        } else {
                            egui::Button::new("Ztlumit").corner_radius(CornerRadius::same(6))
                        };
                        if ui.add(mute_btn).clicked() {
                            client.set_node_mute(node.id, !cur_vol.muted);
                        }
                        let mut pct = (cur_vol.volume * 100.0).round();
                        let slider = ui.add(
                            egui::Slider::new(&mut pct, 0.0..=150.0)
                                .step_by(1.0)
                                .suffix("%"),
                        );
                        if slider.changed() {
                            client.set_node_volume(node.id, pct / 100.0);
                        }
                    });
                }

                if let Some(cfg) = config {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Vlastní pojmenování (Alias)")
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    )
                    .on_hover_text(
                        "Přejmenuje tento proces/zařízení jen pro přehlednost v Crimpu. Interně se stále použije pravý PipeWire název.",
                    );
                    let alias_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.alias_input)
                            .desired_width(240.0)
                            .hint_text("Např. Minecraft nebo Sluchátka"),
                    );
                    let enter_save = alias_resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if (ui.button("Uložit název").clicked() || enter_save)
                            && !self.alias_input.trim().is_empty()
                        {
                            let new_name = self.alias_input.trim().to_string();
                            let matcher = node_to_exact_alias_matcher(node);
                            let mut next = cfg.clone();
                            next.alias.retain(|a| a.matcher != matcher);
                            next.alias.push(AliasRule {
                                matcher,
                                display_name: new_name,
                            });
                            client.send_command(IpcRequest::SaveConfig { config: next });
                        }
                        if node.has_custom_alias && ui.button("Původní název").clicked() {
                            let matcher = node_to_exact_alias_matcher(node);
                            let mut next = cfg.clone();
                            next.alias.retain(|a| a.matcher != matcher);
                            self.alias_input.clear();
                            client.send_command(IpcRequest::SaveConfig { config: next });
                        }
                    });
                }

                // Rychlé stereo propojení s jiným uzlem jedním klikem (L->L, R->R)
                if !node.output_ports.is_empty() {
                    let targets: Vec<&NodeSnapshot> = graph
                        .nodes
                        .iter()
                        .filter(|n| n.id != node.id && !n.input_ports.is_empty() && !is_internal_or_midi_node(n))
                        .collect();
                    if !targets.is_empty() {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Rychlé stereo připojení (L->L, R->R):")
                                .size(12.0)
                                .color(theme::TEXT_MUTED),
                        );
                        egui::ComboBox::from_id_salt(("quick_connect", node.id))
                            .selected_text("Vyber cílový uzel...")
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                for target in targets {
                                    if ui.selectable_label(false, &target.label).clicked() {
                                        let pairs = stereo_port_pairs(node, target);
                                        self.record_and_create_links(
                                            format!("Stereo {} -> {}", node.label, target.label),
                                            &pairs,
                                            config,
                                            client,
                                        );
                                    }
                                }
                            });
                    }
                }

                if let Some(name) = &node.node_name {
                    let class = node.media_class.as_deref().unwrap_or("");
                    if class.contains("Sink") {
                        ui.add_space(10.0);
                        if ui.button("Nastavit jako Default Sink").clicked() {
                            client.send_command(IpcRequest::SetDefaultDevice {
                                kind: DefaultDeviceKind::Sink,
                                node_name: Some(name.clone()),
                            });
                        }
                    } else if class.contains("Source") {
                        ui.add_space(10.0);
                        if ui.button("Nastavit jako Default Source").clicked() {
                            client.send_command(IpcRequest::SetDefaultDevice {
                                kind: DefaultDeviceKind::Source,
                                node_name: Some(name.clone()),
                            });
                        }
                    }
                }

                // Seznam aktivních spojení vybraného uzlu (pohodlné odpojení i na touchpadu)
                let node_links: Vec<_> = graph
                    .links
                    .iter()
                    .filter(|l| l.output_node_id == node.id || l.input_node_id == node.id)
                    .collect();
                if !node_links.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Aktivní spojení ({})", node_links.len()))
                                .size(12.0)
                                .color(theme::TEXT_MUTED),
                        );
                        if node_links.len() > 1 {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Odpojit vše").clicked() {
                                    let undo_links: Vec<UndoLinkOp> = node_links
                                        .iter()
                                        .map(|l| UndoLinkOp::Create {
                                            output_node_id: l.output_node_id,
                                            output_port_id: l.output_port_id,
                                            input_node_id: l.input_node_id,
                                            input_port_id: l.input_port_id,
                                        })
                                        .collect();
                                    let redo_links: Vec<UndoLinkOp> = node_links
                                        .iter()
                                        .map(|l| UndoLinkOp::Destroy {
                                            output_port_id: l.output_port_id,
                                            input_port_id: l.input_port_id,
                                        })
                                        .collect();
                                    self.push_undo(UndoEntry {
                                        label: format!("Odpojení všech linek ({})", node.label),
                                        prev_config: self
                                            .persist_topology
                                            .then(|| config.cloned())
                                            .flatten(),
                                        next_config: None,
                                        undo_links,
                                        redo_links,
                                    });
                                    for l in &node_links {
                                        client.send_command(IpcRequest::DestroyLink {
                                            link_id: l.id,
                                            persist: self.persist_topology,
                                        });
                                    }
                                }
                            });
                        }
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for l in &node_links {
                                let label = if l.output_node_id == node.id {
                                    format!("-> {}", l.input_label)
                                } else {
                                    format!("<- {}", l.output_label)
                                };
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(truncate_label(&label, 24))
                                            .size(11.5)
                                            .color(theme::TEXT_MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("Odpojit").clicked() {
                                                self.record_and_destroy_link(
                                                    l.id, graph, config, client,
                                                );
                                            }
                                        },
                                    );
                                });
                            }
                        });
                }
            });

            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "Ovládání: Přetažením nebo kliknutím z libovolného konce (zelený <-> modrý) propojíš porty. Podržením Shift nebo tlačítkem Stereo L+R spojíš oba kanály najednou. Pravý klik na drát nebo Delete odpojí, Ctrl+Z / Ctrl+Y vrací změny zpět. Klik na % / MUTE na kartě rychle ztlumí uzel.",
                )
                .color(theme::TEXT_SUBTLE)
                .size(11.5),
            );
        }
    }
}

fn stereo_port_pairs(src: &NodeSnapshot, dst: &NodeSnapshot) -> Vec<(u32, u32, u32, u32)> {
    let outs = sorted_ports(&src.output_ports);
    let ins = sorted_ports(&dst.input_ports);
    let pairs = outs.len().min(ins.len());
    (0..pairs)
        .map(|i| (src.id, outs[i].id, dst.id, ins[i].id))
        .collect()
}

fn apply_undo_link_ops(ops: &[UndoLinkOp], graph: &GraphSnapshot, client: &CrimpClient) {
    for op in ops {
        match *op {
            UndoLinkOp::Create {
                output_node_id,
                output_port_id,
                input_node_id,
                input_port_id,
            } => {
                client.send_command(IpcRequest::CreateLink {
                    output_node_id,
                    output_port_id,
                    input_node_id,
                    input_port_id,
                    replace_input: false,
                    persist: false,
                });
            }
            UndoLinkOp::Destroy {
                output_port_id,
                input_port_id,
            } => {
                if let Some(link) = graph.links.iter().find(|l| {
                    l.output_port_id == output_port_id && l.input_port_id == input_port_id
                }) {
                    client.send_command(IpcRequest::DestroyLink {
                        link_id: link.id,
                        persist: false,
                    });
                }
            }
        }
    }
}

fn node_to_exact_alias_matcher(node: &NodeSnapshot) -> NodeMatcher {
    NodeMatcher {
        node_name: node.node_name.clone(),
        application_name: node.application_name.clone(),
        media_class: node.media_class.clone(),
        port_name: None,
    }
}

fn is_internal_or_midi_node(node: &NodeSnapshot) -> bool {
    let name = node.node_name.as_deref().unwrap_or("").to_ascii_lowercase();
    let class = node
        .media_class
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    if name.contains("dummy-driver") || name.contains("freewheel-driver") || name.contains("midi") {
        return true;
    }
    if class.contains("midi") {
        return true;
    }
    node.input_ports.is_empty() && node.output_ports.is_empty()
}

fn sorted_ports(ports: &[PortInfo]) -> Vec<&PortInfo> {
    let mut list: Vec<&PortInfo> = ports.iter().collect();
    list.sort_by_key(|p| (channel_sort_rank(p), p.port_name.as_deref().unwrap_or("")));
    list
}

fn channel_sort_rank(port: &PortInfo) -> (u8, u32) {
    let raw_key = port
        .channel
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| port.port_name.as_deref().and_then(|n| n.rsplit('_').next()))
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();

    let is_monitor = port
        .port_name
        .as_deref()
        .is_some_and(|n| n.to_ascii_lowercase().starts_with("monitor"));

    let base = match raw_key.as_str() {
        "FL" | "L" | "LEFT" | "AUX0" | "0" | "1" => 0,
        "FR" | "R" | "RIGHT" | "AUX1" | "2" => 1,
        "FC" | "C" | "CENTER" | "MONO" => 2,
        "LFE" | "SUB" => 3,
        "RL" | "BL" => 4,
        "RR" | "BR" => 5,
        "SL" => 6,
        "SR" => 7,
        _ => 20,
    };
    (base + if is_monitor { 30 } else { 0 }, port.id)
}

fn format_port_name(port: &PortInfo) -> String {
    let raw = port.port_name.as_deref().unwrap_or("port");
    raw.strip_prefix("playback_")
        .or_else(|| raw.strip_prefix("capture_"))
        .or_else(|| raw.strip_prefix("output_"))
        .or_else(|| raw.strip_prefix("input_"))
        .unwrap_or(raw)
        .to_string()
}

fn node_accent_color(node: &NodeSnapshot) -> Color32 {
    if node.is_virtual_crimp {
        return theme::ECONOMY_GREEN;
    }
    let class = node.media_class.as_deref().unwrap_or("");
    if class.starts_with("Audio/Sink") || class.starts_with("Audio/Source") {
        theme::HARDWARE_AMBER
    } else {
        theme::ACCENT_BLUE
    }
}

fn node_subtitle(node: &NodeSnapshot) -> String {
    if node.has_custom_alias
        && let Some(orig) = &node.node_name
    {
        return format!("{orig}  ·  #{}", node.id);
    }
    if node.is_virtual_crimp {
        return format!("Virtual Bus  ·  #{}", node.id);
    }
    let kind = match node.media_class.as_deref().unwrap_or("") {
        "Stream/Output/Audio" => "Audio Stream",
        "Stream/Input/Audio" => "Recording Stream",
        "Audio/Sink" => "Output Device",
        "Audio/Source" => "Input / Mic",
        "Audio/Duplex" => "Duplex Device",
        other if !other.is_empty() => other,
        _ => "PipeWire Node",
    };
    format!("{kind}  ·  #{}", node.id)
}

fn node_to_matcher(node: &NodeSnapshot) -> NodeMatcher {
    if let Some(app) = &node.application_name {
        NodeMatcher {
            application_name: Some(app.clone()),
            ..Default::default()
        }
    } else {
        NodeMatcher {
            node_name: node.node_name.clone(),
            ..Default::default()
        }
    }
}

fn classify_column(node: &NodeSnapshot) -> usize {
    if node.is_virtual_crimp {
        return 1;
    }
    let has_out = !node.output_ports.is_empty();
    let has_in = !node.input_ports.is_empty();
    match (has_out, has_in) {
        (true, false) => 0,
        (true, true) => 1,
        _ => 2,
    }
}

fn node_width(node: &NodeSnapshot) -> f32 {
    if !node.input_ports.is_empty() && !node.output_ports.is_empty() {
        304.0
    } else {
        264.0
    }
}

fn node_height(node: &NodeSnapshot) -> f32 {
    let rows = node.input_ports.len().max(node.output_ports.len()).max(1);
    HEADER_HEIGHT + rows as f32 * PORT_ROW_HEIGHT + 10.0
}

fn draw_subtle_grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32) {
    let step = 32.0 * zoom;
    if step < 10.0 {
        return;
    }
    let ox = pan.x.rem_euclid(step);
    let oy = pan.y.rem_euclid(step);
    let mut x = rect.left() + ox;
    while x < rect.right() {
        let mut y = rect.top() + oy;
        while y < rect.bottom() {
            painter.circle_filled(Pos2::new(x, y), 1.0, theme::GRID_DOT);
            y += step;
        }
        x += step;
    }
}

fn draw_cable(painter: &egui::Painter, p0: Pos2, p3: Pos2, color: Color32, width: f32) {
    let dx = ((p3.x - p0.x).abs() * 0.45).max(56.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    painter.add(CubicBezierShape::from_points_stroke(
        [p0, p1, p2, p3],
        false,
        Color32::TRANSPARENT,
        PathStroke::new(width, color),
    ));
}

fn bezier_min_distance(p0: Pos2, p3: Pos2, target: Pos2) -> f32 {
    let dx = ((p3.x - p0.x).abs() * 0.45).max(56.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    let steps = 24;
    let mut best = f32::MAX;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let pt = Pos2::new(
            u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
            u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
        );
        best = best.min(pt.distance(target));
    }
    best
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

// Re-export pro potlačení nepoužitého importu BTreeMap v budoucnu (využívá SavedLayout).
#[allow(dead_code)]
fn _unused_check(_m: BTreeMap<String, String>) {}
