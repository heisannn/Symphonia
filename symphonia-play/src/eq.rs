// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// Parametric EQ (Equalizer APO / EasyEffects style text) applied to decoded PCM.

use dsp_process::SplitProcess;
use idsp::iir::{Biquad, DirectForm2Transposed, coefficients};
use log::warn;
use symphonia::core::audio::{
    AsGenericAudioBufferRef, Audio, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef,
};

/// Parsed parametric EQ preset (`Preamp` + ordered `Filter` entries).
#[derive(Clone, Debug)]
pub struct EqConfig {
    pub preamp_db: f32,
    pub filters: Vec<FilterSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
}

#[derive(Clone, Debug)]
pub struct FilterSpec {
    pub enabled: bool,
    pub kind: FilterKind,
    pub fc_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Runtime: biquad coefficients and per-channel state.
pub struct ParametricEqRuntime {
    preamp_linear: f32,
    biquads: Vec<Biquad<f32>>,
    /// `states[channel][section]`
    states: Vec<Vec<DirectForm2Transposed<f32>>>,
    work_buf: AudioBuffer<f32>,
}

impl ParametricEqRuntime {
    /// Builds enabled filters for the given signal spec. `cfg` must match future decoded buffers.
    pub fn new(cfg: &EqConfig, spec: &AudioSpec) -> Self {
        let sr = spec.rate() as f32;
        let n_ch = spec.channels().count();

        let preamp_linear = 10f32.powf(cfg.preamp_db / 20.0);

        let mut biquads = Vec::new();
        for f in &cfg.filters {
            if !f.enabled {
                continue;
            }
            let ba = filter_to_ba(f, sr);
            biquads.push(Biquad::from(ba));
        }

        let n_bq = biquads.len();
        let states: Vec<Vec<DirectForm2Transposed<f32>>> = (0..n_ch)
            .map(|_| (0..n_bq).map(|_| DirectForm2Transposed::default()).collect())
            .collect();

        let capacity = 4096.max(n_ch);
        Self { preamp_linear, biquads, states, work_buf: AudioBuffer::new(spec.clone(), capacity) }
    }

    /// Copy decoded audio into [`Self::work_buf`], apply EQ in-place, return a reference for output.
    pub fn apply<'a>(
        &'a mut self,
        decoded: GenericAudioBufferRef<'_>,
    ) -> GenericAudioBufferRef<'a> {
        let n = decoded.frames();
        if n == 0 {
            self.work_buf.clear();
            return self.work_buf.as_generic_audio_buffer_ref();
        }

        assert_eq!(decoded.spec(), self.work_buf.spec(), "EQ spec mismatch");

        self.work_buf.grow_capacity(n);
        self.work_buf.clear();
        self.work_buf.render_uninit(Some(n));
        decoded.copy_to(&mut self.work_buf.slice_mut(..));

        let n_ch = self.work_buf.num_planes();
        let n_bq = self.biquads.len();

        for ch in 0..n_ch {
            if let Some(plane) = self.work_buf.plane_mut(ch) {
                let plane_states = &mut self.states[ch];
                debug_assert_eq!(plane_states.len(), n_bq);
                for sample in plane.iter_mut().take(n) {
                    let mut s = *sample * self.preamp_linear;
                    for (bi, biquad) in self.biquads.iter().enumerate() {
                        s = biquad.process(&mut plane_states[bi], s);
                    }
                    *sample = s;
                }
            }
        }

        self.work_buf.as_generic_audio_buffer_ref()
    }
}

fn filter_to_ba(spec: &FilterSpec, sample_rate: f32) -> [[f32; 3]; 2] {
    let sr = sample_rate.max(1.0);
    let nyq = sr * 0.5;
    // Stay inside (0, Nyquist) for stable coefficient generation.
    let fc_min = 1.0_f32;
    let fc_max = (nyq - 1.0).max(fc_min + 1.0);
    let fc = spec.fc_hz.clamp(fc_min, fc_max);
    if (fc - spec.fc_hz).abs() > 0.5 {
        warn!(
            "EQ filter Fc {:.2} Hz clamped to {:.2} Hz (sample rate {:.0} Hz)",
            spec.fc_hz, fc, sr
        );
    }

    let q = spec.q.max(1e-6);

    let mut f = coefficients::Filter::default();
    f.frequency(fc, sr);
    f.q(q);
    f.gain(1.0);
    f.shelf_db(spec.gain_db);

    match spec.kind {
        FilterKind::Peaking => f.peaking(),
        FilterKind::LowShelf => f.lowshelf(),
        FilterKind::HighShelf => f.highshelf(),
    }
}

