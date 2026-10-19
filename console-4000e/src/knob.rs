// Hand-painted widgets matching the web console exactly: the SVG knob (270 degree sweep, orange
// value arc, solid disc and pointer), the flat toggle buttons, the round LEDs and the VCA fader.
// Every position is given in the web page's CSS pixels and scaled by S, so the editor can place
// widgets at the coordinates measured from the HTML layout.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

pub const S: f32 = 1.25; // CSS px -> egui points
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x55, 0x19); // --c2
pub const ACCENT_HOT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x52);
pub const INK: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const PAPER: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe1);

pub fn p(x: f32, y: f32) -> Pos2 {
    Pos2::new(x * S, y * S)
}
pub fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(p(x, y), Vec2::new(w * S, h * S))
}
pub fn mono(px: f32) -> FontId {
    FontId::monospace(px * S)
}
pub fn body(px: f32) -> FontId {
    FontId::proportional(px * S)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Dark,  // on a dark or saturated panel: paper disc, ink pointer, paper text
    Light, // on a pale panel: ink disc, paper pointer, ink text
    Accent, // on the orange Input panel: like Dark, but an active button turns ink so it stays visible
}
impl Tone {
    pub fn text(self) -> Color32 {
        match self {
            Tone::Dark | Tone::Accent => PAPER,
            Tone::Light => INK,
        }
    }
}

fn arc_point(c: Pos2, rad: f32, deg: f32) -> Pos2 {
    let a = (deg - 90.0).to_radians();
    c + Vec2::new(a.cos(), a.sin()) * rad
}
fn arc(c: Pos2, rad: f32, a1: f32, a2: f32) -> Vec<Pos2> {
    let n = (((a2 - a1).abs() / 6.0).ceil() as usize).max(2);
    (0..=n).map(|i| arc_point(c, rad, a1 + (a2 - a1) * i as f32 / n as f32)).collect()
}

pub struct KnobSpec<'a> {
    pub x: f32, // svg top-left in CSS px
    pub y: f32,
    pub label: &'a str,
    pub value: String,
    pub tone: Tone,
    pub bipolar: bool,
}

/// A knob bound to any nih-plug parameter (FloatParam or stepped IntParam). Drag up or down,
/// scroll, or double-click to reset.
pub fn knob<P: Param>(ui: &mut egui::Ui, k: KnobSpec, param: &P, setter: &ParamSetter) {
    let size = 38.0 * S;
    let rect = Rect::from_min_size(p(k.x, k.y), Vec2::splat(size));
    let id = ui.id().with(param.name());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let steps = param.step_count();
    if resp.drag_started() {
        setter.begin_set_parameter(param);
        ui.memory_mut(|m| m.data.insert_temp(id, param.unmodulated_normalized_value()));
    }
    if resp.dragged() {
        // keep an unsnapped "virtual" position so stepped knobs still move with small drags
        let cur: f32 = ui.memory(|m| m.data.get_temp(id)).unwrap_or(param.unmodulated_normalized_value());
        let v = (cur - resp.drag_delta().y / (150.0 * S)).clamp(0.0, 1.0);
        ui.memory_mut(|m| m.data.insert_temp(id, v));
        setter.set_parameter_normalized(param, v);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
        ui.memory_mut(|m| m.data.remove::<f32>(id));
    }
    if resp.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, param.default_normalized_value());
        setter.end_set_parameter(param);
    }
    if resp.hovered() {
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let step = match steps {
                Some(n) if n > 0 => 1.0 / n as f32,
                _ => 0.03,
            };
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, (param.unmodulated_normalized_value() + step * scroll.signum()).clamp(0.0, 1.0));
            setter.end_set_parameter(param);
        }
    }

    let v = param.unmodulated_normalized_value().clamp(0.0, 1.0);
    let c = rect.center();
    let sc = size / 56.0;
    let (track, disc, pointer) = match k.tone {
        Tone::Dark | Tone::Accent => (Color32::from_rgba_unmultiplied(0xe8, 0xe6, 0xe1, 89), PAPER, INK),
        Tone::Light => (Color32::from_rgba_unmultiplied(0x14, 0x14, 0x14, 64), INK, PAPER),
    };
    let pt = ui.painter();
    let a0 = -135.0;
    let av = a0 + 270.0 * v;
    pt.add(egui::Shape::line(arc(c, 22.0 * sc, a0, 135.0), Stroke::new(5.0 * sc, track)));
    // on the orange Input panel the value arc is drawn in ink, otherwise it disappears into the panel
    let val_col = if k.tone == Tone::Accent { INK } else { ACCENT };
    if k.bipolar {
        if (v - 0.5).abs() > 0.004 {
            pt.add(egui::Shape::line(arc(c, 22.0 * sc, 0.0f32.min(av), 0.0f32.max(av)), Stroke::new(5.0 * sc, val_col)));
        }
    } else if v > 0.01 {
        pt.add(egui::Shape::line(arc(c, 22.0 * sc, a0, av), Stroke::new(5.0 * sc, val_col)));
    }
    pt.circle_filled(c, 15.0 * sc, disc);
    pt.line_segment([c, arc_point(c, 15.0 * sc, av)], Stroke::new(2.5 * sc, pointer));
    let col = k.tone.text();
    pt.text(Pos2::new(c.x, (k.y + 40.0 + 7.0) * S), egui::Align2::CENTER_CENTER, k.label, body(11.2), col);
    pt.text(Pos2::new(c.x, (k.y + 56.0 + 6.0) * S), egui::Align2::CENTER_CENTER, k.value, mono(10.56), col);
}

