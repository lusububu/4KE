// The SSL 4000 E channel and mix bus, ported from the web app's dsp.js. Circuits come from the
// SSL 4000 G service manual's E-era drawings: channel amp T82241-71, EQ 82E242 (T82242-70),
// dynamics 82E10 (T82010-71), VCA 82E13 (T82013-71), pan T0611E-72, and the bus compressor
// 82E26 (T82026-71) with 82E27 (T82027.71.3). Values marked ASSUMED are not in the manuals; see
// console/ssl-4000e-circuit-notes.md in the project for the full list.
//
// Differences from the web app, because a plugin processes the host's stereo track instead of
// one internal mono source:
//   - Two identical channel paths (left and right) share every setting. The dynamics are linked:
//     one detector listens to the RMS of both sides and one gain is applied to both, so the stereo
//     image never shifts.
//   - Pan works as a balance control normalised to unity at centre, rather than the mono desk's
//     -4 dB centre pan law (the law's shape from R5/R6 6K81 is kept).

use crate::mna::{alog, build_eq, build_hpf, build_lpf, to_ss, EqSet, SsRun};

pub const DBU: f64 = 0.7746; // volts rms at 0 dBu
pub const VCLIP: f64 = 16.0; // NE5534 output swing on +/-18.5 V rails (ASSUMED 2.5 V headroom)

#[inline]
pub fn clip(x: f64, v: f64) -> f64 {
    let a = x.abs() / v;
    if a < 0.4 {
        return x;
    }
    let r = a * a;
    let r2 = r * r;
    let r4 = r2 * r2;
    let r8 = r4 * r4;
    x / (1.0 + r8).powf(0.0625)
}

/// Small, allocation-free noise source (xorshift), roughly unit-variance Gaussian.
pub struct Rng(u32);
impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed.max(1))
    }
    #[inline]
    fn u(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as f64 / 4294967296.0
    }
    #[inline]
    pub fn gauss(&mut self) -> f64 {
        (self.u() + self.u() + self.u() + self.u() - 2.0) * 1.7320508
    }
}

// ---------------- halfband 2x resampler, Kaiser windowed ----------------
const HB_M: usize = 12;
fn bessel0(x: f64) -> f64 {
    let (mut s, mut t) = (1.0, 1.0);
    for k in 1..30 {
        t *= (x / (2.0 * k as f64)).powi(2);
        s += t;
    }
    s
}
fn hb_coefs() -> [f64; HB_M] {
    let beta = 7.5;
    let c = (2 * HB_M) as f64;
    let mut h = [0.0; HB_M];
    for j in 1..=HB_M {
        let k = (2 * j - 1) as f64;
        let ideal = (std::f64::consts::PI * k / 2.0).sin() / (std::f64::consts::PI * k);
        let r = k / c;
        h[j - 1] = ideal * bessel0(beta * (1.0 - r * r).sqrt()) / bessel0(beta);
    }
    h
}

#[derive(Clone)]
pub struct Up2 {
    hb: [f64; HB_M],
    buf: [f64; 64],
    i: usize,
}
impl Up2 {
    pub fn new() -> Self {
        Up2 { hb: hb_coefs(), buf: [0.0; 64], i: 0 }
    }
    #[inline]
    pub fn push(&mut self, x: f64) -> (f64, f64) {
        self.i = (self.i + 1) & 63;
        self.buf[self.i] = x;
        let i = self.i + 64;
        let e = self.buf[(i - HB_M) & 63];
        let mut s = 0.0;
        for j in 1..=HB_M {
            s += self.hb[j - 1] * (self.buf[(i - HB_M + j) & 63] + self.buf[(i - HB_M - j + 1) & 63]);
        }
        (e, 2.0 * s)
    }
}
#[derive(Clone)]
pub struct Down2 {
    hb: [f64; HB_M],
    e: [f64; 64],
    o: [f64; 64],
    i: usize,
}
impl Down2 {
    pub fn new() -> Self {
        Down2 { hb: hb_coefs(), e: [0.0; 64], o: [0.0; 64], i: 0 }
    }
    #[inline]
    pub fn push(&mut self, ze: f64, zo: f64) -> f64 {
        self.i = (self.i + 1) & 63;
        self.e[self.i] = ze;
        let n = self.i + 64;
        let mut s = 0.5 * self.e[(n - HB_M) & 63];
        for j in 1..=HB_M {
            s += self.hb[j - 1] * (self.o[(n - HB_M + j - 1) & 63] + self.o[(n - HB_M - j) & 63]);
        }
        self.o[self.i] = zo;
        s
    }
}

