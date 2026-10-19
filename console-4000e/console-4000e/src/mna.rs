// Modified nodal analysis for the SSL 4000 E card netlists, ported line for line from the web
// app's dsp.js. A netlist of resistors, capacitors and ideal op-amps (nullors) is turned into a
// trapezoidal state-space filter: one state per capacitor, with large coupling capacitors shorted
// unless they sit inside a loop that matters (flagged `keep`). The same netlists also give the
// editor's EQ curve, evaluated from the state-space matrices.

use std::collections::HashMap;

pub const MAX_K: usize = 16;

enum El {
    R(isize, isize, f64),
    C(isize, isize, f64, bool),
    O(isize, isize, isize),
}

pub struct Net {
    el: Vec<El>,
    nodes: HashMap<String, isize>,
}

impl Net {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert("0".to_string(), -1);
        Net { el: Vec::with_capacity(160), nodes }
    }
    pub fn n(&mut self, x: &str) -> isize {
        if let Some(&i) = self.nodes.get(x) {
            return i;
        }
        let i = self.nodes.len() as isize - 1;
        self.nodes.insert(x.to_string(), i);
        i
    }
    pub fn r(&mut self, a: &str, b: &str, v: f64) -> &mut Self {
        let (a, b) = (self.n(a), self.n(b));
        self.el.push(El::R(a, b, v.max(0.1)));
        self
    }
    pub fn c(&mut self, a: &str, b: &str, v: f64) -> &mut Self {
        let (a, b) = (self.n(a), self.n(b));
        self.el.push(El::C(a, b, v, false));
        self
    }
    pub fn c_keep(&mut self, a: &str, b: &str, v: f64) -> &mut Self {
        let (a, b) = (self.n(a), self.n(b));
        self.el.push(El::C(a, b, v, true));
        self
    }
    pub fn op(&mut self, p: &str, m: &str, o: &str) -> &mut Self {
        let (p, m, o) = (self.n(p), self.n(m), self.n(o));
        self.el.push(El::O(p, m, o));
        self
    }
    pub fn fol(&mut self, p: &str, o: &str) -> &mut Self {
        self.op(p, o, o)
    }
    /// J112 FET switch: r_ds(on) <= 50 ohm (datasheet maximum)
    pub fn sw(&mut self, a: &str, b: &str, on: bool) -> &mut Self {
        if on {
            self.r(a, b, 50.0);
        }
        self
    }
    /// centre-tapped linear pot, wiper at p (0 = end_l, 1 = end_r)
    pub fn pot_ct(&mut self, l: &str, r: &str, ct: &str, w: &str, p: f64, rt: f64) -> &mut Self {
        if p <= 0.5 {
            self.r(l, w, rt * p).r(w, ct, rt * (0.5 - p)).r(ct, r, rt / 2.0)
        } else {
            self.r(l, ct, rt / 2.0).r(ct, w, rt * (p - 0.5)).r(w, r, rt * (1.0 - p))
        }
    }
    /// three-terminal pot, p = 0 puts the wiper at a
    pub fn pot(&mut self, a: &str, b: &str, w: &str, p: f64, rt: f64) -> &mut Self {
        self.r(a, w, rt * p).r(w, b, rt * (1.0 - p))
    }
}

/// Dense LU solve with partial pivoting. `a` is n*n row-major (destroyed), `b` is n*m and is
/// overwritten with the solution.
pub fn lu_solve(a: &mut [f64], n: usize, b: &mut [f64], m: usize) {
    for k in 0..n {
        let mut p = k;
        let mut mx = a[k * n + k].abs();
        for i in k + 1..n {
            let v = a[i * n + k].abs();
            if v > mx {
                mx = v;
                p = i;
            }
        }
        if p != k {
            for j in 0..n {
                a.swap(k * n + j, p * n + j);
            }
            for j in 0..m {
                b.swap(k * m + j, p * m + j);
            }
        }
        let piv = a[k * n + k];
        if piv.abs() < 1e-300 {
            continue;
        }
        for i in k + 1..n {
            let f = a[i * n + k] / piv;
            if f == 0.0 {
                continue;
            }
            a[i * n + k] = 0.0;
            for j in k + 1..n {
                a[i * n + j] -= f * a[k * n + j];
            }
            for j in 0..m {
                b[i * m + j] -= f * b[k * m + j];
            }
        }
    }
    for i in (0..n).rev() {
        let piv = a[i * n + i];
        for j in 0..m {
            let mut s = b[i * m + j];
            for k in i + 1..n {
                s -= a[i * n + k] * b[k * m + j];
            }
            b[i * m + j] = if piv.abs() < 1e-300 { 0.0 } else { s / piv };
        }
    }
}

