use std::sync::Arc;

use nih_plug::prelude::*;

use crate::params::StepOneParams;
use crate::seq::euclidean::EuclideanPattern;
use crate::seq::held_notes::HeldNotes;
use crate::seq::pending::PendingNoteOffs;

/// A transport-synced Euclidean arpeggiator CLAP plugin.
///
/// Sequencer state lives here on the plugin struct, NOT in Params. Params holds
/// only what the user/host controls; sequencer state is ephemeral and not persisted.
pub struct StepOne {
    params: Arc<StepOneParams>,

    /// Cached sample rate from the last `initialize()` call.
    sample_rate: f32,

    /// Precomputed Euclidean gate pattern.
    pattern: EuclideanPattern,

    /// Currently held input MIDI notes, sorted ascending.
    held_notes: HeldNotes,

    /// Scheduled output NoteOff events.
    pending_offs: PendingNoteOffs,

    /// Last-seen steps value, for change detection.
    cached_steps: i32,

    /// Last-seen pulses value, for change detection.
    cached_pulses: i32,

    /// Expected start beat of the next buffer, for transport jump detection.
    prev_end_beat: Option<f64>,
}

impl Default for StepOne {
    fn default() -> Self {
        Self {
            params: Arc::new(StepOneParams::default()),
            sample_rate: 0.0,
            pattern: EuclideanPattern::new(),
            held_notes: HeldNotes::new(),
            pending_offs: PendingNoteOffs::new(),
            // Set to -1 to force recompute on first process() call.
            cached_steps: -1,
            cached_pulses: -1,
            prev_end_beat: None,
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
        self.held_notes.clear();
        self.pending_offs.clear();
        self.pattern = EuclideanPattern::new();
        // Force pattern recompute on next process() call.
        self.cached_steps = -1;
        self.cached_pulses = -1;
        self.prev_end_beat = None;
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    /// Minimal mock for `InitContext` — nih-plug does not expose test utilities,
    /// so we implement just enough to call `Plugin::initialize()`.
    struct MockInitContext;

    impl InitContext<StepOne> for MockInitContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }

        fn execute(&self, _task: ()) {}

        fn set_latency_samples(&self, _samples: u32) {}

        fn set_current_voice_capacity(&self, _capacity: u32) {}
    }

    /// Mock `ProcessContext` that feeds a pre-built list of MIDI events
    /// and captures output events sent via `send_event()`.
    struct MockProcessContext {
        events: VecDeque<NoteEvent<()>>,
        transport: Transport,
        sent_events: Vec<NoteEvent<()>>,
    }

    impl MockProcessContext {
        fn new(sample_rate: f32, events: Vec<NoteEvent<()>>) -> Self {
            // HACK(transport): Transport::new() is pub(crate), so we zero-initialize
            //   and set public fields. All pub(crate) fields are Option types that are
            //   valid when zeroed (None). This is fragile if nih-plug adds non-zero-safe
            //   fields — pin the nih-plug git dependency and revisit on updates.
            let mut transport: Transport = unsafe { std::mem::zeroed() };
            transport.sample_rate = sample_rate;
            Self {
                events: events.into(),
                transport,
                sent_events: Vec::new(),
            }
        }

        fn with_transport(mut self, playing: bool, tempo: Option<f64>) -> Self {
            self.transport.playing = playing;
            self.transport.tempo = tempo;
            self
        }
    }

    impl ProcessContext<StepOne> for MockProcessContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }

        fn execute_background(&self, _task: ()) {}

        fn execute_gui(&self, _task: ()) {}

        fn transport(&self) -> &Transport {
            &self.transport
        }

        fn next_event(&mut self) -> Option<NoteEvent<()>> {
            self.events.pop_front()
        }

        fn send_event(&mut self, event: NoteEvent<()>) {
            self.sent_events.push(event);
        }

        fn set_latency_samples(&self, _samples: u32) {}

        fn set_current_voice_capacity(&self, _capacity: u32) {}
    }

    /// Helper: call initialize() then reset() on a plugin instance, matching
    /// the real nih-plug host lifecycle (Default → initialize → reset → process).
    fn initialize_plugin(mut plugin: StepOne, sample_rate: f32) -> StepOne {
        // AUDIO_IO_LAYOUTS is empty, so use a default layout.
        let layout = AudioIOLayout::const_default();
        let config = BufferConfig {
            sample_rate,
            min_buffer_size: None,
            max_buffer_size: 512,
            process_mode: ProcessMode::Realtime,
        };
        let result = plugin.initialize(&layout, &config, &mut MockInitContext);
        assert!(result, "initialize() should return true");
        plugin.reset();
        plugin
    }

    /// Helper: construct a default plugin and call initialize().
    fn init_plugin(sample_rate: f32) -> StepOne {
        initialize_plugin(StepOne::default(), sample_rate)
    }

    #[test]
    fn plugin_can_be_constructed() {
        let plugin = StepOne::default();
        let _params = plugin.params();
    }

    #[test]
    fn initialize_stores_sample_rate() {
        let plugin = init_plugin(44100.0);
        assert_eq!(plugin.sample_rate, 44100.0);
    }

    #[test]
    fn reset_clears_held_notes() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);
        assert!(!plugin.held_notes.is_empty());

        plugin.reset();
        assert!(plugin.held_notes.is_empty());
    }

    #[test]
    fn reset_clears_pending_offs() {
        let mut plugin = init_plugin(44100.0);
        plugin
            .pending_offs
            .add(crate::seq::pending::PendingNoteOff {
                note: 60,
                channel: 0,
                voice_id: None,
                off_at_beat: 1.0,
            });
        assert!(!plugin.pending_offs.is_empty());

        plugin.reset();
        assert!(plugin.pending_offs.is_empty());
    }

    #[test]
    fn reset_clears_expression_stashes() {
        let mut plugin = init_plugin(44100.0);
        // Stash values for notes not yet held.
        plugin.held_notes.set_pressure(60, 0.5);
        plugin.held_notes.set_pan(60, -0.7);

        plugin.reset();

        // After reset, adding note 60 should get default expression values,
        // not the stashed ones.
        plugin.held_notes.note_on(60, 0.8);
        let note = plugin.held_notes.next_note().unwrap();
        assert!((note.pressure - 1.0).abs() < f32::EPSILON);
        assert!(note.pan.abs() < f32::EPSILON);
    }

    #[test]
    fn reset_forces_pattern_recompute() {
        let mut plugin = init_plugin(44100.0);
        // Simulate having cached params.
        plugin.cached_steps = 8;
        plugin.cached_pulses = 4;

        plugin.reset();

        // cached values should be -1 to force recompute.
        assert_eq!(plugin.cached_steps, -1);
        assert_eq!(plugin.cached_pulses, -1);
    }
}
