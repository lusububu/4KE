// Parity checks against the figures the web app's engine produced (and the service/operator
// manuals they were checked against). These run in GitHub Actions before every build.

use crate::dsp::*;
use crate::mna::*;

const FS: f64 = 96000.0;

fn sweep(e: &EqSet) -> Vec<(f64, f64)> {
    let ss = to_ss(&mut build_eq(e), FS, "in", "v2");
    (0..400).map(|i| 20.0 * 1000f64.powf(i as f64 / 399.0)).map(|f| (f, 20.0 * ss_mag(&ss, f, FS).log10())).collect()
}
fn peak(c: &[(f64, f64)]) -> (f64, f64) {
    c.iter().cloned().fold((0.0, 0.0), |a, b| if b.1.abs() > a.1.abs() { b } else { a })
}

#[test]
fn eq_flat_when_centred() {
    let c = sweep(&EqSet::default());
    let worst = c.iter().map(|p| p.1.abs()).fold(0.0, f64::max);
    assert!(worst < 0.35, "flat EQ deviates {worst} dB");
}

#[test]
fn eq_bands_reach_15_db_both_ways() {
    for band in 0..4 {
        for g in [1.0, -1.0] {
            let mut e = EqSet::default();
            match band {
                0 => e.hf_g = g,
                1 => e.hmf_g = g,
                2 => e.lmf_g = g,
                _ => e.lf_g = g,
            }
            let (_, db) = peak(&sweep(&e));
            assert!((db.abs() - 15.3).abs() < 1.4 && db.signum() == g, "band {band} gain {g}: {db}");
        }
    }
}

#[test]
fn mid_band_q_follows_the_panel() {
    // Q at -3 dB from the peak: 2.85 / 1.38 / 0.50 from the solved circuit (panel 3 / 1.5 / 0.5)
    for (q, want) in [(0.0, 2.85), (0.5, 1.38), (1.0, 0.50)] {
        let e = EqSet { hmf_g: 1.0, hmf_q: q, ..EqSet::default() };
        let c = sweep(&e);
        let (fc, pk) = peak(&c);
        let lev = pk - 3.0;
        let lo = c.iter().filter(|p| p.0 < fc && p.1 < lev).map(|p| p.0).fold(0.0, f64::max);
        let hi = c.iter().filter(|p| p.0 > fc && p.1 < lev).map(|p| p.0).fold(f64::MAX, f64::min);
        let qm = fc / (hi - lo);
        assert!((qm - want).abs() / want < 0.12, "q {q}: {qm} vs {want}");
        assert!((pk - 15.1).abs() < 1.0, "q {q} peak {pk}");
    }
}

#[test]
fn hmf_frequency_range() {
    let lo = peak(&sweep(&EqSet { hmf_g: 1.0, hmf_f: 0.0, ..EqSet::default() })).0;
    let hi = peak(&sweep(&EqSet { hmf_g: 1.0, hmf_f: 1.0, ..EqSet::default() })).0;
    assert!((lo - 638.0).abs() < 60.0, "HMF low end {lo}");
    assert!((hi - 9490.0).abs() < 900.0, "HMF high end {hi}");
}

#[test]
fn high_pass_is_18_db_per_octave() {
    let ss = to_ss(&mut build_hpf(1.0), FS, "in", "out");
    let d = |f: f64| 20.0 * ss_mag(&ss, f, FS).log10();
    let corner = (100..600).map(|f| f as f64).find(|&f| d(f) > -3.0).unwrap();
    assert!((corner - 392.0).abs() < 25.0, "HPF corner {corner}");
    let slope = d(100.0) - d(50.0);
    assert!((slope - 18.0).abs() < 1.5, "HPF slope {slope}");
}

#[test]
fn low_pass_bottoms_out_near_3k2() {
    let ss = to_ss(&mut build_lpf(1.0, 6.8e-9), FS, "in", "out");
    let d = |f: f64| 20.0 * ss_mag(&ss, f, FS).log10();
    let corner = (1000..8000).step_by(10).map(|f| f as f64).find(|&f| d(f) < -3.0).unwrap();
    assert!((corner - 3194.0).abs() < 150.0, "LPF corner {corner}");
}

#[test]
fn mic_gain_range() {
    assert!((mic_db(0.0, false) - 20.0).abs() < 1.5);
    assert!((mic_db(1.0, false) - 69.5).abs() < 1.5);
    assert!((mic_db(0.0, false) - mic_db(0.0, true) - 19.8).abs() < 0.1);
}

fn base_settings() -> Settings {
    Settings {
        mic: false,
        mic_gain: 0.5,
        pad: false,
        phase: false,
        trim: 0.5,
        dyn_pos: DynPos::Off,
        dynamics: DynSet { ratio: 0.0, thr: 1.0 / 3.0, rel: 0.3, fast: false, grange: 0.0, gthr: 0.25, grel: 0.3, gate: false, gfast: false },
        eq_on: true,
        eq: EqSet::default(),
        hpf: 0.0,
        lpf: 0.0,
        lpf_resonant: false,
        fader: 0.75,
        pan: 0.5,
        mute: false,
        hiss: false,
        bus: BusSet { on: false, thr: 0.5, mk: 0.0, att: 2, rel: 4, ratio: 1 },
        out_db: 6.0,
    }
}