/// Flat toggle / segment button. Returns true when clicked.
pub fn button(ui: &mut egui::Ui, rect_css: [f32; 4], text: &str, active: bool, tone: Tone, id: &str) -> bool {
    let rect = r(rect_css[0], rect_css[1], rect_css[2], rect_css[3]);
    let resp = ui.interact(rect, ui.id().with(id), Sense::click());
    let pt = ui.painter();
    let fg = tone.text();
    if active && tone == Tone::Accent {
        pt.rect_filled(rect, 0.0, INK);
        pt.text(rect.center(), egui::Align2::CENTER_CENTER, text.to_uppercase(), mono(10.88), PAPER);
    } else if active {
        pt.rect_filled(rect, 0.0, ACCENT);
        pt.text(rect.center(), egui::Align2::CENTER_CENTER, text.to_uppercase(), mono(10.88), INK);
    } else {
        pt.rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0_f32, fg.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        pt.text(rect.center(), egui::Align2::CENTER_CENTER, text.to_uppercase(), mono(10.88), fg);
    }
    resp.clicked()
}

pub fn bool_button(ui: &mut egui::Ui, rect_css: [f32; 4], text: &str, param: &BoolParam, tone: Tone, setter: &ParamSetter) {
    let on = param.value();
    if button(ui, rect_css, text, on, tone, param.name()) {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, !on);
        setter.end_set_parameter(param);
    }
}

#[derive(Clone, Copy)]
pub enum LedGround {
    Dark,
    Light,
}

/// One round LED, 11 px, flat. `hot` gives the brighter top-of-scale colour.
pub fn led(ui: &egui::Ui, x: f32, y: f32, on: bool, hot: bool, ground: LedGround) {
    let rect = r(x, y, 11.0, 11.0);
    let pt = ui.painter();
    let c = rect.center();
    let rad = 5.5 * S;
    if on {
        // the web LED is a gradient from #ff8a52 to #c93c0c; the dark rim keeps it readable on orange
        pt.circle_filled(c, rad, if hot { ACCENT_HOT } else { ACCENT });
        pt.circle_stroke(c, rad - 0.5, Stroke::new(1.0_f32, Color32::from_rgb(0xc9, 0x3c, 0x0c)));
    } else {
        let (fill, edge) = match ground {
            LedGround::Dark => (Color32::from_rgba_unmultiplied(0xe8, 0xe6, 0xe1, 36), Color32::from_rgba_unmultiplied(0xe8, 0xe6, 0xe1, 56)),
            LedGround::Light => (Color32::from_rgba_unmultiplied(0x14, 0x14, 0x14, 31), Color32::from_rgba_unmultiplied(0x14, 0x14, 0x14, 77)),
        };
        pt.circle_filled(c, rad, fill);
        pt.circle_stroke(c, rad - 0.5, Stroke::new(1.0_f32, edge));
    }
}

/// A horizontal LED meter: LEDs at a fixed 17 px pitch with labels centred underneath.
#[allow(clippy::too_many_arguments)]
pub fn led_row(ui: &egui::Ui, x: f32, y: f32, labels: &[&str], thresholds: &[f64], value: f64, hot_from: usize, ground: LedGround, text: Color32) {
    for (i, (lab, thr)) in labels.iter().zip(thresholds).enumerate() {
        let lx = x + 17.0 * i as f32;
        led(ui, lx, y, value >= *thr, i >= hot_from, ground);
        if !lab.is_empty() {
            ui.painter().text(p(lx + 5.5, y + 18.0), egui::Align2::CENTER_CENTER, *lab, mono(8.32), text);
        }
    }
}

pub fn label(ui: &egui::Ui, x: f32, y: f32, text: &str, px: f32, color: Color32) {
    ui.painter().text(p(x, y), egui::Align2::LEFT_TOP, text.to_uppercase(), mono(px), color);
}
