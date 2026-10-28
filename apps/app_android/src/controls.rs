#[cfg(target_os = "android")]
use eframe::egui;

#[cfg(target_os = "android")]
#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Fullscreen,
    Restore,
}

#[cfg(target_os = "android")]
pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("").min_size(egui::vec2(44.0, 40.0)),
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
    use super::clock;

    #[test]
    fn formats_media_times() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(79.9), "1:19");
        assert_eq!(clock(3661.0), "1:01:01");
        assert_eq!(clock(f64::NAN), "0:00");
        assert_eq!(clock(-1.0), "0:00");
    }
}
