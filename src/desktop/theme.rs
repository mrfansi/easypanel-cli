use eframe::egui::{self, Color32, FontId, RichText, Stroke, TextStyle};
use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

pub(super) fn install_fonts(ctx: &egui::Context) {
    static FALLBACKS: LazyLock<Vec<Arc<egui::FontData>>> = LazyLock::new(|| {
        #[cfg(target_os = "macos")]
        let paths = [
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
        ];
        // Debian/Ubuntu fonts-noto-cjk; other distributions may not install this path.
        #[cfg(target_os = "linux")]
        let paths = ["/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"];
        #[cfg(target_os = "windows")]
        let paths = std::env::var_os("WINDIR").into_iter().flat_map(|root| {
            let fonts = std::path::PathBuf::from(root).join("Fonts");
            ["msyh.ttc", "malgun.ttf", "YuGothR.ttc"].map(|name| fonts.join(name))
        });
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let paths: [&str; 0] = [];
        load_cjk_fonts(paths)
    });
    ctx.set_fonts(fonts_with_fallback(&FALLBACKS));
}

fn load_cjk_fonts(paths: impl IntoIterator<Item = impl AsRef<Path>>) -> Vec<Arc<egui::FontData>> {
    use read_fonts::{FileRef, TableProvider as _};

    let mut fallbacks = Vec::new();
    let mut covered = [false; 3];
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let bytes: egui::epaint::text::Blob = Arc::new(bytes);
        let Ok(file) = FileRef::new(bytes.as_ref().as_ref()) else {
            continue;
        };
        for (index, face) in file.fonts().enumerate() {
            let Ok(face) = face else { continue };
            let Ok(cmap) = face.cmap() else { continue };
            let Some((_, _, subtable)) = cmap.best_subtable() else {
                continue;
            };
            // A collection can need different faces for Han, Kana and Hangul.
            let mut contributes = false;
            for (supported, sample) in covered.iter_mut().zip(["中文", "かなカナ", "한국어한글"])
            {
                if !*supported
                    && sample
                        .chars()
                        .all(|c| subtable.map_codepoint(c).is_some_and(|id| id.to_u32() != 0))
                {
                    *supported = true;
                    contributes = true;
                }
            }
            if contributes {
                fallbacks.push(Arc::new(egui::FontData::from_blob(
                    Arc::clone(&bytes),
                    index as u32,
                )));
            }
            if covered.iter().all(|supported| *supported) {
                return fallbacks;
            }
        }
    }
    fallbacks
}

