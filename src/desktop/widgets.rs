use crate::form::{FieldKind, Form};
use eframe::egui::{self, RichText};

/// Keeps multiline content inside its viewport without changing form or submission semantics.
pub(super) fn form_fields_sized(ui: &mut egui::Ui, form: &mut Form, editor_height: f32) -> bool {
    let mut changed = false;
    if let Some(note) = &form.note {
        ui.label(RichText::new(note).color(ui.visuals().weak_text_color()));
        ui.add_space(8.0);
    }
    for index in form.visible_here() {
        let field = &mut form.fields[index];
        ui.push_id(index, |ui| {
            let label = ui.label(RichText::new(field.label).strong());
            let id = ui.scope_id().with("value");
            match &field.kind {
                FieldKind::Text => { changed |= ui.add(egui::TextEdit::singleline(&mut field.value).id(id).desired_width(f32::INFINITY)).labelled_by(label.id).changed(); }
                FieldKind::Secret => {
                    let reveal_id = id.with("reveal");
                    let mut reveal = ui.data_mut(|d| d.get_temp::<bool>(reveal_id).unwrap_or(false));
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 72.0).max(80.0);
                        changed |= ui.add_sized([width, 26.0], egui::TextEdit::singleline(&mut field.value).id(id).password(!reveal)).labelled_by(label.id).changed();
                        if ui.selectable_label(reveal, if reveal { "Hide" } else { "Reveal" }).clicked() { reveal = !reveal; }
                    });
                    ui.data_mut(|d| d.insert_temp(reveal_id, reveal));
                }
                FieldKind::Bool => {
                    let mut value = field.value == "yes";
                    if ui.checkbox(&mut value, "Enabled").labelled_by(label.id).changed() { field.value = if value { "yes" } else { "no" }.into(); changed = true; }
                }
                FieldKind::Editor => {
                    let footer = egui::WidgetText::from(RichText::new(
                        "Tab moves focus. Empty content is saved as empty; Discard leaves the original unchanged.",
                    ).small()).into_galley(
                        ui,
                        Some(egui::TextWrapMode::Wrap),
                        ui.available_width(),
                        egui::TextStyle::Small,
                    );
                    // Reserve both widget gaps: scroll→help and help→the trailing field spacer.
                    let height = editor_height.min(
                        ui.available_height() - footer.size().y - 2.0 * ui.spacing().item_spacing.y - 8.0,
                    ).max(24.0);
                    egui::ScrollArea::vertical()
                        .id_salt(id.with("editor-scroll"))
                        .max_height(height)
                        .min_scrolled_height(24.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let height = ui.available_height().min(height);
                            let font = egui::FontSelection::from(egui::TextStyle::Monospace).resolve(ui.style());
                            let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font))
                                + ui.spacing().extra_text_line_spacing;
                            // min_size expands only the frame; desired_rows expands the text/caret viewport.
                            let rows = ((height - 2.0 * ui.spacing().button_padding.y) / line_height)
                                .floor().max(1.0) as usize;
                            changed |= ui.add(egui::TextEdit::multiline(&mut field.value).id(id).code_editor().lock_focus(false)
                                .desired_width(f32::INFINITY).desired_rows(rows).min_size(egui::vec2(0.0, height))).labelled_by(label.id).changed();
                        });
                    ui.add(egui::Label::new(footer));
                }
                FieldKind::Choice(options) => {
                    let search_id = id.with("search");
                    let mut search = ui.data_mut(|d| d.get_temp::<String>(search_id).unwrap_or_default());
                    egui::ComboBox::from_id_salt(id).width(ui.available_width())
                        .selected_text(if field.value.is_empty() { "Choose…" } else { &field.value }).show_ui(ui, |ui| {
                            if options.len() > 8 { ui.add(egui::TextEdit::singleline(&mut search).hint_text("Search choices")).labelled_by(label.id); }
                            if options.is_empty() { ui.label("No choices loaded. Refresh the source to retry."); }
                            let needle = search.to_lowercase();
                            for option in options.iter().filter(|o| needle.is_empty() || o.to_lowercase().contains(&needle)) {
                                changed |= ui.selectable_value(&mut field.value, option.clone(), if option.is_empty() { "(empty)" } else { option }).changed();
                            }
                        }).response.labelled_by(label.id);
                    ui.data_mut(|d| d.insert_temp(search_id, search));
                }
                FieldKind::Multi(None) => { ui.horizontal(|ui| { ui.spinner(); ui.label("Loading choices…"); }); }
                FieldKind::Multi(Some(options)) => {
                    let search_id = id.with("search");
                    let mut search = ui.data_mut(|d| d.get_temp::<String>(search_id).unwrap_or_default());
                    if options.len() > 8 { ui.add(egui::TextEdit::singleline(&mut search).hint_text("Search choices")).labelled_by(label.id); }
                    let mut selected = field.ticked();
                    let needle = search.to_lowercase();
                    if options.is_empty() { ui.label("No choices returned."); }
                    egui::ScrollArea::vertical().id_salt(id).max_height(190.0).show(ui, |ui| {
                        for option in options.iter().filter(|o| needle.is_empty() || o.to_lowercase().contains(&needle)) {
                            let mut checked = selected.contains(option);
                            if ui.checkbox(&mut checked, option).changed() {
                                if checked { selected.push(option.clone()); } else { selected.retain(|v| v != option); }
                                field.value = selected.join(", "); changed = true;
                            }
                        }
                    });
                    if !field.blank_means.is_empty() { ui.small(field.blank_means); }
                    ui.data_mut(|d| d.insert_temp(search_id, search));
                }
            }
            ui.add_space(8.0);
        });
    }
    if let Some(message) = &form.error {
        error(ui, message);
    }
    changed
}

