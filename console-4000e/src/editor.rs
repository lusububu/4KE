// egui editor for the 4000 E console, matching the fixed-size web plugin (page.tpl.html):
//   - The whole window background is one browser-rendered PNG (assets/backgrounds/console.png),
//     captured from the working HTML with every control hidden and the grain switched off, so the
//     panel washes are exactly what the browser draws. Grain is painted on top procedurally, the
//     same way the tape machine plugin does it.
//   - Every knob, button, label, LED and the fader sit at the coordinates measured from the HTML
//     layout (CSS px, scaled by knob::S), so they land on the panels baked into the PNG.
//
// Differences from the web page, because a plugin gets its audio from the host:
//   - No Play button, status line, source menu or Load file button. The Input panel's controls
//     move up into the space the source menu used.
//   - "Monitor" is the plugin's Output level (default +6 dB, which makes the line path unity gain).

use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};

use crate::dsp::{fader_db, mic_db, ratio_of};
use crate::knob::*;
use crate::mna::{alog, build_eq, build_hpf, build_lpf, ss_mag, to_ss};
use crate::{ConsoleParams, DynPosition, Meters};

const W_CSS: f32 = 1240.0;
const H_CSS: f32 = 699.14;

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size((W_CSS * S).round() as u32, (H_CSS * S).round() as u32)
}

const BG_PNG: &[u8] = include_bytes!("../assets/backgrounds/console.png");

const MORI_REGULAR: &str = "mori-regular";
const MORI_SEMIBOLD: &str = "mori-semibold";

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(MORI_REGULAR.to_owned(), egui::FontData::from_static(include_bytes!("../assets/fonts/PPMori-Regular.otf")).into());
    fonts.font_data.insert(MORI_SEMIBOLD.to_owned(), egui::FontData::from_static(include_bytes!("../assets/fonts/PPMori-Semibold.otf")).into());
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, MORI_SEMIBOLD.to_owned());
    fonts.families.insert(egui::FontFamily::Name(MORI_SEMIBOLD.into()), vec![MORI_SEMIBOLD.to_owned(), MORI_REGULAR.to_owned()]);
    ctx.set_fonts(fonts);
}
fn heading(px: f32) -> egui::FontId {
    egui::FontId::new(px * S, egui::FontFamily::Name(MORI_SEMIBOLD.into()))
}

fn load_png(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(BG_PNG).expect("embedded background PNG failed to decode").to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    ctx.load_texture("console-bg", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
}

// Grain: the same generator as the tape machine plugin (makeGrain() from the style guide, softened
// for a renderer without the browser's blur): black or white texels with alpha from the average of
// three uniform randoms, tiled every 160 points.
const GRAIN_LAYER_OPACITY: f32 = 0.10;
const GRAIN_JITTER: f32 = 55.0;
const GRAIN_SUPERSAMPLE: usize = 2;
fn make_grain(ctx: &egui::Context) -> egui::TextureHandle {
    const N: usize = 160 * GRAIN_SUPERSAMPLE;
    let mut image = egui::ColorImage::new([N, N], egui::Color32::TRANSPARENT);
    let mut seed: u32 = 0x2545_f491;
    let mut rand01 = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / 16_777_216.0
    };
    for px in image.pixels.iter_mut() {
        let v = 128.0 + ((rand01() + rand01() + rand01()) / 3.0 - 0.5) * GRAIN_JITTER;
        let base = if v > 128.0 { 255u8 } else { 0u8 };
        let alpha = ((v - 128.0).abs().clamp(0.0, 255.0) * GRAIN_LAYER_OPACITY) as u8;
        *px = egui::Color32::from_rgba_unmultiplied(base, base, base, alpha);
    }
    ctx.load_texture("console-grain", image, egui::TextureOptions::LINEAR)
}
fn paint_grain(painter: &egui::Painter, rect: egui::Rect, tex: &egui::TextureHandle) {
    const TILE: f32 = 160.0;
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let mut y = rect.top();
    while y < rect.bottom() {
        let mut x = rect.left();
        while x < rect.right() {
            painter.image(tex.id(), egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(TILE, TILE)), uv, egui::Color32::WHITE);
            x += TILE;
        }
        y += TILE;
    }
}

