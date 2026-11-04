use eframe::egui::{self, Color32, RichText, Stroke};
use std::collections::VecDeque;

pub const BACKGROUND: Color32 = Color32::from_rgb(6, 12, 17);
pub const PANEL: Color32 = Color32::from_rgb(12, 23, 29);
pub const BORDER: Color32 = Color32::from_rgb(37, 71, 77);
pub const TEXT: Color32 = Color32::from_rgb(213, 233, 227);
pub const MUTED: Color32 = Color32::from_rgb(139, 171, 167);
pub const GREEN: Color32 = Color32::from_rgb(141, 245, 176);
pub const CYAN: Color32 = Color32::from_rgb(104, 221, 233);
pub const WARNING: Color32 = Color32::from_rgb(249, 203, 121);
pub const ERROR: Color32 = Color32::from_rgb(255, 145, 149);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Source,
    Console,
    Access,
    Player,
}

pub fn active_stage(
    source_focused: bool,
    access_busy: bool,
    player_active: bool,
    process_pending: bool,
    account_mode: bool,
) -> Stage {
    if source_focused {
        Stage::Source
    } else if access_busy {
        Stage::Access
    } else if player_active {
        Stage::Player
    } else if process_pending {
        if account_mode {
            Stage::Access
        } else {
            Stage::Console
        }
    } else {
        Stage::Source
    }
}

fn border_color(time: f64, active: bool) -> Color32 {
    if !active {
        return BORDER;
    }
    let amount = 0.55 + 0.25 * (time * std::f64::consts::TAU / 2.4).sin();
    let mix = |base: u8, accent: u8| {
        (f64::from(base) + (f64::from(accent) - f64::from(base)) * amount).round() as u8
    };
    Color32::from_rgb(
        mix(BORDER.r(), CYAN.r()),
        mix(BORDER.g(), CYAN.g()),
        mix(BORDER.b(), CYAN.b()),
    )
}

pub fn flow_panel(ui: &egui::Ui, active: bool) -> egui::Frame {
    if active {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    panel().stroke(Stroke::new(
        1.0_f32,
        border_color(ui.input(|input| input.time), active),
    ))
}

pub fn configure(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = PANEL;
    style.visuals.code_bg_color = BACKGROUND;
    style.visuals.warn_fg_color = WARNING;
    style.visuals.error_fg_color = ERROR;
    style.visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.selection.bg_fill = Color32::from_rgb(28, 76, 69);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, GREEN);
    style.visuals.hyperlink_color = CYAN;
    style.visuals.slider_trailing_fill = true;
    style.visuals.text_cursor.stroke = Stroke::new(1.5_f32, GREEN);
    style.visuals.text_cursor.blink = false;
    for (widget, fill, border, text) in [
        (
            &mut style.visuals.widgets.noninteractive,
            PANEL,
            BORDER,
            TEXT,
        ),
        (&mut style.visuals.widgets.inactive, PANEL, BORDER, MUTED),
        (
            &mut style.visuals.widgets.hovered,
            Color32::from_rgb(22, 46, 49),
            CYAN,
            TEXT,
        ),
        (
            &mut style.visuals.widgets.active,
            Color32::from_rgb(28, 76, 69),
            GREEN,
            GREEN,
        ),
        (
            &mut style.visuals.widgets.open,
            Color32::from_rgb(18, 38, 42),
            CYAN,
            CYAN,
        ),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, border);
        widget.fg_stroke = Stroke::new(1.0_f32, text);
        widget.rounding = egui::Rounding::same(4.0);
    }
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::monospace(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    context.set_style(style);
}

pub fn panel() -> egui::Frame {
    egui::Frame::none()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .rounding(6.0)
        .inner_margin(12.0)
}

pub fn section(ui: &mut egui::Ui, code: &str, title: &str) {
    ui.label(
        RichText::new(format!("{code} / {title}"))
            .monospace()
            .size(11.0)
            .color(CYAN),
    );
}

pub fn header(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("[VR]")
                .monospace()
                .size(23.0)
                .strong()
                .color(GREEN),
        );
        ui.vertical(|ui| {
            ui.label(
                RichText::new("RUSTMPV")
                    .monospace()
                    .size(21.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                RichText::new("TERMINAL MULTIMEDIA // ANDROID")
                    .monospace()
                    .size(9.0)
                    .color(MUTED),
            );
        });
    });
    let (line, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
    ui.painter().rect_filled(line, 0.0, BORDER);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(line.min, egui::vec2(line.width() * 0.22, line.height())),
        0.0,
        GREEN,
    );
    ui.add_space(2.0);
}

pub fn source_input(ui: &mut egui::Ui, input: &mut String) -> egui::Response {
    let width = (ui.available_width() - 70.0 - ui.spacing().item_spacing.x).max(1.0);
    ui.add_sized(
        [width, 34.0],
        egui::TextEdit::singleline(input)
            .id(egui::Id::new("terminal_source"))
            .font(egui::TextStyle::Monospace)
            .hint_text("https://..."),
    )
}

