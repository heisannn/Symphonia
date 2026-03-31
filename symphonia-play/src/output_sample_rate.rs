// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Allowed output sample rates for `--output-sample-rate` (Hz).

/// String values accepted by clap `possible_values` (decimal Hz).
/// Corresponds to 44.1, 48, 88.2, 96, 176.4, 192, 352.8, 384, 705.6, 768 kHz.
pub const OUTPUT_SAMPLE_RATE_CHOICES: &[&str] = &[
    "44100", "48000", "88200", "96000", "176400", "192000", "352800", "384000", "705600", "768000",
];