// ---------- value text, same formatting as the web page ----------
fn fmt_db(v: f64, d: usize) -> String {
    // round first so a value like -0.02 reads "+0.0 dB" rather than "-0.0 dB"
    let q = 10f64.powi(d as i32);
    let v = (v * q).round() / q;
    let v = if v == 0.0 { 0.0 } else { v };
    format!("{}{:.*} dB", if v >= 0.0 { "+" } else { "\u{2212}" }, d, v.abs())
}
fn f_hz(f: f64) -> String {
    if f >= 1000.0 {
        let s = format!("{:.*}", if f >= 10000.0 { 1 } else { 2 }, f / 1000.0);
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        format!("{s} kHz")
    } else {
        format!("{} Hz", f.round() as i64)
    }
}
fn x_r(r: f64, lo: f64, hi: f64) -> f64 {
    (r / lo).ln() / (hi / lo).ln()
}
fn mf_freq(u: f64, c: f64) -> f64 {
    let fa = 50e3 * alog(1.0 - u);
    let rs = ((3e3 + fa) * (3.6e3 + fa)).sqrt();
    let corr = 0.9226 + (1.004 - 0.9226) * x_r(rs, 3286.0, 53299.0);
    corr / (2.0 * std::f64::consts::PI * c * rs)
}
fn hf_freq(u: f64, bell: bool) -> f64 {
    let x = x_r(3e3 + 50e3 * alog(1.0 - u), 3e3, 53e3);
    if bell {
        18831.0 * (1301.0f64 / 18831.0).powf(x)
    } else {
        10000.0 * (1100.0f64 / 10000.0).powf(x)
    }
}
fn lf_freq(u: f64, bell: bool) -> f64 {
    let x = x_r(4.3e3 + 50e3 * alog(1.0 - u), 4.3e3, 54.3e3);
    if bell {
        420.0 * (37.0f64 / 420.0).powf(x)
    } else {
        500.0 * (42.0f64 / 500.0).powf(x)
    }
}
fn q_of(q: f64) -> f64 {
    let l = [2.85f64.ln(), 1.38f64.ln(), 0.5f64.ln()];
    if q <= 0.5 {
        (l[0] + (l[1] - l[0]) * q / 0.5).exp()
    } else {
        (l[1] + (l[2] - l[1]) * (q - 0.5) / 0.5).exp()
    }
}
fn hpf_freq(u: f64) -> f64 {
    1.0 / (2.0 * std::f64::consts::PI * 220e-9 * (1.8e3 + 47e3 * alog(1.0 - u)))
}
fn lpf_freq(u: f64, resonant: bool) -> f64 {
    let c43: f64 = if resonant { 68e-9 } else { 6.8e-9 };
    let r = 680.0 + 10e3 * u;
    (if resonant { 1.5 } else { 1.015 }) / (2.0 * std::f64::consts::PI * r * (c43 * 3.3e-9).sqrt())
}
fn rel_text(u: f64) -> String {
    let t = 0.1 * 40f64.powf(u);
    format!("{:.*} s", if u > 0.6 { 1 } else { 2 }, t)
}