/// Parse a whitespace-oriented Parametric EQ text (`Preamp: …`, `Filter n: ON PK Fc … Hz Gain … dB Q …`).
///
/// Unknown standalone tokens are skipped so one-line exports with extra spacing still parse.
pub fn parse_parametric_eq(text: &str) -> Result<EqConfig, &'static str> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    let mut preamp_db = 0.0_f32;
    let mut filters = Vec::new();

    while i < tokens.len() {
        let head = tokens[i].trim();
        let cmd = head.trim_end_matches(':').to_lowercase();

        match cmd.as_str() {
            "preamp" => {
                i += 1;
                if i >= tokens.len() {
                    return Err("preamp: missing value");
                }
                preamp_db =
                    tokens[i].parse().map_err(|_| "preamp: expected number after Preamp:")?;
                i += 1;
                if i < tokens.len() && tokens[i].eq_ignore_ascii_case("db") {
                    i += 1;
                }
            }
            "filter" => {
                i += 1;
                if i < tokens.len() {
                    let tok = tokens[i].trim();
                    if tok.ends_with(':')
                        && tok[..tok.len().saturating_sub(1)].chars().all(|c| c.is_ascii_digit())
                    {
                        i += 1;
                    }
                }
                if i >= tokens.len() {
                    return Err("filter: missing ON/OFF");
                }
                let st = tokens[i].trim().trim_end_matches(':').to_lowercase();
                let enabled = match st.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("filter: expected ON or OFF"),
                };
                i += 1;

                if i >= tokens.len() {
                    return Err("filter: missing type");
                }
                let kind = match tokens[i].to_uppercase().as_str() {
                    "PK" | "PEQ" => FilterKind::Peaking,
                    "LSC" => FilterKind::LowShelf,
                    "HSC" => FilterKind::HighShelf,
                    _ => return Err("filter: unsupported type (expected PK, PEQ, LSC, or HSC)"),
                };
                i += 1;

                if i >= tokens.len() || !tokens[i].eq_ignore_ascii_case("fc") {
                    return Err("filter: expected Fc");
                }
                i += 1;
                if i >= tokens.len() {
                    return Err("filter: missing Fc value");
                }
                let fc_hz: f32 = tokens[i].parse().map_err(|_| "filter: bad Fc")?;
                i += 1;
                if i < tokens.len() && tokens[i].eq_ignore_ascii_case("hz") {
                    i += 1;
                }

                if i >= tokens.len() || !tokens[i].eq_ignore_ascii_case("gain") {
                    return Err("filter: expected Gain");
                }
                i += 1;
                if i >= tokens.len() {
                    return Err("filter: missing Gain value");
                }
                let gain_db: f32 = tokens[i].parse().map_err(|_| "filter: bad Gain")?;
                i += 1;
                if i < tokens.len() && tokens[i].eq_ignore_ascii_case("db") {
                    i += 1;
                }

                if i >= tokens.len() || !tokens[i].eq_ignore_ascii_case("q") {
                    return Err("filter: expected Q");
                }
                i += 1;
                if i >= tokens.len() {
                    return Err("filter: missing Q value");
                }
                let q: f32 = tokens[i].parse().map_err(|_| "filter: bad Q")?;
                i += 1;

                filters.push(FilterSpec { enabled, kind, fc_hz, gain_db, q });
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok(EqConfig { preamp_db, filters })
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::audio::Audio;

    const SAMPLE: &str = r#"Preamp: -4.12 dB Filter 1: ON LSC Fc 105.0 Hz Gain 2.8 dB Q 0.70 Filter 2: ON PK Fc 20.0 Hz Gain 0.9 dB Q 0.97"#;

    #[test]
    fn parse_sample_line() {
        let c = parse_parametric_eq(SAMPLE).expect("parse");
        assert!((c.preamp_db + 4.12).abs() < 0.01);
        assert_eq!(c.filters.len(), 2);
        assert_eq!(c.filters[0].kind, FilterKind::LowShelf);
        assert!((c.filters[0].fc_hz - 105.0).abs() < 0.01);
        assert!(c.filters[0].enabled);
        assert_eq!(c.filters[1].kind, FilterKind::Peaking);
    }

    #[test]
    fn parse_off_filter_omitted_from_runtime() {
        let text = "Preamp: 0 dB Filter 1: OFF PK Fc 1000 Hz Gain 3 dB Q 1 Filter 2: ON PK Fc 500 Hz Gain 1 dB Q 2";
        let c = parse_parametric_eq(text).expect("parse");
        let spec = AudioSpec::new(48_000, symphonia::core::audio::layouts::CHANNEL_LAYOUT_STEREO);
        let rt = ParametricEqRuntime::new(&c, &spec);
        assert_eq!(rt.biquads.len(), 1);
    }

    #[test]
    fn silent_in_silent_out_identity_chain() {
        let c = EqConfig { preamp_db: 0.0, filters: vec![] };
        let spec = AudioSpec::new(48_000, symphonia::core::audio::layouts::CHANNEL_LAYOUT_STEREO);
        let mut rt = ParametricEqRuntime::new(&c, &spec);
        let mut buf = AudioBuffer::<f32>::new(spec.clone(), 32);
        buf.render_silence(Some(16));
        let r = rt.apply(buf.as_generic_audio_buffer_ref());
        assert_eq!(r.frames(), 16);
        match r {
            GenericAudioBufferRef::F32(b) => {
                assert_eq!(b.plane(0).unwrap()[0], 0.0);
            }
            _ => panic!("expected f32"),
        }
    }

    #[test]
    fn biquad_coefficients_are_finite() {
        let text = "Preamp: 0 dB Filter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1";
        let c = parse_parametric_eq(text).expect("parse");
        let ba = filter_to_ba(&c.filters[0], 48_000.0);
        for row in &ba {
            for x in row {
                assert!(x.is_finite(), "non-finite coefficient {x}");
            }
        }
    }
}
