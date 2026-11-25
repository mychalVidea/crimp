use crate::client::CrimpClient;
use crate::theme;
use crimp_proto::config::{Config, DenyRule, NodeMatcher, RouteRule};
use crimp_proto::graph::{GraphSnapshot, NodeSnapshot, PortInfo};
use crimp_proto::ipc::{DefaultDeviceKind, IpcRequest};
use egui::epaint::{CubicBezierShape, PathStroke, Shadow};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};

const HEADER_HEIGHT: f32 = 44.0;
const PORT_ROW_HEIGHT: f32 = 24.0;
const PORT_PIN_RADIUS: f32 = 5.4;

#[derive(Debug, Clone, Copy)]
struct PendingDragLink {
    output_node_id: u32,
    output_port_id: u32,
}

/// Stav interaktivního audio Patchbay plátna (vzhled Studio / Apple Matte Obsidian).
pub struct PatchbayState {
    positions: HashMap<u32, Pos2>,
    pan: Vec2,
    zoom: f32,
    needs_fit: bool,
    hide_internal_and_midi: bool,
    dragging_link: Option<PendingDragLink>,
    selected_node_id: Option<u32>,
    selected_link_id: Option<u32>,
    filter_query: String,
}

impl Default for PatchbayState {
    fn default() -> Self {
        Self {
            positions: HashMap::new(),
            pan: Vec2::new(40.0, 40.0),
            zoom: 1.0,
            needs_fit: true,
            hide_internal_and_midi: true,
            dragging_link: None,
            selected_node_id: None,
            selected_link_id: None,
            filter_query: String::new(),
        }
    }
}

impl PatchbayState {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: Option<&GraphSnapshot>,
        config: Option<&Config>,
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

