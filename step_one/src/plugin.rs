use std::sync::Arc;

use nih_plug::prelude::*;

use crate::params::StepOneParams;
use crate::seq::clock;
use crate::seq::euclidean::EuclideanPattern;
use crate::seq::held_notes::HeldNotes;
use crate::seq::pending::{PendingNoteOff, PendingNoteOffs};

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
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Read params and drain input events.
        self.read_params_and_drain_input(context);

        // Read transport state.
        let transport = context.transport();
        let pos_beats = match transport.pos_beats() {
            Some(b) => b,
            None => return ProcessStatus::KeepAlive,
        };
        let tempo = match transport.tempo {
            Some(t) => t,
            None => return ProcessStatus::KeepAlive,
        };
        let playing = transport.playing;
        let num_samples = buffer.samples();

        // Run the sequencer core.
        self.process_sequencer(context, playing, pos_beats, tempo, num_samples);

        ProcessStatus::KeepAlive
    }
}

impl StepOne {
    /// Read params (recomputing pattern if needed) and drain all input MIDI events.
    fn read_params_and_drain_input(&mut self, context: &mut impl ProcessContext<Self>) {
        // 1. Read params — recompute pattern if steps or pulses changed.
        let steps = self.params.steps.value();
        let pulses = self.params.pulses.value().min(steps);
        if steps != self.cached_steps || pulses != self.cached_pulses {
            self.pattern.recompute(steps as usize, pulses as usize);
            self.cached_steps = steps;
            self.cached_pulses = pulses;
        }

        // 2. Drain all input MIDI events — update held notes and stashes.
        //    Do NOT forward input events to output.
        // TODO(interleave): input events are drained before step boundary scan;
        //   a future version could interleave them sample-accurately.
        //   See docs/design.md "Open Questions #1".
        while let Some(event) = context.next_event() {
            match event {
                NoteEvent::NoteOn { note, velocity, .. } => {
                    self.held_notes.note_on(note, velocity);
                }
                NoteEvent::NoteOff { note, .. } => {
                    self.held_notes.note_off(note);
                }
                NoteEvent::PolyPressure { note, pressure, .. } => {
                    self.held_notes.set_pressure(note, pressure);
                }
                NoteEvent::PolyPan { note, pan, .. } => {
                    self.held_notes.set_pan(note, pan);
                }
                _ => {}
            }
        }
    }

    /// Core sequencer logic: check transport, detect boundaries, emit gates.
    /// Separated from process() so tests can call it directly with known
    /// transport values (avoiding nih-plug Transport's pub(crate) fields).
    fn process_sequencer(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        playing: bool,
        pos_beats: f64,
        tempo: f64,
        num_samples: usize,
    ) {
        if !playing {
            // Flush all pending NoteOffs at sample 0.
            self.flush_pending_offs(context, 0);
            self.prev_end_beat = None;
            return;
        }

        // Compute the beat range for this buffer.
        let beats_per_sample = tempo / (60.0 * self.sample_rate as f64);
        let buffer_end_beat = pos_beats + num_samples as f64 * beats_per_sample;

        // Detect transport jump: if current start doesn't match expected.
        if let Some(expected) = self.prev_end_beat {
            // Allow small rounding tolerance (0.001 beats ≈ 0.5 ms at 120 BPM).
            let jump_threshold = 0.001;
            if (pos_beats - expected).abs() > jump_threshold {
                self.flush_pending_offs(context, 0);
            }
        }

        // Read sequencer params.
        let step_duration = self.params.step_duration.value() as u32;
        let gate_length_pct = self.params.gate_length.value() as f64;
        let velocity_scale = self.params.velocity.value() as f64 / 100.0;
        let steps = self.params.steps.value() as u32;

        // Find step boundaries in this buffer.
        let boundaries = clock::find_boundaries(
            pos_beats,
            buffer_end_beat,
            self.sample_rate,
            tempo,
            step_duration,
            steps,
        );

        // Emit pending NoteOffs that fall within this buffer's beat range.
        self.emit_due_noteoffs(context, pos_beats, buffer_end_beat, beats_per_sample);

        // Fire gates at step boundaries where the pattern is active.
        self.emit_gates(
            context,
            &boundaries,
            gate_length_pct,
            velocity_scale,
            step_duration,
        );

        // Update prev_end_beat for jump detection next buffer.
        self.prev_end_beat = Some(buffer_end_beat);
    }