// ---------------- channel amplifier T82241-71 (mic) ----------------
#[derive(Clone)]
pub struct MicAmp {
    dt: f64,
    lam: f64,
    vp: f64,
    n: f64,
    l0: f64,
    lam_s: f64,
    rw: f64,
    rl: f64,
}
impl MicAmp {
    pub fn new(fs4: f64) -> Self {
        let n = 10.0; // ASSUMED 1:10 step-up (gives +20..+70 dB)
        MicAmp { dt: 1.0 / fs4, lam: 0.0, vp: 0.0, n, l0: 5.0, lam_s: 0.0223, rw: 40.0, rl: 150e3 / (n * n) }
    }
    #[inline]
    fn im(&self, l: f64) -> f64 {
        let x = l / self.lam_s;
        let x2 = x * x;
        (l / self.l0) * (1.0 + x2 * x2)
    }
    #[inline]
    fn dim(&self, l: f64) -> f64 {
        let x = l / self.lam_s;
        let x2 = x * x;
        (1.0 + 5.0 * x2 * x2) / self.l0
    }
    /// vth: Thevenin source volts (after phantom feeds / pad), rth its resistance; returns secondary volts
    #[inline]
    pub fn tick(&mut self, vth: f64, rth: f64) -> f64 {
        let rt = rth + self.rw;
        let h = self.dt / 2.0;
        let mut v = self.vp;
        for _ in 0..3 {
            let l = self.lam + h * (v + self.vp);
            let g = (vth - v) / rt - self.im(l) - v / self.rl;
            let dg = -1.0 / rt - self.dim(l) * h - 1.0 / self.rl;
            v -= g / dg;
        }
        self.lam += h * (v + self.vp);
        self.vp = v;
        v * self.n
    }
}

/// Gain law from the drawing: T1 non-inverting, T9 inverting, one 10K lin pot shared.
pub fn mic_gains(rot: f64) -> (f64, f64) {
    let p = rot.clamp(0.001, 0.999);
    let g1 = 1.0 + 10e3 * p / (619.0 * 10e3 / (619.0 + 10e3));
    let g9 = 10.7e3 / (619.0 + 10e3 * (1.0 - p));
    (g1, g9)
}

/// Mic channel gain in dB as shown on the panel (transformer step-up and loading included).
pub fn mic_db(rot: f64, pad: bool) -> f64 {
    let (g1, g9) = mic_gains(rot);
    20.0 * (g1 * g9).log10() + 18.9 + if pad { -19.8 } else { 0.0 }
}

// ---------------- 82E10 dynamics (T82010-71) ----------------
const RATIO_PTS: [f64; 7] = [1.0, 2.0, 3.0, 5.0, 8.0, 20.0, 1e9]; // panel scale, operator manual p10
pub fn ratio_of(rot: f64) -> f64 {
    let x = rot.clamp(0.0, 1.0) * 6.0;
    let i = (x.floor() as usize).min(5);
    let f = x - i as f64;
    let (a, b) = (RATIO_PTS[i], RATIO_PTS[i + 1]);
    if b > 1e8 {
        return if f > 0.999 { 1e9 } else { a / (1.0 - f) };
    }
    (a.ln() * (1.0 - f) + b.ln() * f).exp()
}

#[derive(Clone, Copy, PartialEq)]
pub struct DynSet {
    pub ratio: f64,
    pub thr: f64,
    pub rel: f64,
    pub fast: bool,
    pub grange: f64,
    pub gthr: f64,
    pub grel: f64,
    pub gate: bool,
    pub gfast: bool,
}

