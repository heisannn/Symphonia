// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::marker::PhantomData;

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, PolynomialDegree, Resampler as RubatoResampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::resampler_type::ResamplerType;
use symphonia::core::audio::conv::{FromSample, IntoSample};
use symphonia::core::audio::sample::Sample;
use symphonia::core::audio::{Audio, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef};

pub struct Resampler<T> {
    resampler: Box<dyn RubatoResampler<f32>>,
    buf_in: AudioBuffer<f32>,
    buf_out: AudioBuffer<f32>,
    chunk_size: usize,
    n_channels: usize,
    /// Staging buffer for rubato input (planar, fixed input chunk size).
    staging_in: Vec<Vec<f32>>,
    /// Staging buffer for rubato output (planar, max output frames per chunk).
    staging_out: Vec<Vec<f32>>,
    // May take your heart.
    phantom: PhantomData<T>,
}

fn make_rubato_resampler(
    kind: ResamplerType,
    spec_in: &AudioSpec,
    out_sample_rate: u32,
    chunk_size: usize,
    n_channels: usize,
) -> Box<dyn RubatoResampler<f32>> {
    let rate_in = spec_in.rate() as f64;
    let rate_out = out_sample_rate as f64;
    let ratio = rate_out / rate_in;
    let max_rel = 1.1_f64;

    match kind {
        ResamplerType::Fft => Box::new(
            Fft::<f32>::new(
                spec_in.rate() as usize,
                out_sample_rate as usize,
                chunk_size,
                2,
                n_channels,
                FixedSync::Input,
            )
            .expect("Fft resampler"),
        ),
        ResamplerType::SincCubic
        | ResamplerType::SincQuadratic
        | ResamplerType::SincLinear
        | ResamplerType::SincNearest => {
            let interpolation = match kind {
                ResamplerType::SincCubic => SincInterpolationType::Cubic,
                ResamplerType::SincQuadratic => SincInterpolationType::Quadratic,
                ResamplerType::SincLinear => SincInterpolationType::Linear,
                ResamplerType::SincNearest => SincInterpolationType::Nearest,
                _ => unreachable!(),
            };
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                oversampling_factor: 128,
                interpolation,
                window: WindowFunction::BlackmanHarris2,
            };
            Box::new(
                Async::<f32>::new_sinc(
                    ratio,
                    max_rel,
                    &params,
                    chunk_size,
                    n_channels,
                    FixedAsync::Input,
                )
                .expect("Async sinc resampler"),
            )
        }
        ResamplerType::PolySeptic
        | ResamplerType::PolyQuintic
        | ResamplerType::PolyCubic
        | ResamplerType::PolyLinear
        | ResamplerType::PolyNearest => {
            let degree = match kind {
                ResamplerType::PolySeptic => PolynomialDegree::Septic,
                ResamplerType::PolyQuintic => PolynomialDegree::Quintic,
                ResamplerType::PolyCubic => PolynomialDegree::Cubic,
                ResamplerType::PolyLinear => PolynomialDegree::Linear,
                ResamplerType::PolyNearest => PolynomialDegree::Nearest,
                _ => unreachable!(),
            };
            Box::new(
                Async::<f32>::new_poly(
                    ratio,
                    max_rel,
                    degree,
                    chunk_size,
                    n_channels,
                    FixedAsync::Input,
                )
                .expect("Async poly resampler"),
            )
        }
    }
}

