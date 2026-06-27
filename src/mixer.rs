//! Mixer math: gain conversion, channel mapping, summing, and limiter.
//!
//! All functions here are pure (no CPAL), making them unit-testable.

/// Convert decibels to linear gain: `10^(db / 20)`.
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Hard-clip a single sample to `[-1.0, 1.0]`.
///
/// v0.1 uses hard clipping. A look-ahead limiter can be added later.
pub fn hard_limit_sample(x: f32) -> f32 {
    x.clamp(-1.0, 1.0)
}

/// Apply [`hard_limit_sample`] to every element of a buffer in place.
pub fn hard_limit_buffer(buf: &mut [f32]) {
    for s in buf.iter_mut() {
        *s = hard_limit_sample(*s);
    }
}

/// Mix (add) a single route's audio from an interleaved input buffer into an
/// interleaved output buffer.
///
/// # Arguments
///
/// * `input` — interleaved input samples.
/// * `input_channels` — physical channel count of the input stream.
/// * `output` — interleaved output buffer (cleared to silence by the caller
///   before the first route).
/// * `output_channels` — physical channel count of the output stream.
/// * `from_channels_1based` — source physical channels (1-based), parallel to
///   `to_channels_1based`.
/// * `to_channels_1based` — destination physical channels (1-based).
/// * `gain` — linear gain (already converted from dB; 0.0 for muted routes).
///
/// Channel numbers are **1-based** physical indices. The frame count processed
/// is the minimum of `input.len() / input_channels` and
/// `output.len() / output_channels`.
pub fn mix_route_interleaved(
    input: &[f32],
    input_channels: usize,
    output: &mut [f32],
    output_channels: usize,
    from_channels_1based: &[usize],
    to_channels_1based: &[usize],
    gain: f32,
) {
    if input_channels == 0 || output_channels == 0 {
        return;
    }

    let in_frames = input.len() / input_channels;
    let out_frames = output.len() / output_channels;
    let frames = in_frames.min(out_frames);

    let pairs = from_channels_1based.len().min(to_channels_1based.len());

    for frame in 0..frames {
        let in_base = frame * input_channels;
        let out_base = frame * output_channels;

        for pair in 0..pairs {
            let from_ch = from_channels_1based[pair];
            let to_ch = to_channels_1based[pair];

            if from_ch == 0 || to_ch == 0 {
                continue;
            }

            // 1-based to 0-based
            let in_idx = in_base + (from_ch - 1);
            let out_idx = out_base + (to_ch - 1);

            if in_idx < input.len() && out_idx < output.len() {
                output[out_idx] += input[in_idx] * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_linear_zero_db() {
        let gain = db_to_linear(0.0);
        assert!((gain - 1.0).abs() < 1e-5);
    }

    #[test]
    fn db_to_linear_minus_six() {
        let gain = db_to_linear(-6.0);
        // -6 dB ≈ 0.501
        assert!((gain - 0.5012).abs() < 0.001);
    }

    #[test]
    fn db_to_linear_plus_six() {
        let gain = db_to_linear(6.0);
        // +6 dB ≈ 1.995
        assert!((gain - 1.9952).abs() < 0.001);
    }

    #[test]
    fn hard_limit_above_one() {
        assert_eq!(hard_limit_sample(1.5), 1.0);
    }

    #[test]
    fn hard_limit_below_minus_one() {
        assert_eq!(hard_limit_sample(-2.0), -1.0);
    }

    #[test]
    fn hard_limit_passthrough() {
        assert!((hard_limit_sample(0.5) - 0.5).abs() < 1e-6);
        assert!((hard_limit_sample(-0.3) - (-0.3)).abs() < 1e-6);
    }

    #[test]
    fn hard_limit_buffer_in_place() {
        let mut buf = vec![-2.0f32, -0.5, 0.0, 0.5, 2.0];
        hard_limit_buffer(&mut buf);
        assert_eq!(buf, vec![-1.0, -0.5, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn stereo_passthrough() {
        // 2 frames, 2 channels
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let mut output = vec![0.0f32; 4];

        mix_route_interleaved(
            &input,
            2,           // input_channels
            &mut output, // output
            2,           // output_channels
            &[1, 2],     // from_channels (1-based)
            &[1, 2],     // to_channels (1-based)
            1.0,         // gain
        );

        assert_eq!(output, input);
    }

    #[test]
    fn mono_to_stereo() {
        // 2 frames, 1 channel
        let input = vec![0.5f32, 0.7];
        let mut output = vec![0.0f32; 4]; // 2 frames * 2 channels

        mix_route_interleaved(
            &input,
            1, // input_channels
            &mut output,
            2,       // output_channels
            &[1, 1], // ch1 -> ch1 and ch1 -> ch2
            &[1, 2],
            1.0,
        );

        // frame 0: ch1=0.5, ch2=0.5
        // frame 1: ch1=0.7, ch2=0.7
        assert!((output[0] - 0.5).abs() < 1e-6);
        assert!((output[1] - 0.5).abs() < 1e-6);
        assert!((output[2] - 0.7).abs() < 1e-6);
        assert!((output[3] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn summing_multiple_routes() {
        // Two routes targeting the same output
        let input_a = vec![0.3f32, 0.4];
        let input_b = vec![0.1f32, 0.2];
        let mut output = vec![0.0f32; 4]; // 2 frames * 2 channels

        // Route A: stereo passthrough
        mix_route_interleaved(&input_a, 1, &mut output, 2, &[1, 1], &[1, 2], 1.0);
        // Route B: add into same output
        mix_route_interleaved(&input_b, 1, &mut output, 2, &[1, 1], &[1, 2], 1.0);

        // frame 0: ch1 = 0.3 + 0.1 = 0.4, ch2 = same
        // frame 1: ch1 = 0.4 + 0.2 = 0.6, ch2 = same
        assert!((output[0] - 0.4).abs() < 1e-6);
        assert!((output[1] - 0.4).abs() < 1e-6);
        assert!((output[2] - 0.6).abs() < 1e-6);
        assert!((output[3] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn fanout_one_input_to_two_outputs() {
        let input = vec![0.5f32, 0.6, 0.7, 0.8]; // 2 frames * 2ch
        let mut output_a = vec![0.0f32; 4];
        let mut output_b = vec![0.0f32; 4];

        // Route to output A
        mix_route_interleaved(&input, 2, &mut output_a, 2, &[1, 2], &[1, 2], 1.0);
        // Same route to output B (non-destructive: input unchanged)
        mix_route_interleaved(&input, 2, &mut output_b, 2, &[1, 2], &[1, 2], 1.0);

        // Both outputs receive the same data
        assert_eq!(output_a, output_b);
        assert_eq!(output_a, input);
    }

    #[test]
    fn gain_applied() {
        let input = vec![0.5f32, 0.5];
        let mut output = vec![0.0f32; 4];
        let gain = db_to_linear(-6.0);

        mix_route_interleaved(&input, 1, &mut output, 2, &[1, 1], &[1, 2], gain);

        // 0.5 * 0.5012 ≈ 0.2506
        assert!((output[0] - (0.5 * gain)).abs() < 1e-5);
        assert!((output[1] - (0.5 * gain)).abs() < 1e-5);
    }

    #[test]
    fn muted_route_gain_zero() {
        let input = vec![0.5f32, 0.5];
        let mut output = vec![0.0f32; 4];

        mix_route_interleaved(&input, 1, &mut output, 2, &[1, 1], &[1, 2], 0.0);

        // All zeros — muted
        assert!(output.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn channel_remap_3_4_to_1_2() {
        // 4-channel input: ch3=0.3, ch4=0.4 in each frame
        let input = vec![
            0.0f32, 0.0, 0.3, 0.4, // frame 0
            0.0, 0.0, 0.5, 0.6, // frame 1
        ];
        let mut output = vec![0.0f32; 4]; // 2ch output, 2 frames

        mix_route_interleaved(
            &input,
            4, // 4 input channels
            &mut output,
            2,       // 2 output channels
            &[3, 4], // from physical ch3, ch4
            &[1, 2], // to physical ch1, ch2
            1.0,
        );

        assert!((output[0] - 0.3).abs() < 1e-6);
        assert!((output[1] - 0.4).abs() < 1e-6);
        assert!((output[2] - 0.5).abs() < 1e-6);
        assert!((output[3] - 0.6).abs() < 1e-6);
    }
}
