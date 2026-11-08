use std::sync::Arc;

use alkove_core::{math, Chain, ChainOptions, RATE};
use alkove_engine::Model;
use voiceflow_speaker_filter::gain::{InputFloor, InputGain};
use voiceflow_speaker_filter::models::LoadedModels;

const HOP: usize = 256;

#[test]
#[ignore = "requires the private checksum-pinned model directory; run explicitly with VOICEFLOW_SPEAKER_FILTER_MODELS"]
fn extracted_input_gain_matches_upstream_chain_gain_bit_for_bit() {
    let models_dir = std::env::var("VOICEFLOW_SPEAKER_FILTER_MODELS")
        .expect("set VOICEFLOW_SPEAKER_FILTER_MODELS to the private model directory");
    let loaded =
        LoadedModels::load(std::path::Path::new(&models_dir)).expect("pinned local model files");
    let focus: Arc<Model> = loaded.focus.clone();
    let cue: Arc<Model> = loaded.cue.clone();
    let mut upstream = Chain::new(
        &focus,
        &cue,
        ChainOptions {
            sleep: false,
            normalize: false,
            ..ChainOptions::default()
        },
    )
    .expect("upstream Chain");
    let mut extracted_floor = InputFloor::default();
    let mut extracted_gain = InputGain::default();
    let mut extracted_output = [0.0; HOP];
    let mut hop = [0.0; HOP];
    let mut events = Vec::new();

    for hop_index in 0..2_200 {
        if (300..1_800).contains(&hop_index) {
            let amplitude = math::pow(10.0, -60.0 / 20.0) * std::f64::consts::SQRT_2;
            for (sample_index, sample) in hop.iter_mut().enumerate() {
                *sample = (amplitude
                    * math::sin(
                        2.0 * std::f64::consts::PI
                            * 200.0
                            * (hop_index * HOP + sample_index) as f64
                            / RATE,
                    )) as f32;
            }
        } else if hop_index >= 1_800 {
            let amplitude = math::pow(10.0, -10.0 / 20.0) * std::f64::consts::SQRT_2;
            for (sample_index, sample) in hop.iter_mut().enumerate() {
                *sample = (amplitude
                    * math::sin(
                        2.0 * std::f64::consts::PI
                            * 200.0
                            * (hop_index * HOP + sample_index) as f64
                            / RATE,
                    )) as f32;
            }
        } else {
            hop.fill(0.0);
        }

        let floor = extracted_floor.observe(&hop);
        extracted_gain.hop(&hop, floor);
        extracted_gain.apply(&hop, &mut extracted_output);
        upstream.step(&hop, &mut events);
        assert_eq!(
            upstream.input_gain_db().to_bits(),
            extracted_gain.db().to_bits(),
            "input-gain trajectory differs at 256-sample hop {hop_index}"
        );
    }
}
