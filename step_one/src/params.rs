use nih_plug::prelude::*;

/// User-controllable parameters for StepOne.
///
/// All sequencer state (held notes, arp index, pending NoteOffs, expression
/// stashes) lives on the plugin struct, not here. Params holds only what the
/// user and host can see and automate.
///
/// No smoothing on any parameter — StepOne reads params at event boundaries,
/// not per-sample.
#[derive(Params)]
pub struct StepOneParams {
    /// Number of steps in the Euclidean pattern (1–32).
    #[id = "steps"]
    pub steps: IntParam,

    /// Number of active pulses in the pattern (0–32).
    /// Clamped to min(pulses, steps) wherever it is read.
    #[id = "pulses"]
    pub pulses: IntParam,

    /// Length of each step in sixteenth notes (1–16).
    #[id = "step_dur"]
    pub step_duration: IntParam,

    /// Gate duration as percentage of step duration (0–400%).
    /// 0% = mute, 100% = legato, >100% = overlapping notes.
    #[id = "gate_len"]
    pub gate_length: FloatParam,

    /// Output velocity scale (0–100%).
    /// output_velocity = input_velocity × pressure × (velocity / 100).
    #[id = "velocity"]
    pub velocity: FloatParam,
}

impl Default for StepOneParams {
    fn default() -> Self {
        Self {
            steps: Self::build_steps(8),
            pulses: Self::build_pulses(4),
            step_duration: Self::build_step_duration(1),
            gate_length: Self::build_gate_length(100.0),
            velocity: Self::build_velocity(100.0),
        }
    }
}

impl StepOneParams {
    /// Build the steps IntParam with the given default value.
    /// Shared between `Default` and test helpers.
    fn build_steps(default: i32) -> IntParam {
        IntParam::new("Steps", default, IntRange::Linear { min: 1, max: 32 }).with_unit(" steps")
    }

    /// Build the pulses IntParam with the given default value.
    /// Shared between `Default` and test helpers.
    fn build_pulses(default: i32) -> IntParam {
        IntParam::new("Pulses", default, IntRange::Linear { min: 0, max: 32 }).with_unit(" pulses")
    }

    /// Build the step_duration IntParam with the given default value.
    /// Shared between `Default` and test helpers.
    fn build_step_duration(default: i32) -> IntParam {
        IntParam::new(
            "Step Duration",
            default,
            IntRange::Linear { min: 1, max: 16 },
        )
        .with_unit(" 16ths")
    }

    /// Build the gate_length FloatParam with the given default value.
    /// Shared between `Default` and test helpers.
    fn build_gate_length(default: f32) -> FloatParam {
        FloatParam::new(
            "Gate Length",
            default,
            FloatRange::Linear {
                min: 0.0,
                max: 400.0,
            },
        )
        .with_unit(" %")
        .with_step_size(1.0)
    }

    /// Build the velocity FloatParam with the given default value.
    /// Shared between `Default` and test helpers.
    fn build_velocity(default: f32) -> FloatParam {
        FloatParam::new(
            "Velocity",
            default,
            FloatRange::Linear {
                min: 0.0,
                max: 100.0,
            },
        )
        .with_unit(" %")
        .with_step_size(1.0)
    }
}

#[cfg(test)]
impl StepOneParams {
    /// Create params with a custom steps default for testing.
    pub fn with_steps(n: i32) -> Self {
        let mut params = Self::default();
        params.steps = Self::build_steps(n);
        params
    }

    /// Create params with a custom pulses default for testing.
    pub fn with_pulses(n: i32) -> Self {
        let mut params = Self::default();
        params.pulses = Self::build_pulses(n);
        params
    }

    /// Create params with a custom step_duration default for testing.
    pub fn with_step_duration(n: i32) -> Self {
        let mut params = Self::default();
        params.step_duration = Self::build_step_duration(n);
        params
    }

    /// Create params with a custom gate_length default for testing.
    pub fn with_gate_length(pct: f32) -> Self {
        let mut params = Self::default();
        params.gate_length = Self::build_gate_length(pct);
        params
    }

    /// Create params with a custom velocity default for testing.
    pub fn with_velocity(pct: f32) -> Self {
        let mut params = Self::default();
        params.velocity = Self::build_velocity(pct);
        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify each param's default value is within its declared min/max.
    #[test]
    fn param_defaults_in_range() {
        let params = StepOneParams::default();

        let steps = params.steps.value();
        assert!(
            (1..=32).contains(&steps),
            "steps default {steps} out of range"
        );

        let pulses = params.pulses.value();
        assert!(
            (0..=32).contains(&pulses),
            "pulses default {pulses} out of range"
        );

        let dur = params.step_duration.value();
        assert!(
            (1..=16).contains(&dur),
            "step_duration default {dur} out of range"
        );

        let gate = params.gate_length.value();
        assert!(
            (0.0..=400.0).contains(&gate),
            "gate_length default {gate} out of range"
        );

        let vel = params.velocity.value();
        assert!(
            (0.0..=100.0).contains(&vel),
            "velocity default {vel} out of range"
        );
    }

    /// Verify the exact default values match the design doc.
    #[test]
    fn param_defaults_correct() {
        let params = StepOneParams::default();

        assert_eq!(params.steps.value(), 8);
        assert_eq!(params.pulses.value(), 4);
        assert_eq!(params.step_duration.value(), 1);
        assert!((params.gate_length.value() - 100.0).abs() < f32::EPSILON);
        assert!((params.velocity.value() - 100.0).abs() < f32::EPSILON);
    }

    /// Verify that default values survive a normalize→unnormalize round-trip.
    #[test]
    fn param_defaults_survive_normalize_round_trip() {
        let params = StepOneParams::default();

        // Steps: Linear 1..32, default 8.
        let s_norm = params.steps.preview_normalized(8);
        let s_plain = params.steps.preview_plain(s_norm);
        assert_eq!(s_plain, 8, "steps round-trip: expected 8, got {s_plain}");

        // Pulses: Linear 0..32, default 4.
        let p_norm = params.pulses.preview_normalized(4);
        let p_plain = params.pulses.preview_plain(p_norm);
        assert_eq!(p_plain, 4, "pulses round-trip: expected 4, got {p_plain}");

        // Step Duration: Linear 1..16, default 1.
        let d_norm = params.step_duration.preview_normalized(1);
        let d_plain = params.step_duration.preview_plain(d_norm);
        assert_eq!(
            d_plain, 1,
            "step_duration round-trip: expected 1, got {d_plain}"
        );

        // Gate Length: Linear 0..400, default 100.
        let g_norm = params.gate_length.preview_normalized(100.0);
        let g_plain = params.gate_length.preview_plain(g_norm);
        assert!(
            (g_plain - 100.0).abs() < 0.01,
            "gate_length round-trip: expected 100.0, got {g_plain}"
        );

        // Velocity: Linear 0..100, default 100.
        let v_norm = params.velocity.preview_normalized(100.0);
        let v_plain = params.velocity.preview_plain(v_norm);
        assert!(
            (v_plain - 100.0).abs() < 0.01,
            "velocity round-trip: expected 100.0, got {v_plain}"
        );
    }
}
