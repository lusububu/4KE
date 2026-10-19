// 4000 E console: a Rust/nih-plug port of the "Them Changes" console web app. One SSL 4000 E
// channel strip (mic/line amp, dynamics, 242 equaliser, filters, VCA fader, pan) feeding the mix
// bus and its compressor, all solved from the service-manual schematics. dsp.rs and mna.rs are
// line-for-line ports of the web app's dsp.js; the knob mappings below match page.tpl.html.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub mod dsp;
mod editor;
mod knob;
pub mod mna;
#[cfg(test)]
mod tests;

use dsp::{BusSet, Console, DynPos, DynSet, MeterFrame, Settings};
use mna::EqSet;

/// Meter values shared with the editor (f64 bits in atomics, no locks on the audio thread).
pub struct Meters {
    v: [AtomicU64; 7],
}
impl Meters {
    fn new() -> Self {
        Meters { v: std::array::from_fn(|_| AtomicU64::new((-120f64).to_bits())) }
    }
    fn store(&self, m: &MeterFrame) {
        let vals = [m.input_db, m.out_db, m.gr, m.gate, m.bus_l_db, m.bus_r_db, m.bus_gr];
        for (a, v) in self.v.iter().zip(vals) {
            a.store(v.to_bits(), Ordering::Relaxed);
        }
    }
    pub fn load(&self) -> MeterFrame {
        let g = |i: usize| f64::from_bits(self.v[i].load(Ordering::Relaxed));
        MeterFrame { input_db: g(0), out_db: g(1), gr: g(2), gate: g(3), bus_l_db: g(4), bus_r_db: g(5), bus_gr: g(6) }
    }
}

#[derive(Enum, Debug, PartialEq, Clone, Copy)]
pub enum DynPosition {
    #[id = "off"]
    #[name = "Off"]
    Off,
    #[id = "pre"]
    #[name = "Before EQ"]
    Before,
    #[id = "post"]
    #[name = "After EQ"]
    After,
}

#[derive(Params)]
pub struct ConsoleParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    // input
    #[id = "mic"]
    pub mic: BoolParam,
    #[id = "micgain"]
    pub mic_gain: FloatParam,
    #[id = "pad"]
    pub pad: BoolParam,
    #[id = "phase"]
    pub phase: BoolParam,
    #[id = "trim"]
    pub trim: FloatParam,

    // dynamics
    #[id = "dynpos"]
    pub dyn_pos: EnumParam<DynPosition>,
    #[id = "ratio"]
    pub ratio: FloatParam,
    #[id = "thr"]
    pub thr: FloatParam,
    #[id = "rel"]
    pub rel: FloatParam,
    #[id = "fast"]
    pub fast: BoolParam,
    #[id = "grange"]
    pub grange: FloatParam,
    #[id = "gthr"]
    pub gthr: FloatParam,
    #[id = "grel"]
    pub grel: FloatParam,
    #[id = "gate"]
    pub gate: BoolParam,
    #[id = "gfast"]
    pub gfast: BoolParam,

    // equaliser (gains stored 0..1 with 0.5 = flat)
    #[id = "eqon"]
    pub eq_on: BoolParam,
    #[id = "hfg"]
    pub hf_g: FloatParam,
    #[id = "hff"]
    pub hf_f: FloatParam,
    #[id = "hfbell"]
    pub hf_bell: BoolParam,
    #[id = "hmfg"]
    pub hmf_g: FloatParam,
    #[id = "hmff"]
    pub hmf_f: FloatParam,
    #[id = "hmfq"]
    pub hmf_q: FloatParam,
    #[id = "lmfg"]
    pub lmf_g: FloatParam,
    #[id = "lmff"]
    pub lmf_f: FloatParam,
    #[id = "lmfq"]
    pub lmf_q: FloatParam,
    #[id = "lfg"]
    pub lf_g: FloatParam,
    #[id = "lff"]
    pub lf_f: FloatParam,
    #[id = "lfbell"]
    pub lf_bell: BoolParam,

    // output
    #[id = "hpf"]
    pub hpf: FloatParam,
    #[id = "lpf"]
    pub lpf: FloatParam,
    #[id = "fader"]
    pub fader: FloatParam,
    #[id = "pan"]
    pub pan: FloatParam,
    #[id = "mute"]
    pub mute: BoolParam,

    // fine-tuning
    #[id = "rv1"]
    pub rv1: FloatParam,
    #[id = "rv2"]
    pub rv2: FloatParam,
    #[id = "rv3"]
    pub rv3: FloatParam,
    #[id = "rv4"]
    pub rv4: FloatParam,
    #[id = "lpfres"]
    pub lpf_resonant: BoolParam,
    #[id = "hiss"]
    pub hiss: BoolParam,

    // bus compressor
    #[id = "buson"]
    pub bus_on: BoolParam,
    #[id = "busthr"]
    pub bus_thr: FloatParam,
    #[id = "busmk"]
    pub bus_mk: FloatParam,
    #[id = "busratio"]
    pub bus_ratio: IntParam,
    #[id = "busatt"]
    pub bus_att: IntParam,
    #[id = "busrel"]
    pub bus_rel: IntParam,

    // monitor / output level
    #[id = "out"]
    pub output: FloatParam,
}