    /// Emit pending NoteOffs whose deadline falls within [start, end).
    fn emit_due_noteoffs(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        start_beat: f64,
        end_beat: f64,
        beats_per_sample: f64,
    ) {
        let (due_offs, due_count) = self.pending_offs.take_due(start_beat, end_beat);
        for off in due_offs.iter().take(due_count).flatten() {
            // Convert beat position to sample offset.
            let sample = ((off.off_at_beat - start_beat) / beats_per_sample).round() as u32;
            context.send_event(NoteEvent::NoteOff {
                timing: sample,
                voice_id: off.voice_id,
                channel: off.channel,
                note: off.note,
                velocity: 0.0,
            });
        }
    }

    /// Fire gates at active step boundaries.
    fn emit_gates(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        boundaries: &clock::StepBoundaries,
        gate_length_pct: f64,
        velocity_scale: f64,
        step_duration: u32,
    ) {
        for boundary in boundaries.iter() {
            if !self.pattern.is_active(boundary.step_index) {
                continue;
            }

            // Gate length 0% = mute.
            if gate_length_pct <= 0.0 {
                continue;
            }

            if self.held_notes.is_empty() {
                continue;
            }

            // Get the next note from the arp cycle.
            let held = self.held_notes.next_note().unwrap();
            let note = held.note;
            let pressure = held.pressure;
            let pan = held.pan;

            // Compute output velocity: input_velocity × pressure × (velocity_param / 100).
            let output_velocity =
                (held.velocity as f64 * pressure as f64 * velocity_scale).min(1.0) as f32;

            // Same-pitch retrigger: emit pending NoteOff before new NoteOn.
            if let Some(old_off) = self.pending_offs.take_by_note(note) {
                context.send_event(NoteEvent::NoteOff {
                    timing: boundary.sample_offset,
                    voice_id: old_off.voice_id,
                    channel: old_off.channel,
                    note: old_off.note,
                    velocity: 0.0,
                });
            }

            // Emit NoteOn.
            context.send_event(NoteEvent::NoteOn {
                timing: boundary.sample_offset,
                voice_id: None,
                channel: 0,
                note,
                velocity: output_velocity,
            });

            // Emit PolyPan at the same timing.
            context.send_event(NoteEvent::PolyPan {
                timing: boundary.sample_offset,
                voice_id: None,
                channel: 0,
                note,
                pan,
            });

            // Schedule pending NoteOff.
            // Gate length is a percentage of the distance (in beats) to the next active pulse.
            let distance_steps = self.pattern.distance_to_next_pulse(boundary.step_index);
            let distance_beats = distance_steps as f64 * step_duration as f64 / 4.0;
            let gate_length_beats = (gate_length_pct / 100.0) * distance_beats;
            let off_at_beat = boundary.beat_position + gate_length_beats;

            self.pending_offs.add(PendingNoteOff {
                note,
                channel: 0,
                voice_id: None,
                off_at_beat,
            });
        }
    }