// ---------- EQ curve, solved from the same netlists as the audio path ----------
const FS_UI: f64 = 96000.0;
const NPT: usize = 110;
#[derive(Clone, Copy, PartialEq)]
struct CurveKey {
    eq: crate::mna::EqSet,
    eq_on: bool,
    hpf: f64,
    lpf: f64,
    res: bool,
}
struct Curve {
    key: Option<CurveKey>,
    db: Vec<f64>,
    gains: [f64; 4],
}
fn freqs() -> impl Iterator<Item = f64> {
    (0..NPT).map(|k| 20.0 * 1000f64.powf(k as f64 / (NPT - 1) as f64))
}
fn compute_curve(k: &CurveKey) -> (Vec<f64>, [f64; 4]) {
    let eq = to_ss(&mut build_eq(&k.eq), FS_UI, "in", "v2");
    let hp = if k.hpf > 0.0 { Some(to_ss(&mut build_hpf(k.hpf), FS_UI, "in", "out")) } else { None };
    let lp = if k.lpf > 0.0 { Some(to_ss(&mut build_lpf(k.lpf, if k.res { 68e-9 } else { 6.8e-9 }), FS_UI, "in", "out")) } else { None };
    let db = freqs()
        .map(|f| {
            let mut m = if k.eq_on { ss_mag(&eq, f, FS_UI) } else { 1.0 };
            if let Some(s) = &hp {
                m *= ss_mag(s, f, FS_UI);
            }
            if let Some(s) = &lp {
                m *= ss_mag(s, f, FS_UI);
            }
            20.0 * m.log10()
        })
        .collect();
    let ga = |f: f64| if k.eq_on { 20.0 * ss_mag(&eq, f, FS_UI).log10() } else { 0.0 };
    let e = &k.eq;
    let gains = [
        if e.hf_bell { ga(hf_freq(e.hf_f, true)) } else { ga(20000.0) },
        ga(mf_freq(e.hmf_f, 4.7e-9)),
        ga(mf_freq(e.lmf_f, 22e-9)),
        if e.lf_bell { ga(lf_freq(e.lf_f, true)) } else { ga(20.0) },
    ];
    (db, gains)
}
fn draw_curve(ui: &egui::Ui, db: &[f64]) {
    let rect = r(442.0, 153.0, 356.0, 84.0);
    let pt = ui.painter();
    let (w, h) = (rect.width(), rect.height());
    let x = |f: f64| rect.left() + ((f / 20.0).ln() / 1000f64.ln()) as f32 * w;
    let y = |d: f64| rect.top() + h / 2.0 - (d / 20.0) as f32 * (h / 2.0 - 6.0 * S);
    let line = |a: f32| egui::Color32::from_rgba_unmultiplied(0x14, 0x14, 0x14, (a * 255.0) as u8);
    pt.line_segment([rect.left_top(), rect.right_top()], egui::Stroke::new(1.0_f32, line(0.35)));
    pt.line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0_f32, line(0.35)));
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let a = if f == 100.0 || f == 1000.0 || f == 10000.0 { 0.28 } else { 0.12 };
        pt.line_segment([egui::pos2(x(f), rect.top()), egui::pos2(x(f), rect.bottom())], egui::Stroke::new(1.0_f32, line(a)));
    }
    for d in [-15.0, 15.0] {
        pt.line_segment([egui::pos2(rect.left(), y(d)), egui::pos2(rect.right(), y(d))], egui::Stroke::new(1.0_f32, line(0.14)));
    }
    pt.line_segment([egui::pos2(rect.left(), y(0.0)), egui::pos2(rect.right(), y(0.0))], egui::Stroke::new(1.0_f32, line(0.4)));
    let tc = line(0.7);
    pt.text(egui::pos2(x(100.0) + 3.0, rect.bottom() - 4.0), egui::Align2::LEFT_BOTTOM, "100", mono(9.0), tc);
    pt.text(egui::pos2(x(1000.0) + 3.0, rect.bottom() - 4.0), egui::Align2::LEFT_BOTTOM, "1k", mono(9.0), tc);
    pt.text(egui::pos2(x(10000.0) + 3.0, rect.bottom() - 4.0), egui::Align2::LEFT_BOTTOM, "10k", mono(9.0), tc);
    pt.text(egui::pos2(rect.left() + 3.0, y(15.0) + 2.0), egui::Align2::LEFT_TOP, "+15", mono(9.0), tc);
    pt.text(egui::pos2(rect.left() + 3.0, y(-15.0) - 2.0), egui::Align2::LEFT_BOTTOM, "\u{2212}15", mono(9.0), tc);
    let pts: Vec<egui::Pos2> = freqs().zip(db.iter()).map(|(f, d)| egui::pos2(x(f), y(d.clamp(-21.0, 21.0)))).collect();
    pt.add(egui::Shape::line(pts, egui::Stroke::new(2.0_f32, INK)));
}