pub struct Dynamics {
    dt: f64,
    tavg: f64,
    ms: f64,
    vh: f64,
    vm: f64,
    pub gr_g: f64,
    pub gr_c: f64,
    open: bool,
    p: DynSet,
    r: f64,
    slope: f64,
    t: f64,
    ra: f64,
    rr: f64,
    tg: f64,
    range: f64,
    tau_g: f64,
    hyst: f64,
    makeup: f64,
}
impl Dynamics {
    pub fn new(fs: f64) -> Self {
        let p = DynSet { ratio: 0.0, thr: 1.0 / 3.0, rel: 0.3, fast: false, grange: 0.0, gthr: 0.25, grel: 0.3, gate: false, gfast: false };
        let mut d = Dynamics { dt: 1.0 / fs, tavg: 25e-3 * 3.3e-9 / 1e-6, ms: 0.0, vh: 0.0, vm: 0.0, gr_g: 0.0, gr_c: 0.0, open: false, p, r: 1.0, slope: 0.0, t: 0.0, ra: 1e4, rr: 0.0, tg: 0.0, range: 0.0, tau_g: 0.1, hyst: 2.0, makeup: 0.0 };
        d.set(p);
        d
    }
    pub fn set(&mut self, p: DynSet) {
        self.p = p;
        self.r = ratio_of(p.ratio);
        self.slope = if self.r > 1e8 { 1.0 } else { 1.0 - 1.0 / self.r };
        self.t = 10.0 - 30.0 * p.thr; // threshold +10..-20 dB (panel)
        self.ra = if p.fast { 300.0 } else { 10e3 }; // R19 / R21
        self.rr = 250e3 * alog(p.rel); // release pot 250K log
        self.tg = 10.0 - 40.0 * p.gthr; // gate threshold +10..-30 (panel)
        self.range = 40.0 * p.grange; // 0..40 dB (panel)
        self.tau_g = (10e3 + 250e3 * alog(p.grel)) * 6.8e-6; // R47 + pot, C9 6u8
        self.hyst = 2.0 + 0.1 * (10.0 - self.tg); // ASSUMED hysteresis law
        self.makeup = (-self.t).max(0.0) * self.slope; // automatic make-up, positive only (D18)
    }
    pub fn idle(&mut self) {
        self.gr_c = 0.0;
        self.gr_g = 0.0;
    }
    /// Returns the VCA gain in dB for one sidechain sample (volts).
    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let dt = self.dt;
        self.ms += (x * x - self.ms) * (dt / self.tavg);
        let l = 10.0 * (self.ms / (DBU * DBU) + 1e-12).log10();
        let mut gr = 0.0;
        if self.r > 1.001 {
            let vr = (l - self.t).max(0.0) / 10.0; // 10 dB = 1 V
            let g5 = 3.3e-6 / dt;
            let g6 = 22e-6 / dt;
            let ga = 1.0 / self.ra;
            let (g23, g22) = (1e-4, 1e-4);
            let grr = 1.0 / self.rr.max(10.0);
            let d5 = self.vh - self.vm;
            let d6 = self.vm;
            let mut sol = solve4(false, 0.0, ga, g5, g6, g22, g23, grr, d5, d6);
            if sol[3] < vr {
                sol = solve4(true, vr, ga, g5, g6, g22, g23, grr, d5, d6);
            }
            self.vh = sol[0].max(0.0);
            self.vm = sol[1];
            gr = 10.0 * self.vh * self.slope;
        }
        let mut tgt = 0.0;
        if self.range > 0.01 {
            if self.p.gate {
                if l > self.tg {
                    self.open = true;
                } else if l < self.tg - self.hyst {
                    self.open = false;
                }
                tgt = if self.open { 0.0 } else { self.range };
            } else {
                tgt = self.range.min((self.tg - l).max(0.0));
            }
        }
        if tgt < self.gr_g {
            let sl = (if self.p.gfast { 40.0 / 100e-6 } else { 40.0 / 1.5e-3 }) * dt;
            self.gr_g = tgt.max(self.gr_g - sl);
        } else {
            self.gr_g += (tgt - self.gr_g) * (1.0 - (-dt / self.tau_g).exp());
        }
        self.gr_c = gr;
        -gr + if self.r > 1.001 { self.makeup } else { 0.0 } - self.gr_g
    }
}

// attack/release network: B -(Ra)- H ; H -C5- M ; M -C6- gnd ; B -R23- N ; M -R22- N ; N -Rr- gnd
#[allow(clippy::too_many_arguments)]
#[inline]
fn solve4(forced: bool, vr: f64, ga: f64, g5: f64, g6: f64, g22: f64, g23: f64, gr: f64, d5: f64, d6: f64) -> [f64; 4] {
    let mut a = [ga + g5, -g5, 0.0, -ga, -g5, g5 + g6 + g22, -g22, 0.0, 0.0, -g22, g22 + g23 + gr, -g23, -ga, 0.0, -g23, ga + g23];
    let mut b = [g5 * d5, -g5 * d5 + g6 * d6, 0.0, 0.0];
    if forced {
        a[12] = 0.0;
        a[13] = 0.0;
        a[14] = 0.0;
        a[15] = 1.0;
        b[3] = vr;
    }
    crate::mna::lu_solve(&mut a, 4, &mut b, 1);
    b
}