fn fonts_with_fallback(fallbacks: &[Arc<egui::FontData>]) -> egui::FontDefinitions {
    let mut definitions = egui::FontDefinitions::default();
    for (index, fallback) in fallbacks.iter().enumerate() {
        let name = format!("system-cjk-{index}");
        definitions
            .font_data
            .insert(name.clone(), Arc::clone(fallback));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push(name.clone());
        }
    }
    // shortcut: without an installed OS CJK font, retain bundled fonts; add discovery when more paths are needed.
    definitions
}

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
            for (a, b) in [
                ([2.0, 7.0], [2.0, 2.0]),
                ([2.0, 2.0], [7.0, 2.0]),
                ([11.0, 2.0], [16.0, 2.0]),
                ([16.0, 2.0], [16.0, 7.0]),
                ([16.0, 11.0], [16.0, 16.0]),
                ([16.0, 16.0], [11.0, 16.0]),
                ([7.0, 16.0], [2.0, 16.0]),
                ([2.0, 16.0], [2.0, 11.0]),
            ] {
                line(a, b);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{FontDefinitions, FontFamily};

    fn assert_combining_cluster_renders(fonts: &mut egui::epaint::text::FontsView<'_>, id: FontId) {
        let combining = fonts.layout_no_wrap("e\u{301}".into(), id.clone(), Color32::WHITE);
        let composed = fonts.layout_no_wrap("é".into(), id, Color32::WHITE);
        assert_eq!(combining.text().as_bytes(), "e\u{301}".as_bytes());
        assert_eq!(
            combining.rows[0].glyphs.len(),
            2,
            "retain both caret positions"
        );
        assert!(!combining.rows[0].visuals.mesh.is_empty());
        assert_eq!(combining.rect, composed.rect);
        assert_eq!(
            combining.rows[0].visuals.mesh, composed.rows[0].visuals.mesh,
            "the accent must paint, not disappear or become a tofu glyph"
        );
    }

    #[test]
    fn missing_or_invalid_fallback_preserves_bundled_fonts() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.ttc");
        let invalid = directory.path().join("invalid.ttc");
        std::fs::write(&invalid, b"not a font").unwrap();
        let latin = directory.path().join("latin.ttf");
        let bundled = FontDefinitions::default();
        std::fs::write(&latin, bundled.font_data["Hack"].bytes()).unwrap();
        let fallbacks = load_cjk_fonts([missing, invalid, latin]);
        assert!(fallbacks.is_empty());
        let definitions = fonts_with_fallback(&fallbacks);
        assert_eq!(definitions, bundled);
        let mut fonts = egui::epaint::text::Fonts::new(Default::default(), definitions);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let id = FontId::new(13.0, family);
            assert!(fonts.has_glyphs(&id, "ASCII é"));
            // has_glyphs checks individual cmap entries, not shaped grapheme clusters.
            assert_combining_cluster_renders(&mut fonts.with_pixels_per_point(1.0), id.clone());
            assert!(!fonts.has_glyphs(&id, "中文"));
        }
    }

    #[test]
    fn system_fallback_renders_cjk_without_changing_ascii_or_cell_metrics() {
        let ctx = egui::Context::default();
        let mut baseline =
            egui::epaint::text::Fonts::new(Default::default(), FontDefinitions::default());
        let mut shared = Vec::new();
        install_fonts(&ctx);
        let mut output =
            ctx.run_ui(Default::default(), |ui| {
                ui.fonts_mut(|fonts| {
                let installed = fonts.definitions();
                let bundled = baseline.definitions();
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    assert!(installed.families[&family].starts_with(&bundled.families[&family]));
                }
                let available = installed.font_data.contains_key("system-cjk-0");
                #[cfg(target_os = "macos")]
                assert!(available, "macOS system CJK font must be exercised on this host");
                if !available {
                    eprintln!("CJK rendering NOT VERIFIED: no supported OS CJK font installed");
                    return;
                }
                shared = installed.font_data.iter()
                    .filter(|(name, _)| name.starts_with("system-cjk-"))
                    .map(|(name, data)| (name.clone(), Arc::clone(data)))
                    .collect();
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    let id = FontId::new(13.0, family);
                    assert!(fonts.has_glyphs(&id, "生产环境 漢字 日本語 かなカナ 한국어 한글 ASCII é"));
                    assert_combining_cluster_renders(fonts, id.clone());
                    let mut before = baseline.with_pixels_per_point(1.0);
                    assert_eq!(fonts.glyph_width(&id, 'M'), before.glyph_width(&id, 'M'));
                    assert_eq!(fonts.row_height(&id), before.row_height(&id));
                    let ascii =
                        fonts.layout_no_wrap("ASCII e\u{301}".into(), id.clone(), Color32::WHITE);
                    let original =
                        before.layout_no_wrap("ASCII e\u{301}".into(), id.clone(), Color32::WHITE);
                    assert_eq!(ascii.rect, original.rect);
                    for (first, second) in [("中", "文"), ("か", "な"), ("한", "글")] {
                        let first = fonts.layout_no_wrap(first.into(), id.clone(), Color32::WHITE);
                        let second = fonts.layout_no_wrap(second.into(), id.clone(), Color32::WHITE);
                        assert!(!first.rows[0].visuals.mesh.is_empty());
                        assert!(!second.rows[0].visuals.mesh.is_empty());
                        assert_ne!(
                            first.rows[0].visuals.mesh, second.rows[0].visuals.mesh,
                            "distinct Han, Kana and Hangul characters must not share the tofu bitmap"
                        );
                    }
                }
            });
            });
        output.textures_delta.clear();
        let second = egui::Context::default();
        install_fonts(&second);
        apply(&second, Mode::Light);
        apply(&second, Mode::Dark);
        let mut output = second.run_ui(Default::default(), |ui| {
            ui.fonts_mut(|fonts| {
                for (name, shared) in &shared {
                    assert!(Arc::ptr_eq(shared, &fonts.definitions().font_data[name]));
                }
            });
        });
        output.textures_delta.clear();
    }
}