// ---------- fader (CSS rect 826,219 64x150; drawn in the SVG's 64x168 units) ----------
const FT: [(f64, &str); 9] = [(10.0, "+10"), (5.0, "+5"), (0.0, "0"), (-5.0, "5"), (-10.0, "10"), (-20.0, "20"), (-30.0, "30"), (-40.0, "40"), (-60.0, "60")];
fn db_to_pos(db: f64) -> f64 {
    let p = [0.0, 0.05, 0.15, 0.3, 0.45, 0.6, 0.75, 0.87, 1.0];
    let d = [-120.0, -60.0, -40.0, -25.0, -15.0, -7.0, 0.0, 5.0, 10.0];
    for i in 1..p.len() {
        if db <= d[i] {
            return p[i - 1] + (db - d[i - 1]) / (d[i] - d[i - 1]) * (p[i] - p[i - 1]);
        }
    }
    1.0
}
fn fader(ui: &mut egui::Ui, param: &FloatParam, setter: &ParamSetter) {
    let (ox, oy, sy) = (826.0f32, 219.0f32, 150.0f32 / 168.0);
    let vy = |v: f32| oy + (158.0 - v * 148.0) * sy; // viewBox: top 10, bottom 158
    let rect = r(ox, oy, 64.0, 150.0);
    let id = ui.id().with("fader");
    let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
    if resp.drag_started() {
        setter.begin_set_parameter(param);
        if let Some(pos) = resp.interact_pointer_pos() {
            let yc = vy(param.value()) * S;
            if (pos.y - yc).abs() > 10.0 * S {
                let v = ((oy + 158.0 * sy) - pos.y / S) / (148.0 * sy);
                setter.set_parameter(param, v.clamp(0.0, 1.0));
            }
        }
    }
    if resp.dragged() {
        let v = (param.value() - resp.drag_delta().y / (148.0 * sy * S)).clamp(0.0, 1.0);
        setter.set_parameter(param, v);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
    }
    if resp.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, 0.75);
        setter.end_set_parameter(param);
    }
    let pt = ui.painter();
    let track = egui::Color32::from_rgba_unmultiplied(0xe8, 0xe6, 0xe1, 89);
    pt.rect_filled(r(ox + 20.0, vy(1.0), 4.0, 148.0 * sy), 0.0, track);
    for (db, lab) in FT {
        let yy = vy(db_to_pos(db) as f32);
        pt.line_segment([p(ox + 27.0, yy), p(ox + 33.0, yy)], egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(0xe8, 0xe6, 0xe1, 153)));
        pt.text(p(ox + 36.0, yy), egui::Align2::LEFT_CENTER, lab, mono(8.5 * sy), PAPER);
    }
    let yc = vy(param.value());
    pt.rect_filled(r(ox + 7.0, yc - 8.0 * sy, 30.0, 16.0 * sy), 2.0, PAPER);
    pt.line_segment([p(ox + 9.0, yc), p(ox + 35.0, yc)], egui::Stroke::new(2.0_f32, INK));
    let db = fader_db(param.value() as f64);
    let txt = if db < -100.0 { "\u{2212}\u{221e} dB".to_string() } else { fmt_db(db, 1) };
    pt.text(p(858.0, 379.0), egui::Align2::CENTER_CENTER, "Fader", body(11.2), PAPER);
    pt.text(p(858.0, 395.5), egui::Align2::CENTER_CENTER, txt, mono(10.56), PAPER);
}