        // Horní ovládací pruh patchbaye
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_query)
                    .desired_width(200.0)
                    .hint_text("Hledat aplikaci nebo zařízení..."),
            );
            if ui.button("Auto-layout").clicked() {
                self.positions.clear();
                self.needs_fit = true;
            }
            if ui.button("Vycentrovat").clicked() {
                self.needs_fit = true;
            }
            ui.checkbox(
                &mut self.hide_internal_and_midi,
                "Jen Audio (skrýt Dummy/MIDI)",
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} uzlů  ·  {} linků  ·  {:.0}%",
                        graph.nodes.len(),
                        graph.links.len(),
                        self.zoom * 100.0
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
                .default_size(300.0)
                .min_size(250.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::BG_SECONDARY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ui, |ui| {
                    self.draw_inspector(ui, graph, config, client);
                });
        }

        let live_ids: HashSet<u32> = graph.nodes.iter().map(|n| n.id).collect();
        self.positions.retain(|id, _| live_ids.contains(id));

        let visible_nodes: Vec<&NodeSnapshot> = graph
            .nodes
            .iter()
            .filter(|node| self.matches_filter(node))
            .collect();

        self.ensure_initial_layout(&visible_nodes);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_OBSIDIAN)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .corner_radius(CornerRadius::same(10)),
            )
            .show(ui, |ui| {
                self.draw_canvas(ui, graph, &visible_nodes, client);
            });
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
                .media_class
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
    }

    fn ensure_initial_layout(&mut self, nodes: &[&NodeSnapshot]) {
        let mut col_y = [0.0f32; 3];
        for &node in nodes {
            if self.positions.contains_key(&node.id) {
                continue;
            }
            let col = classify_column(node);
            let x = col as f32 * 390.0;
            let y = col_y[col];
            self.positions.insert(node.id, Pos2::new(x, y));
            col_y[col] += node_height(node) + 32.0;
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
            .clamp(0.65, 1.05);
        let scaled_w = content_w * self.zoom;
        let scaled_h = content_h * self.zoom;
        self.pan = Vec2::new(
            (canvas_rect.width() - scaled_w) * 0.5 - min_x * self.zoom,
            (canvas_rect.height() - scaled_h) * 0.5 - min_y * self.zoom,
        );
        self.needs_fit = false;
    }

    fn draw_canvas(
        &mut self,
        ui: &mut egui::Ui,
        graph: &GraphSnapshot,
        visible_nodes: &[&NodeSnapshot],
        client: &CrimpClient,
    ) {
        let canvas_rect = ui.available_rect_before_wrap();
        if self.needs_fit && !visible_nodes.is_empty() {
            self.fit_to_canvas(canvas_rect, visible_nodes);
        }

        let canvas_resp = ui.allocate_rect(canvas_rect, Sense::click_and_drag());

        if canvas_resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let factor = (1.0 + scroll * 0.0018).clamp(0.85, 1.2);
                let new_zoom = (self.zoom * factor).clamp(0.45, 1.75);
                if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                    let local_ptr = pointer - canvas_rect.min;
                    let world_before = (local_ptr - self.pan) / self.zoom;
                    self.zoom = new_zoom;
                    self.pan = local_ptr - world_before * self.zoom;
                } else {
                    self.zoom = new_zoom;
                }
            }
        }

        if canvas_resp.dragged_by(egui::PointerButton::Middle)
            || (canvas_resp.dragged_by(egui::PointerButton::Primary)
                && self.dragging_link.is_none())
        {
            self.pan += canvas_resp.drag_delta();
        }

        if canvas_resp.clicked() {
            self.selected_link_id = None;
        }

        let painter = ui.painter_at(canvas_rect);
        draw_subtle_grid(&painter, canvas_rect, self.pan, self.zoom);

        let mut port_screen_pos: HashMap<u32, Pos2> = HashMap::new();
        let mut in_pin_rects: Vec<(u32, u32, Rect)> = Vec::new();
        let mut out_pin_rects: Vec<(u32, u32, Rect)> = Vec::new();

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

            // Měkký Apple drop-shadow pod kartou
            let shadow = Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 135),
            };
            painter.add(shadow.as_shape(rect, rounding));

            // Tělo karty
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

            // Tónovaná hlavička
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
            // Horní 1px specular odlesk
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 10.0, rect.top() + 1.0),
                    Pos2::new(rect.right() - 10.0, rect.top() + 1.0),
                ],
                Stroke::new(1.0, theme::SPECULAR_TOP),
            );

            // Barevný levý akcent v záhlaví
            let pill_rect = Rect::from_min_size(
                Pos2::new(
                    header_rect.left() + 10.0 * self.zoom,
                    header_rect.top() + 10.0 * self.zoom,
                ),
                Vec2::new(3.0 * self.zoom, (HEADER_HEIGHT - 20.0) * self.zoom),
            );
            painter.rect_filled(pill_rect, CornerRadius::same(2), accent);

            // Přetahování uzlu přes záhlaví
            let header_id = ui.scope_id().with(("node_drag", node.id));
            let header_resp = ui.interact(header_rect, header_id, Sense::click_and_drag());
            if header_resp.dragged()
                && let Some(pos) = self.positions.get_mut(&node.id)
            {
                *pos += header_resp.drag_delta() / self.zoom;
            }
            if header_resp.clicked() {
                self.selected_node_id = Some(node.id);
                self.selected_link_id = None;
            }

            let title_x = header_rect.left() + 20.0 * self.zoom;
            painter.text(
                Pos2::new(title_x, header_rect.top() + 14.0 * self.zoom),
                Align2::LEFT_CENTER,
                truncate_label(&node.label, 28),
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

            // Seřazené porty (FL před FR atd., aby se linky zbytečně nekřížily!)
            let in_ports = sorted_ports(&node.input_ports);
            let out_ports = sorted_ports(&node.output_ports);
            let has_both = !in_ports.is_empty() && !out_ports.is_empty();
            let max_port_chars = if has_both { 15 } else { 28 };
            let rows = in_ports.len().max(out_ports.len());

            for row in 0..rows {
                let row_y = header_rect.bottom() + (row as f32 + 0.5) * PORT_ROW_HEIGHT * self.zoom;

                if let Some(in_port) = in_ports.get(row) {
                    let pin_pos = Pos2::new(rect.left(), row_y);
                    port_screen_pos.insert(in_port.id, pin_pos);
                    let hit_rect = Rect::from_center_size(pin_pos, Vec2::splat(16.0 * self.zoom));
                    in_pin_rects.push((node.id, in_port.id, hit_rect));

                    painter.circle_filled(pin_pos, PORT_PIN_RADIUS * self.zoom, theme::BG_OBSIDIAN);
                    painter.circle_stroke(
                        pin_pos,
                        PORT_PIN_RADIUS * self.zoom,
                        Stroke::new(1.8, theme::ACCENT_BLUE),
                    );
                    painter.text(
                        Pos2::new(pin_pos.x + 12.0 * self.zoom, row_y),
                        Align2::LEFT_CENTER,
                        truncate_label(&format_port_name(in_port), max_port_chars),
                        FontId::proportional((11.5 * self.zoom).clamp(8.0, 15.0)),
                        theme::TEXT_MUTED,
                    );
                }

                if let Some(out_port) = out_ports.get(row) {
                    let pin_pos = Pos2::new(rect.right(), row_y);
                    port_screen_pos.insert(out_port.id, pin_pos);
                    let hit_rect = Rect::from_center_size(pin_pos, Vec2::splat(16.0 * self.zoom));
                    out_pin_rects.push((node.id, out_port.id, hit_rect));

                    painter.circle_filled(pin_pos, PORT_PIN_RADIUS * self.zoom, theme::BG_OBSIDIAN);
                    painter.circle_stroke(
                        pin_pos,
                        PORT_PIN_RADIUS * self.zoom,
                        Stroke::new(1.8, theme::ECONOMY_GREEN),
                    );
                    painter.text(
                        Pos2::new(pin_pos.x - 12.0 * self.zoom, row_y),
                        Align2::RIGHT_CENTER,
                        truncate_label(&format_port_name(out_port), max_port_chars),
                        FontId::proportional((11.5 * self.zoom).clamp(8.0, 15.0)),
                        theme::TEXT_MUTED,
                    );
                }
            }
        }

        // Tahání nového kabelu z Output pinu
        for &(out_node_id, out_port_id, hit_rect) in &out_pin_rects {
            let resp = ui.interact(
                hit_rect,
                ui.scope_id().with(("out_pin", out_port_id)),
                Sense::click_and_drag(),
            );
            if resp.drag_started() {
                self.dragging_link = Some(PendingDragLink {
                    output_node_id: out_node_id,
                    output_port_id: out_port_id,
                });
            }
        }

        // Kreslení kabelů (se spodním tmavým halo pruhem pro čitelné křížení)
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let primary_clicked = ui.input(|i| i.pointer.primary_clicked());
        let secondary_clicked = ui.input(|i| i.pointer.secondary_clicked());

        for link in &graph.links {
            let (Some(&p0), Some(&p3)) = (
                port_screen_pos.get(&link.output_port_id),
                port_screen_pos.get(&link.input_port_id),
            ) else {
                continue;
            };

            let hovered = pointer_pos.is_some_and(|ptr| bezier_near_point(p0, p3, ptr, 9.0));
            let selected = self.selected_link_id == Some(link.id);

            if hovered && (primary_clicked || secondary_clicked) {
                self.selected_link_id = Some(link.id);
                self.selected_node_id = None;
            }
            if hovered && secondary_clicked {
                client.send_command(IpcRequest::DestroyLink { link_id: link.id });
            }

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
            if let (Some(&start), Some(cur)) =
                (port_screen_pos.get(&drag.output_port_id), pointer_pos)
            {
                draw_cable(&painter, start, cur, theme::BG_OBSIDIAN, 4.6);
                draw_cable(&painter, start, cur, theme::ECONOMY_GREEN, 2.3);
            }

            if ui.input(|i| i.pointer.any_released()) {
                if let Some(release_pos) = pointer_pos
                    && let Some(&(in_node_id, in_port_id, _)) = in_pin_rects
                        .iter()
                        .find(|(_, _, r)| r.contains(release_pos))
                    && in_node_id != drag.output_node_id
                {
                    client.send_command(IpcRequest::CreateLink {
                        output_node_id: drag.output_node_id,
                        output_port_id: drag.output_port_id,
                        input_node_id: in_node_id,
                        input_port_id: in_port_id,
                    });
                }
                self.dragging_link = None;
            }
        }
    }

    fn draw_inspector(
        &mut self,
        ui: &mut egui::Ui,
        graph: &GraphSnapshot,
        config: Option<&Config>,
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
                if ui.button("Rozpojit link").clicked() {
                    client.send_command(IpcRequest::DestroyLink { link_id: link.id });
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
                        ui.label(egui::RichText::new("latency").color(theme::TEXT_MUTED));
                        ui.label(node.node_latency.as_deref().unwrap_or("-"));
                        ui.end_row();
                    });

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
            });

            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "Přetažením ze zeleného výstupu na modrý vstup vytvoříš propojení. Pravý klik na drát link ihned rozpojí.",
                )
                .color(theme::TEXT_SUBTLE)
                .size(11.5),
            );
        }
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
    let key = port
        .channel
        .as_deref()
        .or_else(|| port.port_name.as_deref().and_then(|n| n.rsplit('_').next()))
        .unwrap_or("")
        .to_ascii_uppercase();
    let is_monitor = port
        .port_name
        .as_deref()
        .is_some_and(|n| n.to_ascii_lowercase().starts_with("monitor"));
    let base = match key.as_str() {
        "FL" | "L" | "LEFT" | "AUX0" => 0,
        "FR" | "R" | "RIGHT" | "AUX1" => 1,
        "FC" | "C" | "CENTER" | "MONO" => 2,
        "LFE" | "SUB" => 3,
        "RL" | "BL" => 4,
        "RR" | "BR" => 5,
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

fn bezier_near_point(p0: Pos2, p3: Pos2, target: Pos2, threshold: f32) -> bool {
    let dx = ((p3.x - p0.x).abs() * 0.45).max(56.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    let steps = 20;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let pt = Pos2::new(
            u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
            u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
        );
        if pt.distance(target) <= threshold {
            return true;
        }
    }
    false
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}