/// Discrete state-space: y = C h + D u ; h' = A h + B u (fixed-size so swapping costs no allocation)
#[derive(Clone)]
pub struct Ss {
    pub k: usize,
    pub a: [f64; MAX_K * MAX_K],
    pub b: [f64; MAX_K],
    pub c: [f64; MAX_K],
    pub d: f64,
}

impl Ss {
    pub fn unity() -> Self {
        Ss { k: 0, a: [0.0; MAX_K * MAX_K], b: [0.0; MAX_K], c: [0.0; MAX_K], d: 1.0 }
    }
}

pub fn to_ss(net: &mut Net, fs: f64, inp: &str, out: &str) -> Ss {
    const SHORT_ABOVE: f64 = 1e-6;
    let nn = net.nodes.len() - 1;
    let m_ops = net.el.iter().filter(|e| matches!(e, El::O(..))).count();
    let n = nn + m_ops + 1;
    let mut a = vec![0.0f64; n * n];
    let mut caps: Vec<(isize, isize, f64)> = Vec::new();
    let st = |a: &mut Vec<f64>, x: isize, y: isize, g: f64| {
        if x >= 0 {
            a[x as usize * n + x as usize] += g;
        }
        if y >= 0 {
            a[y as usize * n + y as usize] += g;
        }
        if x >= 0 && y >= 0 {
            a[x as usize * n + y as usize] -= g;
            a[y as usize * n + x as usize] -= g;
        }
    };
    for e in &net.el {
        match *e {
            El::R(x, y, v) => st(&mut a, x, y, 1.0 / v),
            El::C(x, y, v, keep) => {
                if v >= SHORT_ABOVE && !keep {
                    st(&mut a, x, y, 10.0);
                } else {
                    let g = 2.0 * v * fs;
                    caps.push((x, y, g));
                    st(&mut a, x, y, g);
                }
            }
            El::O(..) => {}
        }
    }
    let mut k = 0;
    for e in &net.el {
        if let El::O(p, m, o) = *e {
            let row = nn + k;
            a[o as usize * n + row] += 1.0;
            if p >= 0 {
                a[row * n + p as usize] += 1.0;
            }
            if m >= 0 {
                a[row * n + m as usize] -= 1.0;
            }
            k += 1;
        }
    }
    let ii = net.nodes[inp] as usize;
    let sr = nn + m_ops;
    a[ii * n + sr] += 1.0;
    a[sr * n + ii] = 1.0;
    let kk = caps.len().min(MAX_K);
    let m = kk + 1;
    let mut b = vec![0.0f64; n * m];
    b[sr * m] = 1.0;
    for (j, &(x, y, _)) in caps.iter().take(kk).enumerate() {
        if x >= 0 {
            b[x as usize * m + 1 + j] += 1.0;
        }
        if y >= 0 {
            b[y as usize * m + 1 + j] -= 1.0;
        }
    }
    lu_solve(&mut a, n, &mut b, m);
    let oi = net.nodes[out] as usize;
    let v = |node: isize, j: usize| if node >= 0 { b[node as usize * m + j] } else { 0.0 };
    let mut ss = Ss::unity();
    ss.k = kk;
    ss.d = b[oi * m];
    for r in 0..kk {
        let (x, y, g) = caps[r];
        let g2 = 2.0 * g;
        ss.b[r] = g2 * (v(x, 0) - v(y, 0));
        for j in 0..kk {
            ss.a[r * MAX_K + j] = g2 * (v(x, 1 + j) - v(y, 1 + j)) - if r == j { 1.0 } else { 0.0 };
        }
        ss.c[r] = b[oi * m + 1 + r];
    }
    ss
}