/// Returns the original row index on click or keyboard activation; sorting never changes target indices.
pub(super) fn table(
    ui: &mut egui::Ui,
    id: &str,
    columns: &[&str],
    rows: &[Vec<String>],
    selected: &mut Option<usize>,
) -> Option<usize> {
    if columns.is_empty() {
        return None;
    }
    let id = ui.scope_id().with(id);
    let mut sort = ui.data_mut(|data| {
        data.get_temp::<Option<(usize, bool)>>(id.with("sort"))
            .flatten()
    });
    let width = (ui.available_width() / columns.len() as f32).max(96.0);
    let mut clicked = None;
    let mut activated = None;
    egui::ScrollArea::horizontal()
        .id_salt(id.with("x"))
        .show(ui, |ui| {
            ui.set_min_width(width * columns.len() as f32);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (column, title) in columns.iter().enumerate() {
                    let response = ui.add_sized(
                        [width, 28.0],
                        egui::Button::new(RichText::new(*title).strong()).frame(false),
                    );
                    if let Some((n, descending)) = sort {
                        if n == column {
                            let center = response.rect.right_center() - egui::vec2(9.0, 0.0);
                            let direction = if descending { 1.0 } else { -1.0 };
                            ui.painter().add(egui::Shape::convex_polygon(
                                vec![
                                    center + egui::vec2(-3.0, -2.0 * direction),
                                    center + egui::vec2(3.0, -2.0 * direction),
                                    center + egui::vec2(0.0, 3.0 * direction),
                                ],
                                ui.visuals().selection.stroke.color,
                                egui::Stroke::NONE,
                            ));
                        }
                    }
                    if response.clicked() {
                        sort = Some((column, matches!(sort, Some((n, false)) if n == column)));
                    }
                }
            });
            ui.separator();
            let order = sorted_rows(rows, sort);
            let available = ui.available_height();
            let height = if available.is_finite() {
                available - 100.0
            } else {
                ui.ctx().content_rect().height() - 440.0
            };
            egui::ScrollArea::vertical()
                .id_salt(id.with("y"))
                .max_height(height.max(28.0))
                .min_scrolled_height(28.0)
                .auto_shrink([false, true])
                .show_rows(ui, 28.0, order.len(), |ui, visible| {
                    for position in visible {
                        let index = order[position];
                        let row = &rows[index];
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width * columns.len() as f32, 28.0),
                            egui::Sense::hover(),
                        );
                        let response = ui.interact(rect, id.with(index), egui::Sense::click());
                        let is_selected = *selected == Some(index);
                        let fill = if is_selected {
                            ui.visuals().selection.bg_fill
                        } else if response.hovered() {
                            ui.visuals().widgets.hovered.bg_fill
                        } else if position % 2 == 1 {
                            ui.visuals().faint_bg_color
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, 0.0, fill);
                        if response.has_focus() {
                            ui.painter().rect_stroke(
                                rect.shrink(1.0),
                                2.0,
                                ui.visuals().selection.stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        for column in 0..columns.len() {
                            let cell = row.get(column).map(String::as_str).unwrap_or("");
                            let cell_rect = egui::Rect::from_min_size(
                                rect.min + egui::vec2(column as f32 * width + 8.0, 0.0),
                                egui::vec2(width - 16.0, 28.0),
                            );
                            ui.scope_builder(egui::UiBuilder::new().max_rect(cell_rect), |ui| {
                                ui.add(egui::Label::new(cell).truncate())
                                    .on_hover_text(cell);
                            });
                        }
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::Button,
                                ui.is_enabled(),
                                is_selected,
                                row.join(", "),
                            )
                        });
                        let enter = response.has_focus()
                            && ui.input(|input| {
                                input.key_pressed(egui::Key::Enter)
                                    || input.key_pressed(egui::Key::Space)
                            });
                        if response.clicked() || enter {
                            *selected = Some(index);
                            clicked = Some(index);
                            response.request_focus();
                        }
                        if response.double_clicked()
                            || (response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                        {
                            activated = Some(index);
                        }
                        if response.has_focus() {
                            let delta = ui.input_mut(|input| {
                                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                    1
                                } else if input
                                    .consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                                {
                                    -1
                                } else {
                                    0
                                }
                            });
                            if delta != 0 && !order.is_empty() {
                                let next = (position as isize + delta)
                                    .clamp(0, order.len() as isize - 1)
                                    as usize;
                                *selected = Some(order[next]);
                                clicked = Some(order[next]);
                                ui.memory_mut(|memory| memory.request_focus(id.with(order[next])));
                                ui.scroll_to_rect(
                                    rect.translate(egui::vec2(
                                        0.0,
                                        (next as f32 - position as f32)
                                            * (28.0 + ui.spacing().item_spacing.y),
                                    )),
                                    Some(egui::Align::Center),
                                );
                            }
                        }
                    }
                });
        });
    ui.data_mut(|data| data.insert_temp(id.with("sort"), sort));
    ui.data_mut(|data| data.insert_temp(id.with("activation"), activated));
    clicked
}

