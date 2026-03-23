/// Maximum number of steps in a Euclidean pattern.
pub const MAX_STEPS: usize = 32;

/// A Euclidean rhythm pattern computed by the Bjorklund algorithm.
///
/// The pattern is stored in a fixed-size array. Only indices `0..steps` are
/// meaningful; entries beyond `steps` are always `false`.
pub struct EuclideanPattern {
    /// Fixed-size pattern array. Only indices 0..steps are meaningful.
    pattern: [bool; MAX_STEPS],
    /// Number of steps in the pattern (0..=32).
    steps: usize,
    /// Number of active pulses (0..=steps), after clamping.
    pulses: usize,
}

impl Default for EuclideanPattern {
    fn default() -> Self {
        Self::new()
    }
}

impl EuclideanPattern {
    /// Create an empty pattern (all rests, zero steps).
    pub fn new() -> Self {
        Self {
            pattern: [false; MAX_STEPS],
            steps: 0,
            pulses: 0,
        }
    }

    /// Return the number of steps in the current pattern.
    pub fn steps(&self) -> usize {
        self.steps
    }

    /// Return the number of active pulses (after clamping to steps).
    pub fn pulses(&self) -> usize {
        self.pulses
    }

    /// Return whether the given step is active (a pulse).
    /// Caller must ensure `step_index < self.steps`.
    pub fn is_active(&self, step_index: usize) -> bool {
        self.pattern[step_index]
    }

    /// Recompute the pattern using the Bjorklund algorithm.
    ///
    /// `pulses` is clamped to `min(pulses, steps)`. Entries at indices
    /// `>= steps` are cleared to `false`.
    pub fn recompute(&mut self, steps: usize, pulses: usize) {
        let clamped_pulses = pulses.min(steps);
        self.steps = steps;
        self.pulses = clamped_pulses;

        // Clear entire pattern first.
        self.pattern = [false; MAX_STEPS];

        if steps == 0 {
            return;
        }

        // Run Bjorklund and write results into self.pattern.
        bjorklund(&mut self.pattern, steps, clamped_pulses);
    }
}

/// Bjorklund's algorithm for distributing `pulses` as evenly as possible
/// across `steps` slots, using iterative group interleaving.
///
/// Writes the result into `output[0..steps]`. Assumes `output` is
/// pre-cleared to `false` and `pulses <= steps`.
fn bjorklund(output: &mut [bool; MAX_STEPS], steps: usize, pulses: usize) {
    if pulses == 0 || steps == 0 {
        return;
    }

    // Each "group" is a run of bools stored contiguously in a working buffer.
    // We track groups by their lengths and count, rather than using Vecs.
    //
    // Start: `pulses` groups of [true], then `(steps - pulses)` groups of [false].
    // We store all group contents flat in `buf` and track each group's length
    // in `group_len` (all groups in the "front" section share one length, and
    // all groups in the "remainder" section share another).

    // Flat working buffer — holds the interleaved pattern during construction.
    let mut buf = [false; MAX_STEPS];
    // Initialize: pulses ones followed by rests.
    for slot in buf.iter_mut().take(pulses) {
        *slot = true;
    }

    // `front_count`  = number of groups in the larger partition
    // `front_len`    = length of each group in the larger partition
    // `rem_count`    = number of groups in the smaller partition (remainder)
    // `rem_len`      = length of each group in the smaller partition
    let mut front_count = pulses;
    let mut front_len: usize = 1;
    let mut rem_count = steps - pulses;
    let mut rem_len: usize = 1;

    // Iteratively interleave: append one remainder group to each front group,
    // then the leftover groups become the new remainder. Stop when the
    // remainder has fewer than 2 groups (nothing left to distribute).
    loop {
        if rem_count == 0 {
            break;
        }

        buf = interleave_pass(&buf, front_count, front_len, rem_count, rem_len);

        // After interleaving, the merged groups each have length
        // (front_len + rem_len). The number of merged groups is
        // min(front_count, rem_count). The leftover groups keep their
        // original length.
        let pairs = front_count.min(rem_count);
        let new_front_len = front_len + rem_len;

        // The leftover groups come from whichever partition was larger.
        let leftover_count = front_count.abs_diff(rem_count);
        let leftover_len = if front_count > rem_count {
            front_len
        } else {
            rem_len
        };

        front_count = pairs;
        front_len = new_front_len;
        rem_count = leftover_count;
        rem_len = leftover_len;

        // Stop when fewer than 2 remainder groups — nothing to redistribute.
        if rem_count <= 1 {
            break;
        }
    }

    // Copy result to output.
    output[..steps].copy_from_slice(&buf[..steps]);
}