// ---------------- VCA (dbx 2151 / 202X) ----------------
const VCA_A3: f64 = 4e-5; // ASSUMED residual 3rd-order term: 0.003 % THD at +4 dBu
#[inline]
pub fn vca(x: f64, gdb: f64) -> f64 {
    let y = x * 10f64.powf(gdb / 20.0);
    y - VCA_A3 * y * y * y
}

/// Fader law: ASSUMED (the 82E17 fader card is not modelled), 0 dB at 75 % travel.
const FADER_P: [f64; 9] = [0.0, 0.05, 0.15, 0.3, 0.45, 0.6, 0.75, 0.87, 1.0];
const FADER_DB: [f64; 9] = [-120.0, -60.0, -40.0, -25.0, -15.0, -7.0, 0.0, 5.0, 10.0];
pub fn fader_db(pos: f64) -> f64 {
    let pos = pos.clamp(0.0, 1.0);
    for i in 1..FADER_P.len() {
        if pos <= FADER_P[i] {
            let f = (pos - FADER_P[i - 1]) / (FADER_P[i] - FADER_P[i - 1]);
            return FADER_DB[i - 1] + f * (FADER_DB[i] - FADER_DB[i - 1]);
        }
    }
    10.0
}
/// Pan: 10K lin dual with R5/R6 6K81 from input to each wiper (T0611E-72).
pub fn pan_gain(a: f64) -> f64 {
    let lo = a * 10e3;
    let up = (1.0 - a) * 10e3;
    let upp = up * 6.81e3 / (up + 6.81e3);
    if lo <= 0.0 {
        0.0
    } else {
        lo / (lo + upp)
    }
}

// ---------------- bus compressor 82E26 + 82E27 ----------------
pub const ATT_R: [f64; 6] = [820.0, 2.7e3, 8.2e3, 27e3, 82e3, 270e3]; // R1..R6, panel .1 .3 1 3 10 30 ms
const REL_R: [f64; 4] = [180e3, 270e3, 560e3, 1.2e6]; // R12, R11, R10, R9 with 0.47 uF; index 4 = Auto
pub const RATIOS: [f64; 3] = [2.0, 4.0, 10.0];

#[derive(Clone, Copy, PartialEq)]
pub struct BusSet {
    pub on: bool,
    pub thr: f64,
    pub mk: f64,
    pub att: usize,
    pub rel: usize,
    pub ratio: usize,
}

pub struct BusComp {
    dt: f64,
    v1: f64,
    v2: f64,
    pub gr: f64,
    p: BusSet,
    t: f64,
    make: f64,
    ra: f64,
    beta: f64,
}
impl BusComp {
    pub fn new(fs: f64) -> Self {
        let p = BusSet { on: false, thr: 0.5, mk: 0.0, att: 2, rel: 4, ratio: 1 };
        let mut b = BusComp { dt: 1.0 / fs, v1: 0.0, v2: 0.0, gr: 0.0, p, t: 0.0, make: 0.0, ra: 8.2e3, beta: 3.0 };
        b.set(p);
        b
    }
    pub fn set(&mut self, p: BusSet) {
        self.p = p;
        self.t = -15.0 + 30.0 * p.thr;
        self.make = 15.0 * p.mk;
        self.ra = ATT_R[p.att.min(5)];
        self.beta = RATIOS[p.ratio.min(2)] - 1.0;
    }
    /// Returns the gain in dB for the audio VCAs (both sides).
    #[inline]
    pub fn tick(&mut self, xl: f64, xr: f64) -> f64 {
        let dt = self.dt;
        let pk = xl.abs().max(xr.abs());
        let lpk = 20.0 * (pk / (DBU * std::f64::consts::SQRT_2) + 1e-9).log10(); // dB re peak of a 0 dBu sine
        let knee = self.t + 8.0; // ASSUMED calibration: threshold 0 = +8 dB peak
        let drive = self.beta * (lpk - knee) / 10.0;
        let b1 = self.beta + 1.0;
        let ga = 1.0 / self.ra;
        if self.p.rel < 4 {
            let gc = 0.47e-6 / dt;
            let gr = 1.0 / REL_R[self.p.rel];
            let vp = self.v1;
            let mut v = (gc * vp + ga * drive) / (gc + gr + ga * b1);
            if drive - b1 * v <= 0.0 {
                v = gc * vp / (gc + gr);
            }
            self.v1 = v.max(0.0);
            self.v2 = 0.0;
        } else {
            // Auto: R7 91K || C1 0.47 uF in series with R8 750K || C2 6.8 uF
            let (g1, g2, r7, r8) = (0.47e-6 / dt, 6.8e-6 / dt, 1.0 / 91e3, 1.0 / 750e3);
            let (p1, p2) = (self.v1, self.v2);
            let (a11, a12, a21, a22) = (g1 + r7 + ga * b1, ga * b1, ga * b1, g2 + r8 + ga * b1);
            let (c1, c2) = (g1 * p1 + ga * drive, g2 * p2 + ga * drive);
            let det = a11 * a22 - a12 * a21;
            let mut v1 = (c1 * a22 - a12 * c2) / det;
            let mut v2 = (a11 * c2 - a21 * c1) / det;
            if drive - b1 * (v1 + v2) <= 0.0 {
                v1 = g1 * p1 / (g1 + r7);
                v2 = g2 * p2 / (g2 + r8);
            }
            self.v1 = v1.max(0.0);
            self.v2 = v2.max(0.0);
        }
        self.gr = 10.0 * (self.v1 + self.v2);
        if self.p.on {
            -self.gr + self.make
        } else {
            0.0
        }
    }
}