pub fn message_color(message: &str) -> Color32 {
    let lower = message.to_lowercase();
    if [
        "no se pudo",
        "fallo",
        "error",
        "rechaz",
        "no fue aceptada",
        "no es valida",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        ERROR
    } else if ["aviso", "omitido", "cancelad"]
        .iter()
        .any(|word| lower.contains(word))
    {
        WARNING
    } else {
        GREEN
    }
}

pub fn console(ui: &mut egui::Ui, messages: &VecDeque<String>, pending: bool, active: bool) {
    flow_panel(ui, active).fill(BACKGROUND).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        section(ui, "03", "CONSOLA DEL PROCESO");
        ui.label(
            RichText::new(if pending {
                "> worker / ejecutando"
            } else {
                "> worker / en espera"
            })
            .monospace()
            .size(11.0)
            .color(MUTED),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("terminal_output")
            .max_height(136.0)
            .auto_shrink([false, true])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if messages.is_empty() {
                    ui.label(
                        RichText::new("Sin eventos. Introduce un enlace para comenzar.")
                            .monospace()
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                for (index, message) in messages.iter().enumerate() {
                    ui.label(
                        RichText::new(format!("{:02} > {message}", index + 1))
                            .monospace()
                            .size(12.0)
                            .color(message_color(message)),
                    );
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_follows_work_without_delaying_playback_for_the_queue() {
        assert_eq!(
            active_stage(false, false, false, false, false),
            Stage::Source
        );
        assert_eq!(
            active_stage(false, false, false, true, false),
            Stage::Console
        );
        assert_eq!(active_stage(false, false, false, true, true), Stage::Access);
        assert_eq!(
            active_stage(false, true, false, false, false),
            Stage::Access
        );
        assert_eq!(active_stage(false, false, true, true, true), Stage::Player);
        assert_eq!(
            active_stage(false, false, true, false, false),
            Stage::Player
        );
        assert_eq!(active_stage(true, true, true, true, true), Stage::Source);
    }

    #[test]
    fn border_pulses_slowly_without_disappearing_or_changing_geometry() {
        assert_eq!(border_color(0.0, false), BORDER);
        assert_eq!(border_color(0.6, false), BORDER);
        assert_ne!(border_color(0.6, true), border_color(1.8, true));
        assert_eq!(border_color(0.0, true), border_color(2.4, true));
        for time in [0.0, 0.6, 1.2, 1.8, 2.4] {
            let color = border_color(time, true);
            assert!(color.g() > BORDER.g());
            assert!(color.g() < CYAN.g());
        }
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let active = flow_panel(ui, true);
                let idle = flow_panel(ui, false);
                assert_eq!(active.inner_margin, idle.inner_margin);
                assert_eq!(active.stroke.width, idle.stroke.width);
            });
        });
    }

    #[test]
    fn terminal_sections_fit_small_and_wide_screens_with_long_output() {
        for width in [320.0, 360.0, 520.0, 800.0] {
            let context = egui::Context::default();
            configure(&context);
            let mut input = "https://youtube.com/watch?v=abcdefghijk&list=RDabcdefghijk".to_owned();
            let messages = VecDeque::from([
                "Preparando resolucion; las comprobaciones de JavaScript pueden tardar.".to_owned(),
                "No se pudo resolver el enlace. El proceso ha terminado.".repeat(4),
            ]);
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 800.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        let right = ui.max_rect().right();
                        header(ui);
                        let source = panel().show(ui, |ui| {
                            section(ui, "01", "FUENTE");
                            ui.horizontal(|ui| {
                                source_input(ui, &mut input);
                                assert!(
                                    ui.add_sized([70.0, 34.0], egui::Button::new("PEGAR"))
                                        .rect
                                        .right()
                                        <= right
                                );
                            });
                        });
                        assert!(source.response.rect.right() <= right + 1.0);
                        console(ui, &messages, true, true);
                        assert!(ui.min_rect().right() <= right + 1.0);
                    });
                },
            );
        }
    }

    #[test]
    fn terminal_errors_and_warnings_remain_distinct() {
        assert_eq!(message_color("No se pudo resolver el enlace"), ERROR);
        assert_eq!(message_color("La sesion de YouTube no fue aceptada"), ERROR);
        assert_eq!(message_color("El sitio dio un aviso"), WARNING);
        assert_eq!(
            message_color("Resolucion cancelada por el usuario"),
            WARNING
        );
        assert_eq!(
            message_color("Enlace resuelto; cargando en el player."),
            GREEN
        );
    }

    #[test]
    fn terminal_text_palette_meets_normal_text_contrast() {
        fn luminance(color: Color32) -> f64 {
            let linear = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        }
        for background in [BACKGROUND, PANEL] {
            for foreground in [TEXT, MUTED, GREEN, CYAN, WARNING, ERROR] {
                let ratio = (luminance(foreground) + 0.05) / (luminance(background) + 0.05);
                assert!(ratio >= 4.5, "{foreground:?} on {background:?}: {ratio}");
            }
        }
    }
}
