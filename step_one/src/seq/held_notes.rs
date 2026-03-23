/// Maximum number of simultaneously held MIDI notes.
/// Matches the MIDI note range (0..=127).
pub const MAX_HELD: usize = 128;

/// A single held note with its expression data.
#[derive(Clone, Copy, Debug)]
pub struct HeldNote {
    pub note: u8,
    pub velocity: f32,
    /// Poly aftertouch pressure. Defaults to 1.0 on NoteOn.
    pub pressure: f32,
    /// Poly pan (bipolar). Defaults to 0.0 (center) on NoteOn.
    pub pan: f32,
}

/// Sorted list of currently held MIDI notes with arp index cycling
/// and expression stash for pre-NoteOn PolyPressure/PolyPan events.
///
/// All storage is fixed-size arrays — no heap allocation.
pub struct HeldNotes {
    notes: [HeldNote; MAX_HELD],
    len: usize,
    arp_index: usize,
    /// Stash for PolyPressure events arriving before their NoteOn.
    /// Indexed by MIDI note number (0..=127).
    pressure_stash: [Option<f32>; MAX_HELD],
    /// Stash for PolyPan events arriving before their NoteOn.
    /// Indexed by MIDI note number (0..=127).
    pan_stash: [Option<f32>; MAX_HELD],
}

impl Default for HeldNotes {
    fn default() -> Self {
        Self::new()
    }
}

impl HeldNotes {
    /// Create an empty held note list with cleared stashes.
    pub fn new() -> Self {
        Self {
            notes: [HeldNote {
                note: 0,
                velocity: 0.0,
                pressure: 1.0,
                pan: 0.0,
            }; MAX_HELD],
            len: 0,
            arp_index: 0,
            pressure_stash: [None; MAX_HELD],
            pan_stash: [None; MAX_HELD],
        }
    }

    /// Insert a note in sorted position (ascending by note number).
    /// Duplicates are ignored. After insertion, apply any stashed
    /// pressure/pan values and clear those stash entries.
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        // Single scan: find existing note or the sorted insertion point.
        let insert_at = match self.find_or_insert_index(note) {
            Ok(_) => return, // duplicate — ignore
            Err(idx) => idx,
        };

        // List is full — cannot insert.
        if self.len >= MAX_HELD {
            return;
        }

        // Shift elements right to make room at insert_at.
        self.notes.copy_within(insert_at..self.len, insert_at + 1);

        // Insert with default expression values.
        self.notes[insert_at] = HeldNote {
            note,
            velocity,
            pressure: 1.0,
            pan: 0.0,
        };
        self.len += 1;