// ---------------- settings and the console ----------------

#[derive(Clone, Copy, PartialEq)]
pub enum DynPos {
    Off,
    PreEq,
    PostEq,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub mic: bool,
    pub mic_gain: f64,
    pub pad: bool,
    pub phase: bool,
    pub trim: f64,
    pub dyn_pos: DynPos,
    pub dynamics: DynSet,
    pub eq_on: bool,
    pub eq: EqSet,
    pub hpf: f64,
    pub lpf: f64,
    pub lpf_resonant: bool,
    pub fader: f64,
    pub pan: f64,
    pub mute: bool,
    pub hiss: bool,
    pub bus: BusSet,
    pub out_db: f64,
}

/// Meter readings, written by the audio thread and read by the editor.
#[derive(Clone, Copy, Default)]
pub struct MeterFrame {
    pub input_db: f64,
    pub out_db: f64,
    pub gr: f64,
    pub gate: f64,
    pub bus_l_db: f64,
    pub bus_r_db: f64,
    pub bus_gr: f64,
}

struct Side {
    eq: SsRun,
    hpf: SsRun,
    lpf: SsRun,
    mic: MicAmp,
    up: Up2,
    dn: Down2,
    up_in: Up2,
    dn_out: Down2,
}
impl Side {
    fn new(fs2: f64) -> Self {
        Side { eq: SsRun::new(), hpf: SsRun::new(), lpf: SsRun::new(), mic: MicAmp::new(2.0 * fs2), up: Up2::new(), dn: Down2::new(), up_in: Up2::new(), dn_out: Down2::new() }
    }
}

pub struct Console {
    fs2: f64,
    sides: [Side; 2],
    dynamics: Dynamics,
    bus: BusComp,
    rng: Rng,
    s: Option<Settings>,
    g1: f64,
    g9: f64,
    trim_g: f64,
    fad_g: f64,
    bal_l: f64,
    bal_r: f64,
    kth: f64,
    rth: f64,
    mon: f64,
    pk_in: f64,
    pk_out: f64,
    pk_l: f64,
    pk_r: f64,
}

const KIN_LINE_DBU: f64 = 18.0; // host 0 dBFS = +18 dBu at the line input (EBU R68)
const KIN_MIC_DBU: f64 = -32.0; // host 0 dBFS = -32 dBu at the mic input
const KOUT_DBU: f64 = 24.0; // 0 dBFS out = +24 dBu, before the Output control

fn dbu_peak(dbu: f64) -> f64 {
    DBU * 10f64.powf(dbu / 20.0) * std::f64::consts::SQRT_2
}

