use std::sync::Arc;

use nih_plug::prelude::*;

use crate::params::StepOneParams;

/// A transport-synced Euclidean arpeggiator CLAP plugin.
///
/// Sequencer state lives here on the plugin struct, NOT in Params. Params holds
/// only what the user/host controls; sequencer state is ephemeral and not persisted.
pub struct StepOne {
    params: Arc<StepOneParams>,

    /// Cached sample rate from the last `initialize()` call.
    sample_rate: f32,
}

impl Default for StepOne {
    fn default() -> Self {
        Self {
            params: Arc::new(StepOneParams::default()),
            sample_rate: 0.0,
        }
    }
}

impl Plugin for StepOne {
    const NAME: &'static str = "StepOne";
    const VENDOR: &'static str = "step-one";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Pure MIDI effect — no audio input or output.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];

    // Accept NoteOn, NoteOff, PolyPressure, and PolyPan.
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    // Emit NoteOn, NoteOff, and PolyPan.
    const MIDI_OUTPUT: MidiConfig = MidiConfig::Basic;

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        true
    }

    fn reset(&mut self) {
        // TODO: clear all sequencer state once it exists.
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // REVIEW(keepalive): using ProcessStatus::KeepAlive so the host calls
        //   process() continuously for transport-synced step detection.
        //   Verify this works correctly in Bitwig with no audio I/O.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for StepOne {
    const CLAP_ID: &'static str = "com.step-one.step-one";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Transport-synced Euclidean arpeggiator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Custom("note-effect")];
}