pub fn create(params: Arc<ConsoleParams>, editor_state: Arc<EguiState>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let bg: Mutex<Option<egui::TextureHandle>> = Mutex::new(None);
    let grain: Mutex<Option<egui::TextureHandle>> = Mutex::new(None);
    let curve = Mutex::new(Curve { key: None, db: vec![0.0; NPT], gains: [0.0; 4] });

    create_egui_editor(
        editor_state,
        (),
        |ctx, _| install_fonts(ctx),
        move |ctx, setter, _| {
            ctx.request_repaint();
            let bg_tex = bg.lock().unwrap().get_or_insert_with(|| load_png(ctx)).clone();
            let grain_tex = grain.lock().unwrap().get_or_insert_with(|| make_grain(ctx)).clone();

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0xc4, 0xc4, 0xc2))).show(ctx, |ui| {
                let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W_CSS * S, H_CSS * S));
                ui.painter().image(bg_tex.id(), full, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);

                let pr = &params;
                let st = pr.settings();
                let m = meters.load();
                let dark = Tone::Dark;
                let light = Tone::Light;
                let accent = Tone::Accent;

                // ---------- header ----------
                ui.painter().text(p(28.0, 39.2), egui::Align2::LEFT_TOP, "4000 E", heading(33.6), INK);
                knob(ui, KnobSpec { x: 166.2, y: 22.0, label: "Output", value: fmt_db(st.out_db, 0), tone: light, bipolar: false }, &pr.output, setter);
                let dbfs = m.bus_l_db.max(m.bus_r_db) - 24.0 + st.out_db;
                led_row(ui, 227.7, 44.5, &["\u{2212}24", "\u{2212}18", "\u{2212}12", "\u{2212}6", "\u{2212}3", "0"], &[-24.0, -18.0, -12.0, -6.0, -3.0, 0.0], dbfs, 5, LedGround::Light, INK);

                // ---------- input ----------
                label(ui, 24.0, 118.0, "Input", 11.2, PAPER);
                if button(ui, [24.0, 141.0, 46.8, 25.0], "Line", !st.mic, accent, "line") && st.mic {
                    setter.begin_set_parameter(&pr.mic);
                    setter.set_parameter(&pr.mic, false);
                    setter.end_set_parameter(&pr.mic);
                }
                if button(ui, [70.8, 141.0, 38.6, 25.0], "Mic", st.mic, accent, "micb") && !st.mic {
                    setter.begin_set_parameter(&pr.mic);
                    setter.set_parameter(&pr.mic, true);
                    setter.end_set_parameter(&pr.mic);
                }
                bool_button(ui, [24.0, 172.1, 68.4, 25.0], "Pad \u{2212}20", &pr.pad, accent, setter);
                bool_button(ui, [98.4, 172.1, 54.0, 25.0], "Phase", &pr.phase, accent, setter);
                ui.scope(|ui| {
                    if !st.mic {
                        ui.set_opacity(0.42);
                    }
                    knob(ui, KnobSpec { x: 27.9, y: 207.1, label: "Mic gain", value: fmt_db(mic_db(st.mic_gain, st.pad), 0), tone: accent, bipolar: false }, &pr.mic_gain, setter);
                });
                ui.scope(|ui| {
                    if st.mic {
                        ui.set_opacity(0.42);
                    }
                    knob(ui, KnobSpec { x: 84.3, y: 207.1, label: "Line trim", value: fmt_db(-20.0 + 40.0 * st.trim, 1), tone: accent, bipolar: true }, &pr.trim, setter);
                });
                label(ui, 24.0, 285.1, "Level", 10.56, PAPER);
                let lv = ["\u{2212}20", "\u{2212}10", "0", "+4", "+10", "+16", "+20"];
                let lt = [-20.0, -10.0, 0.0, 4.0, 10.0, 16.0, 20.0];
                led_row(ui, 26.5, 302.1, &lv, &lt, m.input_db, 5, LedGround::Dark, PAPER);

                // ---------- dynamics ----------
                label(ui, 193.0, 118.0, "Dynamics", 11.2, PAPER);
                let dp = pr.dyn_pos.value();
                for (rect, txt, val) in [([193.0, 141.0, 39.6, 25.0], "Off", DynPosition::Off), ([232.6, 141.0, 81.8, 25.0], "Before EQ", DynPosition::Before), ([314.4, 141.0, 74.6, 25.0], "After EQ", DynPosition::After)] {
                    if button(ui, rect, txt, dp == val, dark, txt) && dp != val {
                        setter.begin_set_parameter(&pr.dyn_pos);
                        setter.set_parameter(&pr.dyn_pos, val);
                        setter.end_set_parameter(&pr.dyn_pos);
                    }
                }
                let d = st.dynamics;
                label(ui, 193.0, 176.0, "Compressor", 10.56, PAPER);
                let rt = ratio_of(d.ratio);
                let ratio_txt = if rt > 1e8 { "\u{221e} : 1".to_string() } else if rt < 10.0 { format!("{rt:.1} : 1") } else { format!("{} : 1", rt.round()) };
                knob(ui, KnobSpec { x: 196.2, y: 198.0, label: "Ratio", value: ratio_txt, tone: dark, bipolar: false }, &pr.ratio, setter);
                knob(ui, KnobSpec { x: 255.8, y: 198.0, label: "Threshold", value: fmt_db(10.0 - 30.0 * d.thr, 0), tone: dark, bipolar: false }, &pr.thr, setter);
                knob(ui, KnobSpec { x: 315.2, y: 198.0, label: "Release", value: rel_text(d.rel), tone: dark, bipolar: false }, &pr.rel, setter);
                bool_button(ui, [193.0, 276.0, 97.2, 25.0], "Fast attack", &pr.fast, dark, setter);
                label(ui, 193.0, 311.1, "Turned down, dB", 10.56, PAPER);
                led_row(ui, 195.5, 328.1, &["3", "6", "10", "14", "20"], &[3.0, 6.0, 10.0, 14.0, 20.0], m.gr, 3, LedGround::Dark, PAPER);
                label(ui, 193.0, 363.1, "Gate", 10.56, PAPER);
                knob(ui, KnobSpec { x: 196.0, y: 385.1, label: "Range", value: format!("{:.0} dB", 40.0 * d.grange), tone: dark, bipolar: false }, &pr.grange, setter);
                knob(ui, KnobSpec { x: 255.4, y: 385.1, label: "Threshold", value: fmt_db(10.0 - 40.0 * d.gthr, 0), tone: dark, bipolar: false }, &pr.gthr, setter);
                knob(ui, KnobSpec { x: 314.8, y: 385.1, label: "Release", value: rel_text(d.grel), tone: dark, bipolar: false }, &pr.grel, setter);
                bool_button(ui, [193.0, 463.1, 82.8, 25.0], "Hard gate", &pr.gate, dark, setter);
                bool_button(ui, [281.8, 463.1, 97.2, 25.0], "Fast attack", &pr.gfast, dark, setter);
                label(ui, 193.0, 498.1, "Turned down, dB", 10.56, PAPER);
                led_row(ui, 195.5, 515.1, &["3", "6", "10", "20", "40"], &[3.0, 6.0, 10.0, 20.0, 40.0], m.gate, 3, LedGround::Dark, PAPER);

                // ---------- equaliser ----------
                label(ui, 442.0, 124.0, "Equaliser", 11.2, INK);
                bool_button(ui, [744.0, 118.0, 54.0, 25.0], "EQ in", &pr.eq_on, light, setter);
                let key = CurveKey { eq: st.eq, eq_on: st.eq_on, hpf: st.hpf, lpf: st.lpf, res: st.lpf_resonant };
                let (db, gains) = {
                    let mut c = curve.lock().unwrap();
                    if c.key != Some(key) {
                        let (db, g) = compute_curve(&key);
                        c.db = db;
                        c.gains = g;
                        c.key = Some(key);
                    }
                    (c.db.clone(), c.gains)
                };
                draw_curve(ui, &db);
                let e = st.eq;
                for (yy, name) in [(275.0, "Highs"), (349.0, "High mids"), (423.0, "Low mids"), (497.0, "Lows")] {
                    label(ui, 442.0, yy, name, 10.56, INK);
                }
                knob(ui, KnobSpec { x: 525.2, y: 247.0, label: "Gain", value: fmt_db(gains[0], 1), tone: light, bipolar: true }, &pr.hf_g, setter);
                knob(ui, KnobSpec { x: 586.0, y: 247.0, label: "Freq", value: format!("\u{2248}{}", f_hz(hf_freq(e.hf_f, e.hf_bell))), tone: light, bipolar: false }, &pr.hf_f, setter);
                bool_button(ui, [643.6, 290.0, 46.8, 25.0], "Bell", &pr.hf_bell, light, setter);
                knob(ui, KnobSpec { x: 525.2, y: 321.0, label: "Gain", value: fmt_db(gains[1], 1), tone: light, bipolar: true }, &pr.hmf_g, setter);
                knob(ui, KnobSpec { x: 586.0, y: 321.0, label: "Freq", value: format!("\u{2248}{}", f_hz(mf_freq(e.hmf_f, 4.7e-9))), tone: light, bipolar: false }, &pr.hmf_f, setter);
                knob(ui, KnobSpec { x: 646.6, y: 321.0, label: "Q", value: format!("\u{2248}{:.2}", q_of(e.hmf_q)), tone: light, bipolar: false }, &pr.hmf_q, setter);
                knob(ui, KnobSpec { x: 525.2, y: 395.0, label: "Gain", value: fmt_db(gains[2], 1), tone: light, bipolar: true }, &pr.lmf_g, setter);
                knob(ui, KnobSpec { x: 586.0, y: 395.0, label: "Freq", value: format!("\u{2248}{}", f_hz(mf_freq(e.lmf_f, 22e-9))), tone: light, bipolar: false }, &pr.lmf_f, setter);
                knob(ui, KnobSpec { x: 646.6, y: 395.0, label: "Q", value: format!("\u{2248}{:.2}", q_of(e.lmf_q)), tone: light, bipolar: false }, &pr.lmf_q, setter);
                knob(ui, KnobSpec { x: 525.2, y: 469.0, label: "Gain", value: fmt_db(gains[3], 1), tone: light, bipolar: true }, &pr.lf_g, setter);
                knob(ui, KnobSpec { x: 586.0, y: 469.0, label: "Freq", value: format!("\u{2248}{}", f_hz(lf_freq(e.lf_f, e.lf_bell))), tone: light, bipolar: false }, &pr.lf_f, setter);
                bool_button(ui, [643.6, 512.0, 46.8, 25.0], "Bell", &pr.lf_bell, light, setter);

                // ---------- output ----------
                label(ui, 826.0, 118.0, "Output", 11.2, PAPER);
                let hp_txt = if st.hpf <= 0.0 { "Out".to_string() } else { format!("\u{2248}{}", f_hz(hpf_freq(st.hpf))) };
                let lp_txt = if st.lpf <= 0.0 { "Out".to_string() } else { format!("\u{2248}{}", f_hz(lpf_freq(st.lpf, st.lpf_resonant))) };
                knob(ui, KnobSpec { x: 835.1, y: 141.0, label: "High-pass", value: hp_txt, tone: dark, bipolar: false }, &pr.hpf, setter);
                knob(ui, KnobSpec { x: 899.8, y: 141.0, label: "Low-pass", value: lp_txt, tone: dark, bipolar: false }, &pr.lpf, setter);
                fader(ui, &pr.fader, setter);
                for (i, (lab, thr)) in lv.iter().zip(lt).enumerate() {
                    let yy = 391.0 - 15.0 * i as f32;
                    led(ui, 912.0, yy, m.out_db >= thr, i >= 5, LedGround::Dark);
                    ui.painter().text(p(953.1, yy + 5.5), egui::Align2::RIGHT_CENTER, *lab, mono(9.28), PAPER);
                }
                let pan_txt = if (st.pan - 0.5).abs() < 0.005 { "Centre".to_string() } else { format!("{} {}", if st.pan < 0.5 { "L" } else { "R" }, ((st.pan - 0.5).abs() * 200.0).round()) };
                knob(ui, KnobSpec { x: 829.0, y: 418.0, label: "Pan", value: pan_txt, tone: dark, bipolar: true }, &pr.pan, setter);
                bool_button(ui, [826.0, 496.0, 46.8, 25.0], "Mute", &pr.mute, dark, setter);

                // ---------- fine-tuning ----------
                label(ui, 24.0, 564.1, "Fine-tuning", 11.2, INK);
                label(ui, 24.0, 587.1, "Bell width, per band", 10.56, INK);
                let pct = |v: &FloatParam| format!("{}%", (v.value() * 100.0).round());
                knob(ui, KnobSpec { x: 27.0, y: 607.1, label: "Highs", value: pct(&pr.rv1), tone: light, bipolar: false }, &pr.rv1, setter);
                knob(ui, KnobSpec { x: 86.0, y: 607.1, label: "High mids", value: pct(&pr.rv2), tone: light, bipolar: false }, &pr.rv2, setter);
                knob(ui, KnobSpec { x: 148.7, y: 607.1, label: "Low mids", value: pct(&pr.rv3), tone: light, bipolar: false }, &pr.rv3, setter);
                knob(ui, KnobSpec { x: 206.4, y: 607.1, label: "Lows", value: pct(&pr.rv4), tone: light, bipolar: false }, &pr.rv4, setter);
                label(ui, 275.4, 630.1, "Low-pass filter", 10.56, INK);
                let res = st.lpf_resonant;
                for (rect, txt, val) in [([275.4, 650.1, 61.2, 25.0], "Smooth", false), ([336.6, 650.1, 74.6, 25.0], "Resonant", true)] {
                    if button(ui, rect, txt, res == val, light, txt) && res != val {
                        setter.begin_set_parameter(&pr.lpf_resonant);
                        setter.set_parameter(&pr.lpf_resonant, val);
                        setter.end_set_parameter(&pr.lpf_resonant);
                    }
                }
                label(ui, 439.2, 630.1, "Background hiss", 10.56, INK);
                bool_button(ui, [439.2, 650.1, 68.4, 25.0], "Hiss on", &pr.hiss, light, setter);

                // ---------- mix bus ----------
                ui.painter().text(p(997.0, 120.0), egui::Align2::LEFT_TOP, "Bus compressor", heading(21.6), PAPER);
                bool_button(ui, [997.0, 151.6, 111.6, 25.0], "Compressor on", &pr.bus_on, dark, setter);
                let b = st.bus;
                let thr = -15.0 + 30.0 * b.thr;
                knob(ui, KnobSpec { x: 1005.4, y: 192.6, label: "Threshold", value: format!("{}{:.1}", if thr >= 0.0 { "+" } else { "\u{2212}" }, thr.abs()), tone: dark, bipolar: false }, &pr.bus_thr, setter);
                knob(ui, KnobSpec { x: 1067.0, y: 192.6, label: "Make-up", value: format!("+{:.1} dB", 15.0 * b.mk), tone: dark, bipolar: false }, &pr.bus_mk, setter);
                knob(ui, KnobSpec { x: 1123.3, y: 192.6, label: "Ratio", value: format!("{} : 1", ["2", "4", "10"][b.ratio.min(2)]), tone: dark, bipolar: false }, &pr.bus_ratio, setter);
                knob(ui, KnobSpec { x: 1000.0, y: 270.6, label: "Attack", value: format!("{} ms", ["0.1", "0.3", "1", "3", "10", "30"][b.att.min(5)]), tone: dark, bipolar: false }, &pr.bus_att, setter);
                knob(ui, KnobSpec { x: 1054.0, y: 270.6, label: "Release", value: ["0.1 s", "0.3 s", "0.6 s", "1.2 s", "Auto"][b.rel.min(4)].to_string(), tone: dark, bipolar: false }, &pr.bus_rel, setter);
                label(ui, 997.0, 354.6, "Turned down, dB", 10.56, PAPER);
                led_row(ui, 999.5, 371.6, &["1", "2", "3", "4", "6", "8", "10", "14", "20"], &[1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 14.0, 20.0], m.bus_gr, 6, LedGround::Dark, PAPER);
                label(ui, 997.0, 404.6, "Output level", 10.56, PAPER);
                ui.painter().text(p(997.0, 427.1), egui::Align2::LEFT_CENTER, "L", mono(9.92), PAPER);
                ui.painter().text(p(997.0, 444.1), egui::Align2::LEFT_CENTER, "R", mono(9.92), PAPER);
                led_row(ui, 1013.5, 421.6, &["", "", "", "", "", "", ""], &lt, m.bus_l_db, 5, LedGround::Dark, PAPER);
                led_row(ui, 1013.5, 438.6, &lv, &lt, m.bus_r_db, 5, LedGround::Dark, PAPER);

                paint_grain(ui.painter(), full, &grain_tex);
            });
        },
    )
}