/// Running state-space filter. Swapping matrices keeps the capacitor history, which stays
/// physically meaningful when only resistors change, so knob moves don't click.
#[derive(Clone)]
pub struct SsRun {
    pub ss: Ss,
    h: [f64; MAX_K],
    t: [f64; MAX_K],
}

impl SsRun {
    pub fn new() -> Self {
        SsRun { ss: Ss::unity(), h: [0.0; MAX_K], t: [0.0; MAX_K] }
    }
    pub fn set(&mut self, ss: &Ss) {
        if ss.k != self.ss.k {
            self.h = [0.0; MAX_K];
        }
        self.ss = ss.clone();
    }
    pub fn reset(&mut self) {
        self.h = [0.0; MAX_K];
    }
    #[inline]
    pub fn tick(&mut self, u: f64) -> f64 {
        let k = self.ss.k;
        let s = &self.ss;
        let mut y = s.d * u;
        for i in 0..k {
            y += s.c[i] * self.h[i];
        }
        for i in 0..k {
            let mut acc = s.b[i] * u;
            let row = &s.a[i * MAX_K..i * MAX_K + k];
            for j in 0..k {
                acc += row[j] * self.h[j];
            }
            self.t[i] = acc;
        }
        self.h[..k].copy_from_slice(&self.t[..k]);
        y
    }
}

/// |H(e^jw)| of a discrete state-space at frequency f (Hz) for sample rate fs.
pub fn ss_mag(ss: &Ss, f: f64, fs: f64) -> f64 {
    let k = ss.k;
    if k == 0 {
        return ss.d.abs();
    }
    let w = 2.0 * std::f64::consts::PI * f / fs;
    let (zr, zi) = (w.cos(), w.sin());
    let mut ar = [0.0f64; MAX_K * MAX_K];
    let mut ai = [0.0f64; MAX_K * MAX_K];
    let mut br = [0.0f64; MAX_K];
    let mut bi = [0.0f64; MAX_K];
    for r in 0..k {
        for c in 0..k {
            ar[r * k + c] = -ss.a[r * MAX_K + c];
        }
        ar[r * k + r] += zr;
        ai[r * k + r] = zi;
        br[r] = ss.b[r];
    }
    for kk in 0..k {
        let mut p = kk;
        let mut mx = -1.0;
        for r in kk..k {
            let v = ar[r * k + kk].powi(2) + ai[r * k + kk].powi(2);
            if v > mx {
                mx = v;
                p = r;
            }
        }
        if p != kk {
            for c in 0..k {
                ar.swap(kk * k + c, p * k + c);
                ai.swap(kk * k + c, p * k + c);
            }
            br.swap(kk, p);
            bi.swap(kk, p);
        }
        let (pr, pi) = (ar[kk * k + kk], ai[kk * k + kk]);
        let pd = pr * pr + pi * pi;
        for r in kk + 1..k {
            let (xr, xi) = (ar[r * k + kk], ai[r * k + kk]);
            if xr == 0.0 && xi == 0.0 {
                continue;
            }
            let fr = (xr * pr + xi * pi) / pd;
            let fi = (xi * pr - xr * pi) / pd;
            for c in kk..k {
                let (kr, ki) = (ar[kk * k + c], ai[kk * k + c]);
                ar[r * k + c] -= fr * kr - fi * ki;
                ai[r * k + c] -= fr * ki + fi * kr;
            }
            let (bkr, bki) = (br[kk], bi[kk]);
            br[r] -= fr * bkr - fi * bki;
            bi[r] -= fr * bki + fi * bkr;
        }
    }
    for r in (0..k).rev() {
        let (mut sr, mut si) = (br[r], bi[r]);
        for c in r + 1..k {
            let (xr, xi) = (ar[r * k + c], ai[r * k + c]);
            sr -= xr * br[c] - xi * bi[c];
            si -= xr * bi[c] + xi * br[c];
        }
        let (pr, pi) = (ar[r * k + r], ai[r * k + r]);
        let pd = pr * pr + pi * pi;
        br[r] = (sr * pr + si * pi) / pd;
        bi[r] = (si * pr - sr * pi) / pd;
    }
    let (mut yr, mut yi) = (ss.d, 0.0);
    for r in 0..k {
        yr += ss.c[r] * br[r];
        yi += ss.c[r] * bi[r];
    }
    (yr * yr + yi * yi).sqrt()
}