impl<T> Resampler<T>
where
    T: Sample + FromSample<f32> + IntoSample<f32>,
{
    fn resample_inner<'a>(&mut self, dst: &'a mut Vec<T>) -> &'a [T] {
        // Clear the output buffer.
        self.buf_out.clear();

        // Keep resampling chunks until there are not enough input frames left.
        while self.chunk_size <= self.buf_in.frames() {
            // The resampler will produce this many frames next.
            let len = RubatoResampler::output_frames_next(self.resampler.as_ref());

            // If required, grow the output buffer to accomodate the output.
            let begin = self.buf_out.frames();
            self.buf_out.grow_capacity(begin + len);

            // Reserve frames for the resampler output.
            self.buf_out.render_uninit(Some(len));

            // Copy input chunk into staging (rubato 1.x uses audioadapter buffers).
            for (ch, plane) in self.buf_in.iter_planes().enumerate() {
                self.staging_in[ch][..self.chunk_size].copy_from_slice(&plane[..self.chunk_size]);
            }

            let input_adapter =
                SequentialSliceOfVecs::new(&self.staging_in, self.n_channels, self.chunk_size)
                    .expect("staging_in matches channel count and chunk size");

            let mut output_adapter =
                SequentialSliceOfVecs::new_mut(&mut self.staging_out, self.n_channels, len)
                    .expect("staging_out matches channel count and output length");

            let (read, _) = RubatoResampler::process_into_buffer(
                self.resampler.as_mut(),
                &input_adapter,
                &mut output_adapter,
                None,
            )
            .expect("resampler process_into_buffer");

            // Copy resampled chunk into buf_out.
            for (ch, plane) in self.buf_out.iter_planes_mut().enumerate() {
                plane[begin..begin + len].copy_from_slice(&self.staging_out[ch][..len]);
            }

            // Remove consumed samples from the input buffer.
            self.buf_in.shift(read);
        }

        // Return interleaved samples.
        self.buf_out.copy_to_vec_interleaved(dst);

        dst
    }
}

impl<T> Resampler<T>
where
    T: Sample + FromSample<f32> + IntoSample<f32>,
{
    pub fn new(
        spec_in: &AudioSpec,
        out_sample_rate: u32,
        chunk_size: usize,
        kind: ResamplerType,
    ) -> Self {
        let n_channels = spec_in.channels().count();

        let resampler =
            make_rubato_resampler(kind, spec_in, out_sample_rate, chunk_size, n_channels);

        let spec_out = AudioSpec::new(out_sample_rate, spec_in.channels().clone());

        let buf_in = AudioBuffer::new(spec_in.clone(), chunk_size);
        let buf_out =
            AudioBuffer::new(spec_out, RubatoResampler::output_frames_max(resampler.as_ref()));

        let staging_in = (0..n_channels).map(|_| vec![0.0f32; chunk_size]).collect();
        let staging_out = (0..n_channels)
            .map(|_| vec![0.0f32; RubatoResampler::output_frames_max(resampler.as_ref())])
            .collect();

        Self {
            resampler,
            buf_in,
            buf_out,
            chunk_size,
            n_channels,
            staging_in,
            staging_out,
            phantom: Default::default(),
        }
    }

    /// Resamples a planar/non-interleaved input.
    ///
    /// Returns the resampled samples in an interleaved format.
    pub fn resample<'a>(&mut self, src: GenericAudioBufferRef<'_>, dst: &'a mut Vec<T>) -> &'a [T] {
        // Calculate the space required in the resampler input buffer.
        let begin = self.buf_in.frames();
        let num_frames = src.frames();

        // If required, grow the resampler input buffer capacity.
        self.buf_in.grow_capacity(begin + num_frames);

        // Reserve space in the resampler input buffer to accomodate the new frames.
        self.buf_in.render_uninit(Some(num_frames));

        // Copy and convert the source buffer to resampler input buffer.
        src.copy_to(&mut self.buf_in.slice_mut(begin..begin + num_frames));

        // Resample.
        self.resample_inner(dst)
    }

    /// Resample any remaining samples in the resample buffer.
    pub fn flush<'a>(&mut self, dst: &'a mut Vec<T>) -> &'a [T] {
        let partial_len = self.buf_in.frames() % self.chunk_size;

        if partial_len != 0 {
            // Pad the input buffer with silence such that the length of the input is a multiple of
            // the chunk size.
            self.buf_in.render_silence(Some(self.chunk_size - partial_len));
        }

        // Resample.
        self.resample_inner(dst)
    }
}