    /// Emit all pending NoteOffs at the given sample offset and clear the list.
    fn flush_pending_offs(&mut self, context: &mut impl ProcessContext<Self>, timing: u32) {
        let (flushed, count) = self.pending_offs.flush_all();
        for off in flushed.iter().take(count).flatten() {
            context.send_event(NoteEvent::NoteOff {
                timing,
                voice_id: off.voice_id,
                channel: off.channel,
                note: off.note,
                velocity: 0.0,
            });
        }
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

    /// Helper: construct a plugin with custom params and call initialize().
    fn init_plugin_with(params: StepOneParams, sample_rate: f32) -> StepOne {
        initialize_plugin(
            StepOne {
                params: Arc::new(params),
                ..StepOne::default()
            },
            sample_rate,
        )
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

    /// Helper: run the sequencer for one buffer with given transport state.
    /// Feeds input events, runs process_sequencer, and returns output events.
    fn run_sequencer(
        plugin: &mut StepOne,
        pos_beats: f64,
        tempo: f64,
        playing: bool,
        num_samples: usize,
        input_events: Vec<NoteEvent<()>>,
    ) -> Vec<NoteEvent<()>> {
        let sample_rate = plugin.sample_rate;
        let mut context =
            MockProcessContext::new(sample_rate, input_events).with_transport(playing, Some(tempo));

        // Drain input events to update held notes.
        plugin.read_params_and_drain_input(&mut context);

        // Run sequencer core with known beat position.
        plugin.process_sequencer(&mut context, playing, pos_beats, tempo, num_samples);

        context.sent_events
    }

    /// Extract note numbers from all NoteOn events.
    fn note_on_notes(events: &[NoteEvent<()>]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|e| match e {
                NoteEvent::NoteOn { note, .. } => Some(*note),
                _ => None,
            })
            .collect()
    }

    /// Find the first NoteOn and return (note, velocity), or panic.
    fn expect_note_on(events: &[NoteEvent<()>]) -> (u8, f32) {
        events
            .iter()
            .find_map(|e| match e {
                NoteEvent::NoteOn { note, velocity, .. } => Some((*note, *velocity)),
                _ => None,
            })
            .expect("expected at least one NoteOn")
    }

    /// Find the first PolyPan and return (note, pan), or panic.
    fn expect_poly_pan(events: &[NoteEvent<()>]) -> (u8, f32) {
        events
            .iter()
            .find_map(|e| match e {
                NoteEvent::PolyPan { note, pan, .. } => Some((*note, *pan)),
                _ => None,
            })
            .expect("expected at least one PolyPan")
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

    // ---- process() tests ----

    #[test]
    fn no_output_when_stopped() {
        let mut plugin = init_plugin(44100.0);
        // Hold a note so there's something to potentially output.
        plugin.held_notes.note_on(60, 0.8);

        let events = run_sequencer(&mut plugin, 0.0, 120.0, false, 512, vec![]);
        // No NoteOn output — only potential NoteOff flushes (none pending).
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, NoteEvent::NoteOn { .. })),
            "should not emit NoteOn when stopped"
        );
    }

    #[test]
    fn no_output_when_no_notes_held() {
        let mut plugin = init_plugin(44100.0);

        // Buffer from beat 0.0 with default params (8 steps, 4 pulses).
        // Pattern has pulses but no notes are held — should emit nothing.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);
        assert!(
            events.is_empty(),
            "should not emit events with no held notes"
        );
    }

    #[test]
    fn single_note_produces_noteon() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Buffer starting at beat 0.0 — step boundary at beat 0.0 (step 0).
        // Default pattern E(4,8) has step 0 active.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        let (note, _) = expect_note_on(&events);
        assert_eq!(note, 60);
    }

    #[test]
    fn noteon_has_correct_velocity() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Default velocity param = 100%, pressure = 1.0.
        // output_velocity = 0.8 × 1.0 × 1.0 = 0.8
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        let (_, velocity) = expect_note_on(&events);
        assert!(
            (velocity - 0.8).abs() < 1e-6,
            "expected velocity 0.8, got {velocity}"
        );
    }

    #[test]
    fn two_notes_alternate() {
        let mut plugin = init_plugin(44100.0);
        // E(8,8) with duration=1 → all steps active, step every 0.25 beats.
        plugin.pattern.recompute(8, 8);
        plugin.cached_steps = 8;
        plugin.cached_pulses = 8;

        plugin.held_notes.note_on(60, 0.8); // C4
        plugin.held_notes.note_on(64, 0.8); // E4

        // At 120 BPM, 44100 Hz: beats_per_sample = 120/(60*44100) ≈ 0.0000454.
        // Step boundary every 0.25 beats ≈ 5513 samples.
        // Use 16384 samples to cover ~0.743 beats → boundaries at 0.0, 0.25, 0.5.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 16384, vec![]);

        let note_ons = note_on_notes(&events);
        assert!(
            note_ons.len() >= 2,
            "expected at least 2 NoteOns, got {}",
            note_ons.len()
        );
        assert_eq!(note_ons[0], 60, "first gate should be C4");
        assert_eq!(note_ons[1], 64, "second gate should be E4");
    }

    #[test]
    fn gate_length_zero_mutes() {
        let mut plugin = init_plugin_with(StepOneParams::with_gate_length(0.0), 44100.0);
        plugin.held_notes.note_on(60, 0.8);

        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        assert!(
            note_on_notes(&events).is_empty(),
            "gate_length=0 should produce no NoteOns"
        );
    }

    #[test]
    fn polypan_emitted_with_noteon() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);
        plugin.held_notes.set_pan(60, -0.5);

        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        let (note, pan) = expect_poly_pan(&events);
        assert_eq!(note, 60);
        assert!((pan - (-0.5)).abs() < f32::EPSILON);
    }

    #[test]
    fn transport_stop_flushes_noteoffs() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // First buffer: playing, fires a gate.
        let _ = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);
        assert!(
            !plugin.pending_offs.is_empty(),
            "should have pending NoteOff"
        );

        // Second buffer: stopped — should flush pending NoteOffs.
        let events = run_sequencer(&mut plugin, 0.5, 120.0, false, 512, vec![]);

        let note_offs: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::NoteOff { .. }))
            .collect();
        assert!(
            !note_offs.is_empty(),
            "stopping transport should flush pending NoteOffs"
        );
        assert!(plugin.pending_offs.is_empty());
    }

    #[test]
    fn transport_jump_flushes_noteoffs() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // First buffer at beat 0.0 — fires a gate, sets prev_end_beat.
        let _ = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        // Jump: next buffer at beat 10.0 (far from expected ~0.556).
        // Should flush pending NoteOffs before processing.
        let events = run_sequencer(&mut plugin, 10.0, 120.0, true, 512, vec![]);

        let note_offs: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::NoteOff { .. }))
            .collect();
        assert!(
            !note_offs.is_empty(),
            "transport jump should flush pending NoteOffs"
        );
    }

    #[test]
    fn noteoff_at_correct_time() {
        // Default E(8,4) = [X.X.X.X.], distance to next pulse = 2 steps.
        // gate_length=50% of distance (2 × 0.25 beats) = 0.25 beats.
        // NoteOn at beat 0.0 → NoteOff at beat 0.25.
        let mut plugin = init_plugin_with(StepOneParams::with_gate_length(50.0), 44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Buffer 1 (512 samples ≈ 0.023 beats): fires gate at 0.0, schedules off at 0.25.
        let _ = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);
        assert!(!plugin.pending_offs.is_empty(), "NoteOff should be pending");

        // Buffer 2 at beat 0.24 (covers [0.24, 0.263)): NoteOff at 0.25 IS due.
        let events = run_sequencer(&mut plugin, 0.24, 120.0, true, 512, vec![]);

        assert!(
            events
                .iter()
                .any(|e| matches!(e, NoteEvent::NoteOff { .. })),
            "NoteOff should be emitted when its beat falls in the buffer range"
        );
    }

    #[test]
    fn nonuniform_gate_distances() {
        // E(8,3) = [X..X..X.] — pulses at 0, 3, 6 with distances 3, 3, 2.
        // gate_length=100%, step_duration=1 sixteenth.
        // Pulse at step 0: off_at = 0.0 + 3×0.25 = 0.75 beats.
        // Pulse at step 6: off_at = 6×0.25 + 2×0.25 = 1.5 + 0.5 = 2.0 beats.
        // The two NoteOff positions differ, proving per-pulse distance is used.
        let params = StepOneParams::with_pulses(3);
        let mut plugin = init_plugin_with(params, 44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Fire gate at step 0 (distance=3, off_at=0.75).
        let _ = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);
        // NoteOff for step 0 should be at beat 0.75 (distance=3, 100% gate).
        assert!(!plugin.pending_offs.is_empty(), "should have pending off");

        // Advance past the NoteOff at 0.75 to collect it, then fire step 3.
        // Buffer at beat 0.74, 512 samples ≈ 0.023 beats → covers [0.74, 0.763).
        let events = run_sequencer(&mut plugin, 0.74, 120.0, true, 512, vec![]);
        let off_count = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::NoteOff { .. }))
            .count();
        assert!(
            off_count > 0,
            "NoteOff at beat 0.75 should fire in [0.74, 0.763)"
        );

        // Now fire step 6 at beat 1.5 (global step index 6, distance=2).
        let _ = run_sequencer(&mut plugin, 1.5, 120.0, true, 512, vec![]);
        // NoteOff should be at 1.5 + 2×0.25 = 2.0 beats (distance=2).
        // Verify it's NOT at 1.5 + 3×0.25 = 2.25 (which old fixed-step would give for step_duration).
        // Buffer at beat 1.99, covers [1.99, 2.013) — should contain NoteOff at 2.0.
        let events = run_sequencer(&mut plugin, 1.99, 120.0, true, 512, vec![]);
        let off_count = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::NoteOff { .. }))
            .count();
        assert!(
            off_count > 0,
            "NoteOff at beat 2.0 (distance=2) should fire in [1.99, 2.013)"
        );
    }

    // ---- lifecycle tests ----

    #[test]
    fn input_noteon_produces_output() {
        let mut plugin = init_plugin(44100.0);

        // Feed NoteOn as an input event (not pre-loaded on held_notes).
        let events = run_sequencer(
            &mut plugin,
            0.0,
            120.0,
            true,
            512,
            vec![NoteEvent::NoteOn {
                timing: 0,
                voice_id: None,
                channel: 0,
                note: 60,
                velocity: 0.8,
            }],
        );

        let (note, _) = expect_note_on(&events);
        assert_eq!(note, 60);
    }

    #[test]
    fn input_noteoff_stops_output() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Buffer 1: playing, fires a gate.
        let _ = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        // Buffer 2: send NoteOff as input event.
        let _ = run_sequencer(
            &mut plugin,
            0.25,
            120.0,
            true,
            512,
            vec![NoteEvent::NoteOff {
                timing: 0,
                voice_id: None,
                channel: 0,
                note: 60,
                velocity: 0.0,
            }],
        );

        // Buffer 3: no notes held — should produce no NoteOns.
        let events = run_sequencer(&mut plugin, 0.5, 120.0, true, 512, vec![]);
        assert!(
            note_on_notes(&events).is_empty(),
            "no NoteOn after all notes released via input NoteOff"
        );
    }

    #[test]
    fn expression_stash_pressure_affects_velocity() {
        let mut plugin = init_plugin(44100.0);

        // Bitwig may send PolyPressure before NoteOn in the same buffer.
        // Stashed pressure should be applied when NoteOn arrives.
        // Expected output velocity: 0.8 × 0.5 × 1.0 (velocity_param=100%) = 0.4.
        let events = run_sequencer(
            &mut plugin,
            0.0,
            120.0,
            true,
            512,
            vec![
                NoteEvent::PolyPressure {
                    timing: 0,
                    voice_id: None,
                    channel: 0,
                    note: 60,
                    pressure: 0.5,
                },
                NoteEvent::NoteOn {
                    timing: 0,
                    voice_id: None,
                    channel: 0,
                    note: 60,
                    velocity: 0.8,
                },
            ],
        );

        let (_, velocity) = expect_note_on(&events);
        assert!(
            (velocity - 0.4).abs() < 1e-6,
            "expected velocity 0.4 (0.8 × 0.5), got {velocity}"
        );
    }

    #[test]
    fn expression_stash_pan_via_input() {
        let mut plugin = init_plugin(44100.0);

        // PolyPan before NoteOn — stashed pan should appear in output PolyPan.
        let events = run_sequencer(
            &mut plugin,
            0.0,
            120.0,
            true,
            512,
            vec![
                NoteEvent::PolyPan {
                    timing: 0,
                    voice_id: None,
                    channel: 0,
                    note: 60,
                    pan: -0.7,
                },
                NoteEvent::NoteOn {
                    timing: 0,
                    voice_id: None,
                    channel: 0,
                    note: 60,
                    velocity: 0.8,
                },
            ],
        );

        let (note, pan) = expect_poly_pan(&events);
        assert_eq!(note, 60);
        assert!(
            (pan - (-0.7)).abs() < f32::EPSILON,
            "expected pan -0.7, got {pan}"
        );
    }

    #[test]
    fn same_pitch_retrigger_emits_noteoff_before_noteon() {
        let mut plugin = init_plugin_with(StepOneParams::with_pulses(8), 44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Buffer covering 2 step boundaries (0.0 and 0.25).
        // At 120 BPM, 0.25 beats ≈ 5513 samples. Use 8192 samples.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 8192, vec![]);

        // Filter to NoteOn/NoteOff for note 60 only, then verify ordering:
        // expect [NoteOn, NoteOff, NoteOn] — NoteOff before second NoteOn.
        let note_60_events: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                NoteEvent::NoteOn { note: 60, .. } => Some("on"),
                NoteEvent::NoteOff { note: 60, .. } => Some("off"),
                _ => None,
            })
            .collect();

        assert!(
            note_60_events.len() >= 3,
            "expected [on, off, on], got {note_60_events:?}"
        );
        assert_eq!(
            &note_60_events[..3],
            &["on", "off", "on"],
            "same-pitch retrigger must emit NoteOff before second NoteOn"
        );
    }

    #[test]
    fn arp_cycles_across_buffers() {
        let mut plugin = init_plugin_with(StepOneParams::with_pulses(8), 44100.0);
        plugin.held_notes.note_on(60, 0.8); // C4
        plugin.held_notes.note_on(64, 0.8); // E4
        plugin.held_notes.note_on(67, 0.8); // G4

        // 3 separate buffers, each containing exactly one step boundary.
        // Step duration=1 (0.25 beats). Each buffer is 512 samples ≈ 0.023 beats.
        // Place each buffer to start exactly at a step boundary.
        let mut notes_emitted = Vec::new();

        for &start_beat in &[0.0, 0.25, 0.5] {
            let events = run_sequencer(&mut plugin, start_beat, 120.0, true, 512, vec![]);
            notes_emitted.extend(note_on_notes(&events));
        }

        assert_eq!(
            notes_emitted,
            vec![60, 64, 67],
            "arp should cycle C4→E4→G4 across buffers"
        );
    }

    #[test]
    fn pressure_modulates_output_velocity() {
        let mut plugin = init_plugin_with(StepOneParams::with_velocity(50.0), 44100.0);
        plugin.held_notes.note_on(60, 0.8);
        plugin.held_notes.set_pressure(60, 0.5);

        // output_velocity = input_velocity × pressure × (velocity_param / 100)
        //                  = 0.8 × 0.5 × 0.5 = 0.2
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        let (_, velocity) = expect_note_on(&events);
        assert!(
            (velocity - 0.2).abs() < 1e-6,
            "expected velocity 0.2 (0.8 × 0.5 × 0.5), got {velocity}"
        );
    }

    /// Helper: call Plugin::process() directly (not process_sequencer) to test
    /// the transport-guard branches in process(). Returns (ProcessStatus, sent events).
    fn run_process(plugin: &mut StepOne, context: &mut MockProcessContext) -> ProcessStatus {
        let mut buffer = Buffer::default();
        let mut aux = AuxiliaryBuffers {
            inputs: &mut [],
            outputs: &mut [],
        };
        plugin.process(&mut buffer, &mut aux, context)
    }

    #[test]
    fn missing_pos_beats_returns_keepalive() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Zeroed transport: pos_beats() returns None (no pos_beats, pos_seconds,
        // or pos_samples set). process() should return KeepAlive immediately.
        let mut context = MockProcessContext::new(44100.0, vec![]);
        let status = run_process(&mut plugin, &mut context);

        assert_eq!(status, ProcessStatus::KeepAlive);
        assert!(
            context.sent_events.is_empty(),
            "no events should be emitted when pos_beats is None"
        );
    }

    #[test]
    fn missing_tempo_returns_keepalive() {
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // Set tempo=None but leave pos_beats derivable from pos_samples+tempo.
        // Since tempo is None, pos_beats() also returns None (all fallback paths
        // require tempo). So this test actually hits the pos_beats None branch too.
        // To specifically test the tempo=None branch at line 117, we would need
        // pos_beats to be Some — but that field is pub(crate).
        // This still exercises the early-return path with tempo=None.
        let mut context = MockProcessContext::new(44100.0, vec![]).with_transport(true, None);
        let status = run_process(&mut plugin, &mut context);

        assert_eq!(status, ProcessStatus::KeepAlive);
        assert!(
            context.sent_events.is_empty(),
            "no events should be emitted when tempo is None"
        );
    }

    #[test]
    fn velocity_clipping_at_max() {
        // Velocity 100%, pressure 1.0, input velocity 1.0 → product = 1.0 (no clip).
        // But if we manually set pressure > 1.0 (via the expression stash, which
        // doesn't clamp), the product exceeds 1.0 and must be clipped.
        // Actually, PolyPressure values from MIDI are 0.0–1.0, but the stash
        // stores whatever f32 is passed. Let's use velocity_scale > 1.0 instead:
        // There's no param value >100%, but we can test via a known combination.
        //
        // Alternative: input_velocity=1.0, pressure=1.0, velocity_param=100%.
        // Product = 1.0 × 1.0 × 1.0 = 1.0 — exactly at the boundary.
        // We need product > 1.0. Since velocity param max is 100%, we'd need
        // pressure > 1.0 or input_velocity > 1.0.
        //
        // pressure is stored as f32 from PolyPressure which can be any value.
        // Let's set pressure = 1.5 directly.
        let mut plugin = init_plugin(44100.0);
        plugin.held_notes.note_on(60, 1.0);
        plugin.held_notes.set_pressure(60, 1.5);

        // output_velocity = 1.0 × 1.5 × 1.0 = 1.5 → clipped to 1.0.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 512, vec![]);

        let (_, velocity) = expect_note_on(&events);
        assert!(
            (velocity - 1.0).abs() < f32::EPSILON,
            "expected velocity clipped to 1.0, got {velocity}"
        );
    }

    #[test]
    fn inactive_step_skipped() {
        // E(1,8): only step 0 is active. Use a large buffer that covers steps 0 and 1.
        // Only step 0 should produce a NoteOn.
        let mut plugin = init_plugin_with(StepOneParams::with_pulses(1), 44100.0);
        plugin.held_notes.note_on(60, 0.8);

        // At 120 BPM with step_duration=1 (0.25 beats), boundaries at 0.0, 0.25.
        // 0.25 beats = 5512.5 samples. Use 8192 samples to cover both.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 8192, vec![]);

        let note_ons = note_on_notes(&events);
        assert_eq!(
            note_ons.len(),
            1,
            "only step 0 is active in E(1,8); expected 1 NoteOn, got {}: {:?}",
            note_ons.len(),
            note_ons
        );
    }

    #[test]
    fn overlapping_noteoffs_with_long_gate() {
        // All-pulse pattern + 200% gate length → NoteOffs extend past next step.
        let mut plugin = init_plugin_with(
            StepOneParams::with_pulses(8).and_gate_length(200.0),
            44100.0,
        );
        plugin.held_notes.note_on(60, 0.8); // C4
        plugin.held_notes.note_on(64, 0.8); // E4

        // E(8,8) all-pulse: distance to next pulse = 1 step = 0.25 beats.
        // gate_length=200% of distance (0.25 beats) = 0.5 beats.
        // NoteOn at beat 0.0 → NoteOff at beat 0.5.
        // NoteOn at beat 0.25 → NoteOff at beat 0.75.
        // Use 8192 samples (≈0.372 beats) to get two step boundaries.
        let events = run_sequencer(&mut plugin, 0.0, 120.0, true, 8192, vec![]);

        // With 8192 samples ≈ 0.372 beats, boundaries at 0.0 and 0.25.
        // Both NoteOns fire. NoteOff for C4 at 0.5, NoteOff for E4 at 0.75.
        // Both are beyond the buffer range, so they remain pending.
        let note_ons = note_on_notes(&events);
        assert!(
            note_ons.len() >= 2,
            "expected 2 NoteOns with long gate, got {}",
            note_ons.len()
        );

        // No NoteOff should have been emitted yet — both are beyond the buffer.
        let note_off_count = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::NoteOff { .. }))
            .count();
        assert_eq!(
            note_off_count, 0,
            "NoteOffs should still be pending with 200% gate"
        );

        // Both NoteOffs should be pending.
        assert!(!plugin.pending_offs.is_empty());
    }
}