/// One pass of the Bjorklund interleave: pair each front group with a
/// remainder group, then append any leftover groups at the end.
///
/// Returns a new buffer with the interleaved result.
fn interleave_pass(
    buf: &[bool; MAX_STEPS],
    front_count: usize,
    front_len: usize,
    rem_count: usize,
    rem_len: usize,
) -> [bool; MAX_STEPS] {
    let mut result = [false; MAX_STEPS];
    let mut write_pos = 0;

    // The front groups occupy buf[0 .. front_count * front_len].
    // The remainder groups occupy buf[front_count * front_len ..].
    let rem_start = front_count * front_len;

    let pairs = front_count.min(rem_count);

    for pair_index in 0..pairs {
        // Copy one front group.
        let f_start = pair_index * front_len;
        result[write_pos..write_pos + front_len]
            .copy_from_slice(&buf[f_start..f_start + front_len]);
        write_pos += front_len;

        // Copy one remainder group.
        let r_start = rem_start + pair_index * rem_len;
        result[write_pos..write_pos + rem_len].copy_from_slice(&buf[r_start..r_start + rem_len]);
        write_pos += rem_len;
    }

    // Append leftover front groups (if front_count > rem_count).
    for group_index in pairs..front_count {
        let f_start = group_index * front_len;
        result[write_pos..write_pos + front_len]
            .copy_from_slice(&buf[f_start..f_start + front_len]);
        write_pos += front_len;
    }

    // Append leftover remainder groups (if rem_count > front_count).
    for group_index in pairs..rem_count {
        let r_start = rem_start + group_index * rem_len;
        result[write_pos..write_pos + rem_len].copy_from_slice(&buf[r_start..r_start + rem_len]);
        write_pos += rem_len;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: compute a pattern and return the active slice as a Vec<bool> for easy comparison.
    fn compute(steps: usize, pulses: usize) -> Vec<bool> {
        let mut pat = EuclideanPattern::new();
        pat.recompute(steps, pulses);
        pat.pattern[..steps].to_vec()
    }

    #[test]
    fn e_3_8_tresillo() {
        // E(3,8) — the tresillo rhythm: X..X..X.
        assert_eq!(
            compute(8, 3),
            vec![true, false, false, true, false, false, true, false]
        );
    }

    #[test]
    fn e_5_8_cinquillo() {
        // E(5,8) — the cinquillo rhythm: X.XX.XX.
        assert_eq!(
            compute(8, 5),
            vec![true, false, true, true, false, true, true, false]
        );
    }

    #[test]
    fn e_0_8_all_rests() {
        // E(0,8) — no pulses, all rests.
        assert_eq!(compute(8, 0), vec![false; 8]);
    }

    #[test]
    fn e_8_8_all_pulses() {
        // E(8,8) — every step is a pulse.
        assert_eq!(compute(8, 8), vec![true; 8]);
    }

    #[test]
    fn e_1_1_single() {
        // E(1,1) — single step, single pulse.
        assert_eq!(compute(1, 1), vec![true]);
    }

    #[test]
    fn e_5_12() {
        // E(5,12)
        assert_eq!(
            compute(12, 5),
            vec![true, false, false, true, false, true, false, false, true, false, true, false]
        );
    }

    #[test]
    fn e_3_4_cumbia() {
        // E(3,4) — cumbia rhythm: X.XX
        assert_eq!(compute(4, 3), vec![true, false, true, true]);
    }

    #[test]
    fn e_4_12() {
        // E(4,12) — evenly spaced: X..X..X..X..
        assert_eq!(
            compute(12, 4),
            vec![true, false, false, true, false, false, true, false, false, true, false, false]
        );
    }

    #[test]
    fn pulses_clamped() {
        // E(10,4) should behave as E(4,4) — pulses clamped to steps.
        assert_eq!(compute(4, 10), vec![true; 4]);
    }

    #[test]
    fn no_true_beyond_steps() {
        // After computing E(3,8), indices 8..32 must all be false.
        let mut pat = EuclideanPattern::new();
        pat.recompute(8, 3);
        for index in 8..MAX_STEPS {
            assert!(
                !pat.pattern[index],
                "expected false at index {index}, got true"
            );
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn pulse_count_and_bounds(steps in 1_usize..=MAX_STEPS, pulses in 0_usize..=MAX_STEPS) {
            let mut pat = EuclideanPattern::new();
            pat.recompute(steps, pulses);

            let expected_pulses = pulses.min(steps);

            // Count of true in the active region must equal clamped pulses.
            let active_count = pat.pattern[..steps].iter().filter(|&&b| b).count();
            prop_assert_eq!(active_count, expected_pulses,
                "E({},{}) expected {} pulses, got {}", pulses, steps, expected_pulses, active_count);

            // No true beyond the active region.
            for index in steps..MAX_STEPS {
                prop_assert!(!pat.pattern[index],
                    "E({},{}) unexpected true at index {}", pulses, steps, index);
            }
        }
    }
}