// ---------------- 82E242 equaliser and filters (drawing T82242-70) ----------------

/// standard 'A' (log) taper: 10% resistance at half rotation, two straight segments (ASSUMED law)
pub fn alog(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if u <= 0.5 {
        0.2 * u
    } else {
        0.1 + 1.8 * (u - 0.5)
    }
}
// Dual Q pot: the only one of four section wirings that holds 15 dB of gain while Q sweeps.
fn qa(q: f64) -> f64 {
    1.0 - alog(q)
}
fn qb(q: f64) -> f64 {
    alog(q)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EqSet {
    pub hf_g: f64,
    pub hf_f: f64,
    pub hf_bell: bool,
    pub hmf_g: f64,
    pub hmf_f: f64,
    pub hmf_q: f64,
    pub lmf_g: f64,
    pub lmf_f: f64,
    pub lmf_q: f64,
    pub lf_g: f64,
    pub lf_f: f64,
    pub lf_bell: bool,
    pub rv1: f64,
    pub rv2: f64,
    pub rv3: f64,
    pub rv4: f64,
}

impl Default for EqSet {
    fn default() -> Self {
        EqSet { hf_g: 0.0, hf_f: 0.5, hf_bell: false, hmf_g: 0.0, hmf_f: 0.5, hmf_q: 0.5, lmf_g: 0.0, lmf_f: 0.5, lmf_q: 0.5, lf_g: 0.0, lf_f: 0.5, lf_bell: false, rv1: 5e3, rv2: 250.0, rv3: 250.0, rv4: 2.5e3 }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mf,
    Hf,
    Lf,
}

#[allow(clippy::too_many_arguments)]
fn band(n: &mut Net, vin: &str, vout: &str, pre: &str, other_out: &str, out: &str, kind: Kind, g: f64, fu: f64, q: f64, bell: bool, trim: f64) {
    let p = |s: &str| format!("{pre}.{s}");
    let (l, r, ct, w) = (p("l"), p("r"), p("ct"), p("w"));
    n.r(vin, &l, 1e3).r(vout, &r, 1e3).pot_ct(&l, &r, &ct, &w, g, 25e3).r(&ct, "0", 4.7e3).r(&w, "0", 1.8e3).r(&w, other_out, 12e3);
    let fa = 50e3 * alog(1.0 - fu);
    let (m, o, pp) = (p("m"), p("o"), p("p"));
    match kind {
        Kind::Mf => {
            let cc = if pre == "hm" { 4.7e-9 } else { 22e-9 };
            n.r(&w, &p("a"), fa).r(&p("a"), &p("a2"), 3e3).c(&p("a2"), &m, cc);
            n.c(&m, &o, cc).r(&m, &p("b"), 3.6e3).r(&p("b"), &o, fa);
            n.op(&pp, &m, &o).r(&pp, "0", 100e3);
            let v5 = p("v5");
            n.c_keep(&o, &v5, 100e-6); // C11 / C14: inside the Q loop
            n.r(&v5, &p("n22"), trim).r(&p("n22"), &p("n19"), 910.0).r(&p("n19"), &p("n18"), 1.8e3).r(&p("n18"), "0", 470.0);
            n.pot(&p("n18"), &p("n19"), &pp, qa(q), 10e3);
            let tm = p("tm");
            n.r(&v5, &tm, 750.0).r(&tm, &p("n21"), 750.0).r(&p("n21"), out, 10e3 * qb(q)).op("0", &tm, out);
        }
        Kind::Hf | Kind::Lf => {
            if kind == Kind::Hf {
                n.r(&w, &p("a"), fa).c(&w, &p("a"), 220e-12).r(&p("a"), &p("a2"), 3e3).c(&p("a2"), &m, 2.2e-9);
                n.c(&m, &o, 220e-12).r(&m, &p("b"), 3.6e3).r(&p("b"), &o, fa).c(&m, &p("q"), 2.2e-9);
            } else {
                n.r(&w, &p("a"), fa).c(&p("a"), &p("a2"), 80e-9);
                if !bell {
                    n.r(&p("a"), &p("a2"), 0.1); // SHELF contact shorts C16 || C17
                }
                n.r(&p("a2"), &m, 4.3e3).c(&m, &o, 80e-9).r(&m, &p("b"), 4.7e3).r(&p("b"), &o, fa);
            }
            n.op(&pp, &m, &o).r(&pp, "0", 5.1e3).r(&pp, &p("n7"), 2.4e3).r(&p("n7"), &p("q"), trim);
            let s = p("s");
            n.c(&p("q"), &s, 100e-6).r(&o, &s, 100e3).sw(&o, &s, bell);
            n.c(&o, &p("c7"), 100e-6).r(&p("c7"), &p("tm"), 3e3).r(&p("tm"), out, 43e3).op("0", &p("tm"), out);
            let (rg, cg) = if kind == Kind::Hf { (33e3, 2.2e-6) } else { (7.5e3, 100e-6) };
            n.r(&p("tm"), &p("g1"), rg).c(&p("g1"), &p("g2"), cg).r(&p("g2"), out, 100e3).sw(&p("g2"), out, bell);
        }
    }
}

/// Gains in [-1, 1] (clockwise = boost), frequencies and Q in [0, 1] (Q 0 = narrow).
pub fn build_eq(e: &EqSet) -> Net {
    let mut n = Net::new();
    let pg = |g: f64| 0.5 - 0.5 * g;
    n.r("in", "s1", 4.75e3).r("s1", "v1", 4.75e3).c("s1", "v1", 220e-12).r("s1", "hm.out", 4.75e3).r("s1", "hf.out", 4.75e3).op("0", "s1", "v1");
    band(&mut n, "in", "v1", "hm", "hf.out", "hm.out", Kind::Mf, pg(e.hmf_g), e.hmf_f, e.hmf_q, false, e.rv2);
    band(&mut n, "in", "v1", "hf", "hm.out", "hf.out", Kind::Hf, pg(e.hf_g), e.hf_f, 0.0, e.hf_bell, e.rv1);
    n.r("v1", "s2", 4.75e3).r("s2", "v2", 4.75e3).c("s2", "v2", 220e-12).r("s2", "lm.out", 4.75e3).r("s2", "lf.out", 4.75e3).op("0", "s2", "v2");
    band(&mut n, "v1", "v2", "lm", "lf.out", "lm.out", Kind::Mf, pg(e.lmf_g), e.lmf_f, e.lmf_q, false, e.rv3);
    band(&mut n, "v1", "v2", "lf", "lm.out", "lf.out", Kind::Lf, pg(e.lf_g), e.lf_f, 0.0, e.lf_bell, e.rv4);
    n
}

/// u in (0, 1]: 1 = highest corner. RV5/RV6 set for unity gain and K = 2 (3rd-order Butterworth).
pub fn build_hpf(u: f64) -> Net {
    let mut n = Net::new();
    let r = 47e3 * alog(1.0 - u);
    n.r("in", "x", 300.0).r("x", "0", 300.0).c("x", "p1", 220e-9).r("p1", "0", 1e6).r("p1", "n7", 1.8e3).r("n7", "0", r).fol("p1", "o1");
    n.c("o1", "y1", 220e-9).c("y1", "p2", 220e-9).r("y1", "n56", 1.8e3).r("n56", "out", r).r("p2", "n55", 1.8e3).r("n55", "0", r);
    n.op("p2", "m2", "out").r("out", "m2", 7.5e3).r("m2", "n57", 5.6e3).r("n57", "0", 1.9e3);
    n
}

/// u in (0, 1]: 1 = lowest corner (lin pots)
pub fn build_lpf(u: f64, c43: f64) -> Net {
    let mut n = Net::new();
    let r = 10e3 * u;
    n.r("in", "a", 680.0).r("a", "b", r).c("b", "out", c43).r("b", "c", r).r("c", "p", 680.0).c("p", "0", 3.3e-9).fol("p", "out");
    n
}