impl Console {
    pub fn new(sample_rate: f64) -> Self {
        let fs2 = 2.0 * sample_rate;
        Console {
            fs2,
            sides: [Side::new(fs2), Side::new(fs2)],
            dynamics: Dynamics::new(fs2),
            bus: BusComp::new(fs2),
            rng: Rng::new(0x1234_5678),
            s: None,
            g1: 1.0,
            g9: 1.0,
            trim_g: 1.0,
            fad_g: 1.0,
            bal_l: 1.0,
            bal_r: 1.0,
            kth: 1.0,
            rth: 150.0,
            mon: 1.0,
            pk_in: 0.0,
            pk_out: 0.0,
            pk_l: 0.0,
            pk_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        let fs2 = self.fs2;
        self.sides = [Side::new(fs2), Side::new(fs2)];
        let s = self.s.take();
        self.dynamics = Dynamics::new(fs2);
        self.bus = BusComp::new(fs2);
        if let Some(s) = s {
            self.apply(&s);
        }
    }

    /// Apply settings; rebuilds only the networks whose controls changed.
    pub fn apply(&mut self, s: &Settings) {
        let old = self.s;
        let eq_changed = old.map_or(true, |o| o.eq != s.eq);
        let hpf_changed = old.map_or(true, |o| o.hpf != s.hpf);
        let lpf_changed = old.map_or(true, |o| o.lpf != s.lpf || o.lpf_resonant != s.lpf_resonant);
        if eq_changed {
            let ss = to_ss(&mut build_eq(&s.eq), self.fs2, "in", "v2");
            for sd in self.sides.iter_mut() {
                sd.eq.set(&ss);
            }
        }
        if hpf_changed && s.hpf > 0.001 {
            let ss = to_ss(&mut build_hpf(s.hpf), self.fs2, "in", "out");
            for sd in self.sides.iter_mut() {
                sd.hpf.set(&ss);
            }
        }
        if lpf_changed && s.lpf > 0.001 {
            let c43 = if s.lpf_resonant { 68e-9 } else { 6.8e-9 };
            let ss = to_ss(&mut build_lpf(s.lpf, c43), self.fs2, "in", "out");
            for sd in self.sides.iter_mut() {
                sd.lpf.set(&ss);
            }
        }
        if old.map_or(true, |o| o.dynamics != s.dynamics) {
            self.dynamics.set(s.dynamics);
        }
        if old.map_or(true, |o| o.bus != s.bus) {
            self.bus.set(s.bus);
        }
        let (g1, g9) = mic_gains(s.mic_gain);
        self.g1 = g1;
        self.g9 = g9;
        self.trim_g = 10f64.powf((-20.0 + 40.0 * s.trim) / 20.0);
        self.fad_g = 10f64.powf(fader_db(s.fader) / 20.0);
        let pc = pan_gain(0.5);
        self.bal_l = (pan_gain(1.0 - s.pan) / pc).min(1.0);
        self.bal_r = (pan_gain(s.pan) / pc).min(1.0);
        // source seen by the transformer: mic 150 R (ASSUMED), R1/R2 6K81 phantom feeds, optional pad
        let (rs, rph) = (150.0, 13.62e3);
        let k1 = rph / (rs + rph);
        let r1 = rs * rph / (rs + rph);
        if s.pad {
            let rser = r1 + 1298.0;
            self.kth = k1 * 165.0 / (rser + 165.0);
            self.rth = rser * 165.0 / (rser + 165.0);
        } else {
            self.kth = k1;
            self.rth = r1;
        }
        self.mon = 10f64.powf(s.out_db / 20.0) / dbu_peak(KOUT_DBU);
        self.s = Some(*s);
    }

    #[inline]
    fn input_stage(&mut self, side: usize, x: f64, s: &Settings) -> f64 {
        let v = if s.mic {
            let nz = if s.hiss { self.rng.gauss() * 3.4e-7 * 1.73 } else { 0.0 }; // ASSUMED EIN -127 dBu
            let (kth, rth, g1, g9) = (self.kth, self.rth, self.g1, self.g9);
            let sd = &mut self.sides[side];
            let (a, b) = sd.up.push(x + nz);
            let a = clip(-g9 * clip(g1 * sd.mic.tick(a * kth, rth), VCLIP), VCLIP);
            let b = clip(-g9 * clip(g1 * sd.mic.tick(b * kth, rth), VCLIP), VCLIP);
            -sd.dn.push(a, b) // T4 phase stage restores polarity
        } else {
            clip(x * self.trim_g, VCLIP)
        };
        if s.phase {
            -v
        } else {
            v
        }
    }

    #[inline]
    fn filters(&mut self, side: usize, v: f64, s: &Settings) -> f64 {
        let sd = &mut self.sides[side];
        let mut v = v;
        if s.eq_on {
            v = clip(sd.eq.tick(v), VCLIP);
        }
        if s.hpf > 0.001 {
            v = sd.hpf.tick(v);
        }
        if s.lpf > 0.001 {
            v = sd.lpf.tick(v);
        }
        v
    }

    /// Process one block in place. `l` and `r` are the host buffers.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = match self.s {
            Some(s) => s,
            None => return,
        };
        let kin = dbu_peak(if s.mic { KIN_MIC_DBU } else { KIN_LINE_DBU });
        for i in 0..l.len() {
            let (l0, l1) = self.sides[0].up_in.push(l[i] as f64 * kin);
            let (r0, r1) = self.sides[1].up_in.push(r[i] as f64 * kin);
            let mut ol = [0.0; 2];
            let mut or = [0.0; 2];
            for (ph, (xl, xr)) in [(l0, r0), (l1, r1)].into_iter().enumerate() {
                let mut vl = self.input_stage(0, xl, &s);
                let mut vr = self.input_stage(1, xr, &s);
                self.pk_in = self.pk_in.max(vl.abs()).max(vr.abs());
                let mut gdb = 0.0;
                if s.dyn_pos == DynPos::PreEq {
                    gdb = self.dynamics.tick(((vl * vl + vr * vr) * 0.5).sqrt());
                }
                vl = self.filters(0, vl, &s);
                vr = self.filters(1, vr, &s);
                if s.dyn_pos == DynPos::PostEq {
                    gdb = self.dynamics.tick(((vl * vl + vr * vr) * 0.5).sqrt());
                }
                if s.dyn_pos == DynPos::Off {
                    self.dynamics.idle();
                }
                if gdb != 0.0 {
                    vl = clip(vca(vl, gdb), VCLIP);
                    vr = clip(vca(vr, gdb), VCLIP);
                }
                let (fg, mute) = (self.fad_g, s.mute);
                vl = if mute { 0.0 } else { clip(vca(vl, 0.0) * fg, VCLIP) };
                vr = if mute { 0.0 } else { clip(vca(vr, 0.0) * fg, VCLIP) };
                self.pk_out = self.pk_out.max(vl.abs()).max(vr.abs());
                // mix bus: balance, mix amp noise and clipping, then the bus compressor
                let mut bl = vl * self.bal_l;
                let mut br = vr * self.bal_r;
                if s.hiss {
                    bl += self.rng.gauss() * 2.5e-5; // ASSUMED mix bus floor ~ -88 dBu
                    br += self.rng.gauss() * 2.5e-5;
                }
                bl = clip(bl, VCLIP);
                br = clip(br, VCLIP);
                let g = self.bus.tick(bl, br);
                if g != 0.0 {
                    bl = clip(vca(bl, g), VCLIP);
                    br = clip(vca(br, g), VCLIP);
                }
                ol[ph] = bl;
                or[ph] = br;
            }
            let yl = self.sides[0].dn_out.push(ol[0], ol[1]);
            let yr = self.sides[1].dn_out.push(or[0], or[1]);
            self.pk_l = self.pk_l.max(yl.abs());
            self.pk_r = self.pk_r.max(yr.abs());
            l[i] = (yl * self.mon) as f32;
            r[i] = (yr * self.mon) as f32;
        }
    }

    /// Peak levels (dB re the peak of a 0 dBu sine) and gain reduction since the last call.
    pub fn take_meters(&mut self) -> MeterFrame {
        let to_db = |v: f64| 20.0 * (v / (DBU * std::f64::consts::SQRT_2) + 1e-9).log10();
        let on = self.s.map_or(false, |s| s.bus.on);
        let m = MeterFrame {
            input_db: to_db(self.pk_in),
            out_db: to_db(self.pk_out),
            gr: self.dynamics.gr_c,
            gate: self.dynamics.gr_g,
            bus_l_db: to_db(self.pk_l),
            bus_r_db: to_db(self.pk_r),
            bus_gr: if on { self.bus.gr } else { 0.0 },
        };
        self.pk_in = 0.0;
        self.pk_out = 0.0;
        self.pk_l = 0.0;
        self.pk_r = 0.0;
        m
    }
}
