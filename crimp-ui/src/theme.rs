use egui::{Color32, CornerRadius, Frame, Margin, Stroke, Style, Visuals};

pub const BG_OBSIDIAN: Color32 = Color32::from_rgb(10, 10, 12);
pub const BG_SECONDARY: Color32 = Color32::from_rgb(15, 15, 19);
pub const SURFACE: Color32 = Color32::from_rgb(20, 20, 25);
pub const SURFACE_HEADER: Color32 = Color32::from_rgb(26, 26, 33);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(29, 30, 37);
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(39, 41, 50);

pub const ACCENT_BLUE: Color32 = Color32::from_rgb(10, 103, 229);
pub const ECONOMY_GREEN: Color32 = Color32::from_rgb(33, 222, 0);
pub const DANGER_RED: Color32 = Color32::from_rgb(245, 21, 21);
pub const HARDWARE_AMBER: Color32 = Color32::from_rgb(245, 158, 11);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(241, 245, 249);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 163, 184);
pub const TEXT_SUBTLE: Color32 = Color32::from_rgb(100, 116, 139);

// V premultiplied RGBA je bílá o průhlednosti A rovna (A, A, A, A).
pub const BORDER_SUBTLE: Color32 = Color32::from_rgba_premultiplied(18, 18, 20, 18);
pub const BORDER_STRONG: Color32 = Color32::from_rgba_premultiplied(36, 36, 40, 36);
pub const SPECULAR_TOP: Color32 = Color32::from_rgba_premultiplied(24, 24, 26, 24);
pub const GRID_DOT: Color32 = Color32::from_rgb(28, 29, 35);

/// Aplikuje designový jazyk Matte Obsidian (Apple / Linear minimalismus bez křiklavých okrajů).
pub fn apply_obsidian_style(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = BG_OBSIDIAN;
    visuals.window_fill = SURFACE;
    visuals.faint_bg_color = BG_SECONDARY;
    visuals.extreme_bg_color = Color32::from_rgb(12, 12, 15);
    visuals.window_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.window_corner_radius = CornerRadius::same(10);

    visuals.selection.bg_fill = ACCENT_BLUE;
    visuals.selection.stroke = Stroke::new(1.0, TEXT_PRIMARY);

    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = BG_SECONDARY;
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(7);

    visuals.widgets.inactive.bg_fill = SURFACE_HOVER;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(7);

    visuals.widgets.hovered.bg_fill = SURFACE_ACTIVE;
    visuals.widgets.hovered.weak_bg_fill = SURFACE_ACTIVE;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(7);

    visuals.widgets.active.bg_fill = ACCENT_BLUE;
    visuals.widgets.active.weak_bg_fill = ACCENT_BLUE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(7);

    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.window_margin = Margin::same(12);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[must_use]
pub fn card_frame() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
}

pub fn spec_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 14.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, BORDER_SUBTLE),
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
