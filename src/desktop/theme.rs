use eframe::egui::{self, Color32, FontId, RichText, Stroke, TextStyle};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Mode {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub canvas: Color32,
    pub explorer: Color32,
    pub chrome: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub selected: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
}
impl Palette {
    pub fn for_dark(dark: bool) -> Self {
        let rgb = Color32::from_rgb;
        if dark {
            Self {
                canvas: rgb(23, 30, 40),
                explorer: rgb(28, 37, 49),
                chrome: rgb(32, 41, 54),
                border: rgb(48, 59, 73),
                text: rgb(218, 226, 237),
                muted: rgb(158, 171, 188),
                accent: rgb(114, 169, 255),
                selected: rgb(39, 65, 100),
                success: rgb(111, 206, 159),
                warning: rgb(238, 190, 106),
                error: rgb(255, 143, 151),
            }
        } else {
            Self {
                canvas: rgb(255, 255, 255),
                explorer: rgb(243, 245, 248),
                chrome: rgb(232, 237, 243),
                border: rgb(207, 215, 225),
                text: rgb(30, 44, 62),
                muted: rgb(83, 101, 123),
                accent: rgb(36, 92, 179),
                selected: rgb(219, 233, 255),
                success: rgb(29, 112, 75),
                warning: rgb(137, 86, 12),
                error: rgb(175, 40, 57),
            }
        }
    }
}
pub(super) fn apply(ctx: &egui::Context, mode: Mode) -> Palette {
    let preference = match mode {
        Mode::System => egui::ThemePreference::System,
        Mode::Light => egui::ThemePreference::Light,
        Mode::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(preference);
    let dark = match mode {
        Mode::System => ctx.theme() == egui::Theme::Dark,
        Mode::Light => false,
        Mode::Dark => true,
    };
    let palette = Palette::for_dark(dark);
    let mut style = (*ctx.global_style()).clone();
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let v = &mut style.visuals;
    v.panel_fill = palette.canvas;
    v.window_fill = palette.explorer;
    v.override_text_color = Some(palette.text);
    v.weak_text_color = Some(palette.muted);
    v.extreme_bg_color = palette.canvas;
    v.text_edit_bg_color = Some(palette.canvas);
    v.faint_bg_color = palette.explorer;
    v.code_bg_color = palette.explorer;
    v.window_stroke = Stroke::new(1.0, palette.border);
    v.selection.bg_fill = palette.selected;
    v.selection.stroke = Stroke::new(1.0, palette.accent);
    v.hyperlink_color = palette.accent;
    v.warn_fg_color = palette.warning;
    v.error_fg_color = palette.error;
    v.text_cursor.stroke = Stroke::new(2.0, palette.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    for widget in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(4);
        widget.fg_stroke = Stroke::new(1.0, palette.text);
        widget.bg_stroke = Stroke::new(1.0, palette.border);
        widget.bg_fill = palette.chrome;
        widget.weak_bg_fill = palette.chrome;
    }
    v.widgets.hovered.bg_fill = palette.selected;
    v.widgets.hovered.weak_bg_fill = palette.selected;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent);
    v.widgets.active.bg_fill = palette.selected;
    v.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    style.spacing.interact_size = egui::vec2(28.0, 28.0);
    style.spacing.indent = 16.0;
    // Bundled Ubuntu and Hack are deterministic across all supported platforms.
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(18.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(12.0));
    ctx.set_global_style(style);
    palette
}

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Server,
    Cloud,
    Terminal,
    Tasks,
    Search,
    Settings,
    Close,
    Maximize,
    Refresh,
}
pub(super) fn icon(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    palette: Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
    if selected || response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect.shrink(2.0), 4.0, palette.selected);
    }
    let color = if selected {
        palette.accent
    } else {
        palette.muted
    };
    let stroke = Stroke::new(1.5, color);
    let origin = rect.center() - egui::vec2(9.0, 9.0);
    let point = |x, y| origin + egui::vec2(x, y);
    let painter = ui.painter();
    let line = |a: [f32; 2], b: [f32; 2]| {
        painter.line_segment([point(a[0], a[1]), point(b[0], b[1])], stroke);
    };
    let box_at = |x, y, w, h| {
        painter.rect_stroke(
            egui::Rect::from_min_size(point(x, y), egui::vec2(w, h)),
            2.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Server => {
            box_at(1.0, 1.0, 16.0, 7.0);
            box_at(1.0, 10.0, 16.0, 7.0);
            line([4.0, 4.5], [7.0, 4.5]);
            line([4.0, 13.5], [7.0, 13.5]);
        }
        Icon::Cloud => {
            painter.circle_stroke(point(7.0, 8.0), 4.5, stroke);
            painter.circle_stroke(point(12.0, 10.0), 4.0, stroke);
            line([3.0, 14.0], [14.0, 14.0]);
        }
        Icon::Terminal => {
            box_at(0.0, 1.0, 18.0, 16.0);
            line([4.0, 6.0], [7.0, 9.0]);
            line([7.0, 9.0], [4.0, 12.0]);
            line([10.0, 12.0], [14.0, 12.0]);
        }
        Icon::Tasks => {
            for y in [3.0, 9.0, 15.0] {
                line([6.0, y], [17.0, y]);
                box_at(0.0, y - 1.5, 3.0, 3.0);
            }
        }
        Icon::Search => {
            painter.circle_stroke(point(7.0, 7.0), 5.5, stroke);
            line([11.0, 11.0], [17.0, 17.0]);
        }
        Icon::Settings => {
            painter.circle_stroke(point(9.0, 9.0), 6.0, stroke);
            painter.circle_stroke(point(9.0, 9.0), 2.0, stroke);
            for (a, b) in [
                ([9.0, 0.0], [9.0, 3.0]),
                ([9.0, 15.0], [9.0, 18.0]),
                ([0.0, 9.0], [3.0, 9.0]),
                ([15.0, 9.0], [18.0, 9.0]),
            ] {
                line(a, b);
            }
        }
        Icon::Close => {
            line([4.0, 4.0], [14.0, 14.0]);
            line([14.0, 4.0], [4.0, 14.0]);
        }
        Icon::Maximize => {
            box_at(2.0, 2.0, 14.0, 14.0);
        }
        Icon::Refresh => {
            painter.circle_stroke(point(9.0, 9.0), 6.0, stroke);
            line([14.0, 2.0], [16.0, 7.0]);
            line([16.0, 7.0], [11.0, 6.0]);
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response.on_hover_text(label)
}
pub(super) fn primary(ui: &mut egui::Ui, label: &str, palette: Palette) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).color(palette.canvas)).fill(palette.accent))
}