fn unit(name: &str, def: f32) -> FloatParam {
    FloatParam::new(name, def, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(formatters::v2s_f32_rounded(2))
}

impl Default for ConsoleParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),
            mic: BoolParam::new("Mic input", false),
            mic_gain: unit("Mic gain", 0.5),
            pad: BoolParam::new("Pad -20", false),
            phase: BoolParam::new("Phase", false),
            trim: unit("Line trim", 0.5),
            dyn_pos: EnumParam::new("Dynamics position", DynPosition::After),
            ratio: unit("Comp ratio", 0.0),
            thr: unit("Comp threshold", 1.0 / 3.0),
            rel: unit("Comp release", 0.3),
            fast: BoolParam::new("Comp fast attack", false),
            grange: unit("Gate range", 0.0),
            gthr: unit("Gate threshold", 0.25),
            grel: unit("Gate release", 0.3),
            gate: BoolParam::new("Hard gate", false),
            gfast: BoolParam::new("Gate fast attack", false),
            eq_on: BoolParam::new("EQ in", true),
            hf_g: unit("Highs gain", 0.5),
            hf_f: unit("Highs freq", 0.5),
            hf_bell: BoolParam::new("Highs bell", false),
            hmf_g: unit("High mids gain", 0.5),
            hmf_f: unit("High mids freq", 0.5),
            hmf_q: unit("High mids Q", 0.5),
            lmf_g: unit("Low mids gain", 0.5),
            lmf_f: unit("Low mids freq", 0.5),
            lmf_q: unit("Low mids Q", 0.5),
            lf_g: unit("Lows gain", 0.5),
            lf_f: unit("Lows freq", 0.5),
            lf_bell: BoolParam::new("Lows bell", false),
            hpf: unit("High-pass", 0.0),
            lpf: unit("Low-pass", 0.0),
            fader: unit("Fader", 0.75),
            pan: unit("Pan", 0.5),
            mute: BoolParam::new("Mute", false),
            rv1: unit("Highs bell width", 0.5),
            rv2: unit("High mids width", 0.5),
            rv3: unit("Low mids width", 0.5),
            rv4: unit("Lows bell width", 0.5),
            lpf_resonant: BoolParam::new("Low-pass resonant", false),
            hiss: BoolParam::new("Background hiss", true),
            bus_on: BoolParam::new("Bus compressor on", false),
            bus_thr: unit("Bus threshold", 0.5),
            bus_mk: unit("Bus make-up", 0.0),
            bus_ratio: IntParam::new("Bus ratio", 1, IntRange::Linear { min: 0, max: 2 }),
            bus_att: IntParam::new("Bus attack", 2, IntRange::Linear { min: 0, max: 5 }),
            bus_rel: IntParam::new("Bus release", 4, IntRange::Linear { min: 0, max: 4 }),
            output: unit("Output", 30.0 / 36.0),
        }
    }
}