fn run_sine(c: &mut Console, amp: f32, f: f64, sr: f64, secs: f64) -> f64 {
    let n = (sr * secs) as usize;
    let mut l: Vec<f32> = (0..n).map(|i| amp * (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32).collect();
    let mut r = l.clone();
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        c.process(a, b);
    }
    let tail = &l[n / 2..];
    (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt() * std::f64::consts::SQRT_2
}

#[test]
fn line_path_is_unity_at_default_settings() {
    let mut c = Console::new(48000.0);
    c.apply(&base_settings());
    let out = run_sine(&mut c, 0.1, 1000.0, 48000.0, 0.5);
    let db = 20.0 * (out / 0.1).log10();
    assert!(db.abs() < 0.5, "line unity {db} dB");
}

#[test]
fn compressor_fast_attack_about_3_ms() {
    let fs = 96000.0;
    let mut d = Dynamics::new(fs);
    d.set(DynSet { ratio: 1.0, thr: 1.0 / 3.0, rel: 0.0, fast: true, grange: 0.0, gthr: 0.25, grel: 0.3, gate: false, gfast: false });
    let lo = DBU * 10f64.powf(-10.0 / 20.0) * std::f64::consts::SQRT_2;
    let hi = DBU * 10f64.powf(20.0 / 20.0) * std::f64::consts::SQRT_2;
    for i in 0..(fs as usize) {
        d.tick(lo * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin());
    }
    let mut tr = Vec::new();
    for i in 0..(fs as usize / 2) {
        d.tick(hi * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin());
        tr.push(d.gr_c);
    }
    let fin = *tr.last().unwrap();
    let t90 = tr.iter().position(|&v| v >= 0.9 * fin).unwrap() as f64 / fs * 1000.0;
    assert!((2.0..4.5).contains(&t90), "fast attack 90% at {t90} ms");
}

#[test]
fn bus_attack_tracks_the_panel_at_4_to_1() {
    let fs = 96000.0;
    for (att, want_ms) in [(0usize, 0.21), (2, 1.74), (5, 28.9)] {
        let mut b = BusComp::new(fs);
        b.set(BusSet { on: true, thr: 0.5, mk: 0.0, att, rel: 0, ratio: 1 });
        let lo = DBU * std::f64::consts::SQRT_2;
        let hi = DBU * 10.0 * std::f64::consts::SQRT_2;
        for i in 0..(fs as usize) {
            let x = lo * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin();
            b.tick(x, x);
        }
        let mut tr = Vec::new();
        for i in 0..(fs as usize) {
            let x = hi * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin();
            b.tick(x, x);
            tr.push(b.gr);
        }
        let fin = *tr.last().unwrap();
        let t = tr.iter().position(|&v| v >= 0.632 * fin).unwrap() as f64 / fs * 1000.0;
        assert!((t - want_ms).abs() / want_ms < 0.2, "attack {att}: {t} ms vs {want_ms}");
    }
}

#[test]
fn random_settings_stay_finite() {
    let mut c = Console::new(48000.0);
    let mut rng = Rng::new(7);
    let mut u = || (rng.gauss() * 0.3 + 0.5).clamp(0.0, 1.0);
    let mut buf_l = vec![0.0f32; 256];
    let mut buf_r = vec![0.0f32; 256];
    for it in 0..150 {
        let mut s = base_settings();
        s.mic = it % 2 == 0;
        s.pad = it % 3 == 0;
        s.mic_gain = u();
        s.trim = u();
        s.dyn_pos = [DynPos::Off, DynPos::PreEq, DynPos::PostEq][it % 3];
        s.dynamics = DynSet { ratio: u(), thr: u(), rel: u(), fast: it % 2 == 1, grange: u(), gthr: u(), grel: u(), gate: it % 4 == 0, gfast: it % 5 == 0 };
        s.eq = EqSet { hf_g: u() * 2.0 - 1.0, hf_f: u(), hf_bell: it % 2 == 0, hmf_g: u() * 2.0 - 1.0, hmf_f: u(), hmf_q: u(), lmf_g: u() * 2.0 - 1.0, lmf_f: u(), lmf_q: u(), lf_g: u() * 2.0 - 1.0, lf_f: u(), lf_bell: it % 3 == 0, rv1: 10e3 * u(), rv2: 500.0 * u(), rv3: 500.0 * u(), rv4: 5e3 * u() };
        s.hpf = u();
        s.lpf = u();
        s.lpf_resonant = it % 2 == 0;
        s.fader = u();
        s.pan = u();
        s.hiss = true;
        s.bus = BusSet { on: it % 3 != 0, thr: u(), mk: u(), att: it % 6, rel: it % 5, ratio: it % 3 };
        c.apply(&s);
        for _ in 0..20 {
            for i in 0..256 {
                buf_l[i] = (u() as f32 - 0.5) * 1.6;
                buf_r[i] = (u() as f32 - 0.5) * 1.6;
            }
            c.process(&mut buf_l, &mut buf_r);
            assert!(buf_l.iter().chain(buf_r.iter()).all(|v| v.is_finite()), "non-finite output at iteration {it}");
        }
    }
}
