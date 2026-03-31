// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resampler algorithm selection (no `rubato` dependency; usable on all targets).

use std::str::FromStr;

/// Rubato resampler preset selected via `--resampler`.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum ResamplerType {
    /// Synchronous FFT resampler (default).
    #[default]
    Fft,
    SincCubic,
    SincQuadratic,
    SincLinear,
    SincNearest,
    PolySeptic,
    PolyQuintic,
    PolyCubic,
    PolyLinear,
    PolyNearest,
}

impl ResamplerType {
    /// Strings accepted by [`FromStr`] and the `--resampler` CLI flag.
    pub const VARIANTS: &'static [&'static str] = &[
        "fft",
        "sinc-cubic",
        "sinc-quadratic",
        "sinc-linear",
        "sinc-nearest",
        "poly-septic",
        "poly-quintic",
        "poly-cubic",
        "poly-linear",
        "poly-nearest",
    ];
}

impl FromStr for ResamplerType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fft" => Ok(Self::Fft),
            "sinc-cubic" => Ok(Self::SincCubic),
            "sinc-quadratic" => Ok(Self::SincQuadratic),
            "sinc-linear" => Ok(Self::SincLinear),
            "sinc-nearest" => Ok(Self::SincNearest),
            "poly-septic" => Ok(Self::PolySeptic),
            "poly-quintic" => Ok(Self::PolyQuintic),
            "poly-cubic" => Ok(Self::PolyCubic),
            "poly-linear" => Ok(Self::PolyLinear),
            "poly-nearest" => Ok(Self::PolyNearest),
            _ => Err(format!(
                "unknown resampler {s:?}, expected one of: {}",
                Self::VARIANTS.join(", ")
            )),
        }
    }
}