impl ConsoleParams {
    /// Engine settings from the current parameter values (same mappings as the web page).
    pub fn settings(&self) -> Settings {
        let g = |p: &FloatParam| ((p.value() as f64) - 0.5) * 2.0;
        let f = |p: &FloatParam| p.value() as f64;
        let filt = |p: &FloatParam| if p.value() < 0.005 { 0.0 } else { p.value() as f64 }; // detented OUT
        Settings {
            mic: self.mic.value(),
            mic_gain: f(&self.mic_gain),
            pad: self.pad.value(),
            phase: self.phase.value(),
            trim: f(&self.trim),
            dyn_pos: match self.dyn_pos.value() {
                DynPosition::Off => DynPos::Off,
                DynPosition::Before => DynPos::PreEq,
                DynPosition::After => DynPos::PostEq,
            },
            dynamics: DynSet {
                ratio: f(&self.ratio),
                thr: f(&self.thr),
                rel: f(&self.rel),
                fast: self.fast.value(),
                grange: f(&self.grange),
                gthr: f(&self.gthr),
                grel: f(&self.grel),
                gate: self.gate.value(),
                gfast: self.gfast.value(),
            },
            eq_on: self.eq_on.value(),
            eq: EqSet {
                hf_g: g(&self.hf_g),
                hf_f: f(&self.hf_f),
                hf_bell: self.hf_bell.value(),
                hmf_g: g(&self.hmf_g),
                hmf_f: f(&self.hmf_f),
                hmf_q: f(&self.hmf_q),
                lmf_g: g(&self.lmf_g),
                lmf_f: f(&self.lmf_f),
                lmf_q: f(&self.lmf_q),
                lf_g: g(&self.lf_g),
                lf_f: f(&self.lf_f),
                lf_bell: self.lf_bell.value(),
                rv1: 10e3 * f(&self.rv1),
                rv2: 500.0 * f(&self.rv2),
                rv3: 500.0 * f(&self.rv3),
                rv4: 5e3 * f(&self.rv4),
            },
            hpf: filt(&self.hpf),
            lpf: filt(&self.lpf),
            lpf_resonant: self.lpf_resonant.value(),
            fader: f(&self.fader),
            pan: f(&self.pan),
            mute: self.mute.value(),
            hiss: self.hiss.value(),
            bus: BusSet {
                on: self.bus_on.value(),
                thr: f(&self.bus_thr),
                mk: f(&self.bus_mk),
                att: self.bus_att.value() as usize,
                rel: self.bus_rel.value() as usize,
                ratio: self.bus_ratio.value() as usize,
            },
            out_db: -24.0 + 36.0 * f(&self.output),
        }
    }
}

pub struct ConsolePlugin {
    params: Arc<ConsoleParams>,
    console: Option<Console>,
    meters: Arc<Meters>,
    meter_count: usize,
    meter_every: usize,
}

impl Default for ConsolePlugin {
    fn default() -> Self {
        Self { params: Arc::new(ConsoleParams::default()), console: None, meters: Arc::new(Meters::new()), meter_count: 0, meter_every: 2048 }
    }
}

impl Plugin for ConsolePlugin {
    const NAME: &'static str = "4000 E Console";
    const VENDOR: &'static str = "Them Changes";
    const URL: &'static str = "https://claude.ai/artifact/SwUktSGsa8pCpYyBMngJe8";
    const EMAIL: &'static str = "info@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.params.editor_state.clone(), self.meters.clone())
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, buffer_config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        let sr = buffer_config.sample_rate as f64;
        let mut c = Console::new(sr);
        c.apply(&self.params.settings());
        self.console = Some(c);
        self.meter_every = (sr / 30.0) as usize; // about 30 meter updates per second
        true
    }

    fn reset(&mut self) {
        if let Some(c) = &mut self.console {
            c.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.params.settings();
        let console = match &mut self.console {
            Some(c) => c,
            None => return ProcessStatus::Normal,
        };
        console.apply(&settings);
        let n = buffer.samples();
        let slices = buffer.as_slice();
        if slices.len() >= 2 {
            let (a, b) = slices.split_at_mut(1);
            console.process(&mut a[0][..], &mut b[0][..]);
        }
        self.meter_count += n;
        if self.meter_count >= self.meter_every {
            self.meter_count = 0;
            self.meters.store(&console.take_meters());
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ConsolePlugin {
    const CLAP_ID: &'static str = "com.themchanges.console-4000e";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("SSL 4000 E channel strip and bus compressor, modelled from the schematics");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Equalizer, ClapFeature::Compressor];
}

impl Vst3Plugin for ConsolePlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"ThemChanges4000E";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Eq, Vst3SubCategory::Dynamics];
}

nih_export_clap!(ConsolePlugin);
nih_export_vst3!(ConsolePlugin);
