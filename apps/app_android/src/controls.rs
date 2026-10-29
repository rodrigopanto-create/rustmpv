#[cfg(any(target_os = "android", test))]
use eframe::egui;
use std::time::{Duration, Instant};

pub const HIDE_AFTER: Duration = Duration::from_secs(3);
pub const CONTROL_SIZE: f32 = 40.0;

pub fn update_visibility(
    last_interaction: &mut Instant,
    show_controls: &mut bool,
    now: Instant,
    playing: bool,
    interacting: bool,
) {
    if interacting || !playing {
        *last_interaction = now;
        *show_controls = true;
    } else if now.duration_since(*last_interaction) > HIDE_AFTER {
        *show_controls = false;
    }
}

#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Fullscreen,
    Restore,
}

#[cfg(any(target_os = "android", test))]
pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("").min_size(egui::Vec2::splat(CONTROL_SIZE)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let center = response.rect.center();
    let color = ui.style().interact(&response).fg_stroke.color;
    let painter = ui.painter();
    match icon {
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + egui::vec2(-6.0, -9.0),
                    center + egui::vec2(9.0, 0.0),
                    center + egui::vec2(-6.0, 9.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for x in [-5.0, 5.0] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        center + egui::vec2(x, 0.0),
                        egui::vec2(5.0, 18.0),
                    ),
                    0.0,
                    color,
                );
            }
        }
        Icon::Stop => {
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(16.0, 16.0)),
                0.0,
                color,
            );
        }
        Icon::Fullscreen | Icon::Restore => {
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    let (corner, step) = if matches!(icon, Icon::Fullscreen) {
                        (center + egui::vec2(x * 9.0, y * 9.0), -1.0)
                    } else {
                        (center + egui::vec2(x * 3.0, y * 3.0), 1.0)
                    };
                    painter.line_segment(
                        [corner, corner + egui::vec2(x * step * 6.0, 0.0)],
                        (2.0, color),
                    );
                    painter.line_segment(
                        [corner, corner + egui::vec2(0.0, y * step * 6.0)],
                        (2.0, color),
                    );
                }
            }
        }
    }
    response.on_hover_text(label)
}

#[cfg(any(target_os = "android", test))]
pub fn seek(
    ui: &mut egui::Ui,
    position: &mut f64,
    duration: Option<f64>,
    timer: &str,
    enabled: bool,
) -> egui::Response {
    let timer = ui.painter().layout_no_wrap(
        timer.to_owned(),
        egui::FontId::monospace(12.0),
        ui.visuals().text_color(),
    );
    let width = (ui.available_width() - timer.size().x - ui.spacing().item_spacing.x).max(0.0);
    // In egui 0.29 add_sized alone does not expand the slider's track.
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = width;
            ui.add_enabled_ui(enabled, |ui| {
                ui.add_sized(
                    [width, 20.0],
                    egui::Slider::new(position, 0.0..=duration.unwrap_or(1.0)).show_value(false),
                )
            })
            .inner
        })
        .inner
        .on_hover_text("Buscar posicion");
    ui.add(egui::Label::new(timer).selectable(false));
    response
}

#[cfg(target_os = "android")]
pub fn loading_bar(ui: &mut egui::Ui) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            "Carga en curso, duracion desconocida",
        )
    });
    if ui.is_rect_visible(rect) {
        let phase = ui.input(|input| (input.time * 2.5).sin() as f32 * 0.5 + 0.5);
        let width = rect.width() * 0.25;
        let left = rect.left() + phase * (rect.width() - width);
        let segment = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(width, rect.height()),
        );
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        ui.painter()
            .rect_filled(segment, 3.0, ui.visuals().selection.bg_fill);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

pub fn clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_only_after_three_seconds_of_playback_inactivity() {
        let start = Instant::now();
        let mut last = start;
        let mut visible = true;
        update_visibility(&mut last, &mut visible, start + HIDE_AFTER, true, false);
        assert!(visible);
        update_visibility(
            &mut last,
            &mut visible,
            start + HIDE_AFTER + Duration::from_millis(1),
            true,
            false,
        );
        assert!(!visible);
        for playing in [true, false] {
            update_visibility(
                &mut last,
                &mut visible,
                start + Duration::from_secs(10),
                playing,
                playing,
            );
            assert!(visible);
            assert_eq!(last, start + Duration::from_secs(10));
        }
    }

    #[test]
    fn held_pointer_keeps_controls_visible_and_inactivity_restarts_on_resume() {
        let start = Instant::now();
        let mut last = start;
        let mut visible = true;
        for seconds in 1..=10 {
            update_visibility(
                &mut last,
                &mut visible,
                start + Duration::from_secs(seconds),
                true,
                true,
            );
            assert!(visible);
        }
        let paused_at = start + Duration::from_secs(30);
        update_visibility(&mut last, &mut visible, paused_at, false, false);
        update_visibility(
            &mut last,
            &mut visible,
            paused_at + Duration::from_secs(2),
            true,
            false,
        );
        assert!(visible);
    }

    #[test]
    fn controls_share_a_single_row_and_seek_expands_without_overflow() {
        for width in [320.0, 360.0, 800.0] {
            for (duration, timer) in [
                (Some(120.0), "0:10 / 2:00"),
                (Some(7200.0), "1:01:01 / 2:00:00"),
                (None, "0:00 / --:--"),
            ] {
                let context = egui::Context::default();
                let mut slider_width = 0.0;
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 480.0),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::TopBottomPanel::bottom("controls_panel")
                            .exact_height(CONTROL_SIZE + 12.0)
                            .frame(
                                egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 6.0)),
                            )
                            .show(context, |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let right = ui.max_rect().right();
                                let row = ui.horizontal_centered(|ui| {
                                    let mut buttons = Vec::new();
                                    for icon in [
                                        Icon::Play,
                                        Icon::Pause,
                                        Icon::Stop,
                                        if width > 400.0 {
                                            Icon::Restore
                                        } else {
                                            Icon::Fullscreen
                                        },
                                    ] {
                                        buttons.push(button(ui, icon, "Control", true).rect);
                                    }
                                    let mut position = 10.0;
                                    let slider = seek(
                                        ui,
                                        &mut position,
                                        duration,
                                        timer,
                                        duration.is_some(),
                                    );
                                    slider_width = slider.rect.width();
                                    for rect in buttons {
                                        assert_eq!(rect.size(), egui::Vec2::splat(CONTROL_SIZE));
                                        assert!(
                                            (rect.center().y - slider.rect.center().y).abs() < 1.0
                                        );
                                    }
                                    assert!(ui.available_width() < 1.0);
                                });
                                assert!(row.response.rect.right() <= right + 1.0);
                            });
                    },
                );
                assert!(slider_width > 0.0);
                if width == 800.0 {
                    assert!(slider_width > 400.0);
                }
            }
        }
    }

    #[test]
    fn formats_media_times() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(79.9), "1:19");
        assert_eq!(clock(3661.0), "1:01:01");
        assert_eq!(clock(f64::NAN), "0:00");
        assert_eq!(clock(-1.0), "0:00");
    }
}
