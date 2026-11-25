use egui::{Color32, CornerRadius, Frame, Margin, Stroke, Style, Visuals};

pub const BG_OBSIDIAN: Color32 = Color32::from_rgb(10, 10, 12);
pub const BG_SECONDARY: Color32 = Color32::from_rgb(15, 15, 19);
pub const SURFACE: Color32 = Color32::from_rgb(18, 18, 21);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(24, 24, 28);
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(32, 32, 38);

pub const ACCENT_BLUE: Color32 = Color32::from_rgb(10, 103, 229);
pub const ECONOMY_GREEN: Color32 = Color32::from_rgb(33, 222, 0);
pub const DANGER_RED: Color32 = Color32::from_rgb(245, 21, 21);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(241, 245, 249);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 163, 184);

pub const BORDER_SUBTLE: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 18);
pub const BORDER_STRONG: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 36);
pub const SPECULAR_TOP: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 28);

/// Aplikuje designový jazyk Matte Obsidian (Apple / Linear minimalismus bez glow bloatu).
pub fn apply_obsidian_style(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = BG_OBSIDIAN;
    visuals.window_fill = SURFACE;
    visuals.faint_bg_color = BG_SECONDARY;
    visuals.extreme_bg_color = Color32::from_rgb(7, 7, 9);
    visuals.window_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.window_corner_radius = CornerRadius::same(10);

    visuals.selection.bg_fill = ACCENT_BLUE;
    visuals.selection.stroke = Stroke::new(1.0, TEXT_PRIMARY);

    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = BG_SECONDARY;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);

    visuals.widgets.inactive.bg_fill = SURFACE_HOVER;
    visuals.widgets.inactive.weak_bg_fill = SURFACE_HOVER;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);

    visuals.widgets.hovered.bg_fill = SURFACE_ACTIVE;
    visuals.widgets.hovered.weak_bg_fill = SURFACE_ACTIVE;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);

    visuals.widgets.active.bg_fill = ACCENT_BLUE;
    visuals.widgets.active.weak_bg_fill = ACCENT_BLUE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);

    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[must_use]
pub fn card_frame() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
}

pub fn spec_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 14.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, BORDER_STRONG),
    );
}

pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_ascii_uppercase())
            .size(11.0)
            .color(TEXT_MUTED)
            .strong(),
    );
}