/// Activation is deliberately separate from selection; call immediately after `table` in the same UI scope.
pub(super) fn table_activation(ui: &egui::Ui, id: &str) -> Option<usize> {
    ui.data(|data| {
        data.get_temp::<Option<usize>>(ui.scope_id().with(id).with("activation"))
            .flatten()
    })
}

fn sorted_rows(rows: &[Vec<String>], sort: Option<(usize, bool)>) -> Vec<usize> {
    let mut order: Vec<_> = (0..rows.len()).collect();
    if let Some((column, descending)) = sort {
        order.sort_by(|&a, &b| {
            let a = rows[a].get(column).map(String::as_str).unwrap_or("");
            let b = rows[b].get(column).map(String::as_str).unwrap_or("");
            let cmp = match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.total_cmp(&b),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                _ => a.cmp(b),
            };
            if descending {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }
    order
}

pub(super) fn error(ui: &mut egui::Ui, message: &str) {
    egui::Frame::new()
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.10))
        .inner_margin(10.0)
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(message).color(ui.visuals().error_fg_color)).wrap(),
            );
        });
}
pub(super) fn confirm(
    ui: &mut egui::Ui,
    title: &str,
    target: &str,
    typed: Option<&str>,
    entry: &mut String,
) -> bool {
    ui.separator();
    ui.label(
        RichText::new(title)
            .strong()
            .color(ui.visuals().warn_fg_color),
    );
    ui.add(egui::Label::new(target).wrap());
    if let Some(expected) = typed {
        ui.label(format!("Type {expected} to confirm this target:"));
        ui.add(egui::TextEdit::singleline(entry).desired_width(f32::INFINITY));
    }
    let allowed = typed.is_none_or(|expected| entry.as_str() == expected);
    ui.add_enabled(
        allowed,
        egui::Button::new("Confirm").fill(ui.visuals().error_fg_color.gamma_multiply(0.20)),
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorting_preserves_resource_indices_and_orders_mixed_values_consistently() {
        let rows = vec![
            vec!["10".into()],
            vec!["2".into()],
            vec!["1x".into()],
            vec![],
        ];
        assert_eq!(sorted_rows(&rows, Some((0, false))), vec![1, 0, 3, 2]);
        assert_eq!(sorted_rows(&rows, Some((0, true))), vec![2, 3, 0, 1]);
        assert_eq!(sorted_rows(&rows, None), vec![0, 1, 2, 3]);
        assert_eq!(rows[sorted_rows(&rows, Some((0, false)))[0]][0], "2");
    }
}