        // Apply stashed expression data if present, then clear stash.
        let note_idx = note as usize;
        if let Some(pressure) = self.pressure_stash[note_idx].take() {
            self.notes[insert_at].pressure = pressure;
        }
        if let Some(pan) = self.pan_stash[note_idx].take() {
            self.notes[insert_at].pan = pan;
        }
    }

    /// Remove a note from the list. Adjusts arp_index if needed
    /// to avoid skipping notes. Clears stash entries for this note.
    pub fn note_off(&mut self, note: u8) {
        let Ok(remove_at) = self.find_or_insert_index(note) else {
            return;
        };

        // Shift elements left to fill the gap.
        self.notes.copy_within(remove_at + 1..self.len, remove_at);
        self.len -= 1;

        // Adjust arp_index: if the removed element was before the
        // current index, shift index down to keep pointing at the
        // same note. If index is now past the end, wrap to 0.
        if self.len == 0 {
            self.arp_index = 0;
        } else if remove_at < self.arp_index {
            self.arp_index -= 1;
        } else if self.arp_index >= self.len {
            self.arp_index = 0;
        }

        // Clear stash entries for this note.
        let note_idx = note as usize;
        self.pressure_stash[note_idx] = None;
        self.pan_stash[note_idx] = None;
    }

    /// Update pressure for a held note, or stash it if the note
    /// is not yet held (Bitwig may send PolyPressure before NoteOn).
    pub fn set_pressure(&mut self, note: u8, pressure: f32) {
        if let Ok(idx) = self.find_or_insert_index(note) {
            self.notes[idx].pressure = pressure;
        } else {
            self.pressure_stash[note as usize] = Some(pressure);
        }
    }

    /// Update pan for a held note, or stash it if the note
    /// is not yet held (Bitwig may send PolyPan before NoteOn).
    pub fn set_pan(&mut self, note: u8, pan: f32) {
        if let Ok(idx) = self.find_or_insert_index(note) {
            self.notes[idx].pan = pan;
        } else {
            self.pan_stash[note as usize] = Some(pan);
        }
    }

    /// Return the note at the current arp index, then advance.
    /// Returns `None` if the list is empty.
    pub fn next_note(&mut self) -> Option<&HeldNote> {
        if self.len == 0 {
            return None;
        }
        let current = self.arp_index;
        // Advance arp_index, wrapping around to the start.
        self.arp_index = (self.arp_index + 1) % self.len;
        Some(&self.notes[current])
    }

    /// Returns true if no notes are held.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of currently held notes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Reset all state: remove all notes, reset arp index,
    /// clear both expression stash arrays.
    pub fn clear(&mut self) {
        self.len = 0;
        self.arp_index = 0;
        self.pressure_stash = [None; MAX_HELD];
        self.pan_stash = [None; MAX_HELD];
    }

    /// Search the sorted list for a note. Returns `Ok(index)` if found,
    /// or `Err(insertion_index)` if not found (where it would be inserted
    /// to maintain sorted order). Single scan with early exit.
    fn find_or_insert_index(&self, note: u8) -> Result<usize, usize> {
        for i in 0..self.len {
            if self.notes[i].note == note {
                return Ok(i);
            }
            // Sorted ascending — if we've passed where the note would be,
            // this is the insertion point.
            if self.notes[i].note > note {
                return Err(i);
            }
        }
        Err(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_none() {
        let mut held = HeldNotes::new();
        assert!(held.next_note().is_none());
    }

    #[test]
    fn single_note_repeats() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.8);
        for _ in 0..5 {
            let note = held.next_note().unwrap();
            assert_eq!(note.note, 60);
        }
    }

    #[test]
    fn ascending_order() {
        let mut held = HeldNotes::new();
        // Insert out of order: E4, C4, G4
        held.note_on(64, 0.5);
        held.note_on(60, 0.5);
        held.note_on(67, 0.5);

        assert_eq!(held.len(), 3);
        // Verify sorted order by reading arp sequence.
        let notes: Vec<u8> = (0..3).map(|_| held.next_note().unwrap().note).collect();
        assert_eq!(notes, vec![60, 64, 67]);
    }

    #[test]
    fn arp_index_wraps() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.note_on(64, 0.5);
        held.note_on(67, 0.5);

        // 4 calls should wrap: [60, 64, 67, 60]
        let notes: Vec<u8> = (0..4).map(|_| held.next_note().unwrap().note).collect();
        assert_eq!(notes, vec![60, 64, 67, 60]);
    }

    #[test]
    fn remove_before_index_adjusts() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5); // C4 at index 0
        held.note_on(64, 0.5); // E4 at index 1
        held.note_on(67, 0.5); // G4 at index 2

        // Advance arp_index twice: returns C4 (idx 0), E4 (idx 1).
        // After two calls, arp_index is 2.
        held.next_note();
        held.next_note();

        // Remove C4 (index 0). List becomes [E4, G4].
        // arp_index was 2, should adjust to 1 (shifted down).
        held.note_off(60);

        // Next note should be G4 (index 1 of [E4, G4]).
        let note = held.next_note().unwrap();
        assert_eq!(note.note, 67);
    }

    #[test]
    fn remove_only_note() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.note_off(60);

        assert!(held.is_empty());
        assert!(held.next_note().is_none());
    }

    #[test]
    fn noteoff_absent_noop() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        // NoteOff for a note not in the list — should not panic or change state.
        held.note_off(64);
        assert_eq!(held.len(), 1);
        assert_eq!(held.next_note().unwrap().note, 60);
    }

    #[test]
    fn duplicate_noteon_ignored() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.8);
        held.note_on(60, 0.5); // duplicate — should be ignored
        assert_eq!(held.len(), 1);
        // Velocity should remain from the first NoteOn.
        assert_eq!(held.next_note().unwrap().velocity, 0.8);
    }

    #[test]
    fn velocity_stored() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.8);
        let note = held.next_note().unwrap();
        assert!((note.velocity - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn index_wraps_on_shrink() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.note_on(64, 0.5);
        held.note_on(67, 0.5);

        // Advance arp_index to 2 (pointing at G4).
        held.next_note(); // returns C4, index -> 1
        held.next_note(); // returns E4, index -> 2

        // Remove the last note (G4 at index 2).
        // arp_index was 2, now len=2, so it wraps to 0.
        held.note_off(67);

        // Next note should be C4 (index 0 of [C4, E4]).
        let note = held.next_note().unwrap();
        assert_eq!(note.note, 60);
    }

    #[test]
    fn pressure_defaults_to_one() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_pressure_updates() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.set_pressure(60, 0.3);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn set_pressure_absent_stashes() {
        let mut held = HeldNotes::new();
        // No NoteOn yet — pressure should be stashed.
        held.set_pressure(60, 0.7);
        assert!(held.is_empty());
        // Now NoteOn — stashed pressure should be applied.
        held.note_on(60, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn pressure_stash_applied_on_noteon() {
        let mut held = HeldNotes::new();
        held.set_pressure(60, 0.5);
        held.note_on(60, 0.8);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn pressure_resets_on_readd() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.set_pressure(60, 0.3);
        held.note_off(60);
        // Re-add the same note — pressure should reset to 1.0.
        held.note_on(60, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pan_defaults_to_center() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pan - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_pan_updates() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.set_pan(60, -0.7);
        let note = held.next_note().unwrap();
        assert!((note.pan - (-0.7)).abs() < f32::EPSILON);
    }

    #[test]
    fn pan_stash_applied_on_noteon() {
        let mut held = HeldNotes::new();
        held.set_pan(60, -0.5);
        held.note_on(60, 0.8);
        let note = held.next_note().unwrap();
        assert!((note.pan - (-0.5)).abs() < f32::EPSILON);
    }

    #[test]
    fn pan_resets_on_readd() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.set_pan(60, 0.8);
        held.note_off(60);
        // Re-add — pan should reset to 0.0.
        held.note_on(60, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pan - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clear_resets_everything() {
        let mut held = HeldNotes::new();
        held.note_on(60, 0.5);
        held.note_on(64, 0.5);
        held.set_pressure(60, 0.3);
        // Stash a value for an absent note.
        held.set_pressure(72, 0.9);
        held.set_pan(72, -0.5);

        held.clear();

        assert!(held.is_empty());
        assert_eq!(held.len(), 0);
        assert!(held.next_note().is_none());

        // Re-add note 72 — stashed values should have been cleared,
        // so defaults (pressure=1.0, pan=0.0) apply.
        held.note_on(72, 0.5);
        let note = held.next_note().unwrap();
        assert!((note.pressure - 1.0).abs() < f32::EPSILON);
        assert!((note.pan - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn note_on_at_capacity_ignored() {
        // Fill to MAX_HELD (128), then try one more — should be silently ignored.
        let mut held = HeldNotes::new();
        for note in 0..MAX_HELD as u8 {
            held.note_on(note, 0.5);
        }
        assert_eq!(held.len(), MAX_HELD);

        // Attempt to add note 128 (would be index 128 if u8 could hold it).
        // Since MAX_HELD == 128 and MIDI notes are 0..=127, this is already
        // at capacity. Verify the guard works by removing one, re-adding, then
        // filling again.
        held.note_off(64);
        assert_eq!(held.len(), MAX_HELD - 1);
        held.note_on(64, 0.5);
        assert_eq!(held.len(), MAX_HELD);

        // Now at capacity again — a new note_on for an unused pitch is impossible
        // since all 128 MIDI notes are held. Duplicate should be ignored.
        held.note_on(60, 0.9);
        assert_eq!(held.len(), MAX_HELD);
    }

    #[test]
    fn remove_at_arp_index_wraps_to_zero() {
        // When the note at the current arp_index is removed and
        // arp_index >= len, it should wrap to 0.
        let mut held = HeldNotes::new();
        held.note_on(60, 0.8);
        held.note_on(64, 0.8);
        held.note_on(67, 0.8);

        // Advance arp_index: next_note() returns notes[0]=60 (index becomes 1),
        // then notes[1]=64 (index becomes 2).
        held.next_note();
        held.next_note();
        // arp_index is now 2 (pointing at note 67, the last element).

        // Remove note 67 (at index 2 == arp_index). len drops to 2.
        // arp_index (2) >= len (2), so it should wrap to 0.
        held.note_off(67);
        assert_eq!(held.len(), 2);

        // Next note should be notes[0] = 60, confirming arp_index wrapped to 0.
        let note = held.next_note().unwrap();
        assert_eq!(note.note, 60);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// A random operation on a HeldNotes list.
    #[derive(Clone, Debug)]
    enum Op {
        NoteOn { note: u8, velocity: f32 },
        NoteOff { note: u8 },
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..=127, 0.0f32..=1.0).prop_map(|(note, velocity)| Op::NoteOn { note, velocity }),
            (0u8..=127).prop_map(|note| Op::NoteOff { note }),
        ]
    }

    proptest! {
        #[test]
        fn invariants_hold_after_random_ops(ops in proptest::collection::vec(op_strategy(), 1..200)) {
            let mut held = HeldNotes::new();

            for op in &ops {
                match *op {
                    Op::NoteOn { note, velocity } => held.note_on(note, velocity),
                    Op::NoteOff { note } => held.note_off(note),
                }

                // Invariant 1: len <= MAX_HELD
                prop_assert!(held.len() <= MAX_HELD);

                // Invariant 2: list is sorted ascending by note number
                for i in 1..held.len() {
                    prop_assert!(
                        held.notes[i - 1].note < held.notes[i].note,
                        "list not sorted at index {}: {} >= {}",
                        i,
                        held.notes[i - 1].note,
                        held.notes[i].note,
                    );
                }

                // Invariant 3: all pressures in [0.0, 1.0]
                // (only check held entries — pressure defaults to 1.0)
                for i in 0..held.len() {
                    prop_assert!(
                        (0.0..=1.0).contains(&held.notes[i].pressure),
                        "pressure out of range at index {}: {}",
                        i,
                        held.notes[i].pressure,
                    );
                }

                // Invariant 4: all pans in [-1.0, 1.0]
                // (only check held entries — pan defaults to 0.0)
                for i in 0..held.len() {
                    prop_assert!(
                        (-1.0..=1.0).contains(&held.notes[i].pan),
                        "pan out of range at index {}: {}",
                        i,
                        held.notes[i].pan,
                    );
                }
            }
        }
    }
}
