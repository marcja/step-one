//! Pending NoteOff management for the Euclidean arpeggiator.
//!
//! Tracks output NoteOff events that need to fire at future beat positions.
//! Fixed-size array (no heap allocation) with a conservative capacity of 8.

/// Maximum number of simultaneously pending NoteOff events.
/// At 400% gate length with all-pulse patterns, at most 4 notes overlap.
/// 8 is a conservative upper bound.
pub const MAX_PENDING: usize = 8;

/// A scheduled NoteOff event.
#[derive(Clone, Copy, Debug)]
pub struct PendingNoteOff {
    pub note: u8,
    pub channel: u8,
    pub voice_id: Option<i32>,
    /// Beat position at which this NoteOff should fire.
    pub off_at_beat: f64,
}

/// Fixed-size list of pending NoteOff events.
pub struct PendingNoteOffs {
    entries: [Option<PendingNoteOff>; MAX_PENDING],
}

impl Default for PendingNoteOffs {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingNoteOffs {
    /// Create an empty pending list.
    pub fn new() -> Self {
        Self {
            entries: [None; MAX_PENDING],
        }
    }

    /// Schedule a NoteOff at the given beat position.
    /// Silently drops if at capacity.
    pub fn add(&mut self, entry: PendingNoteOff) {
        for slot in &mut self.entries {
            if slot.is_none() {
                *slot = Some(entry);
                return;
            }
        }
    }

    /// Remove and return the pending NoteOff for a specific note, if any.
    /// Used for same-pitch retrigger: emit the old NoteOff before the new NoteOn.
    pub fn take_by_note(&mut self, note: u8) -> Option<PendingNoteOff> {
        for slot in &mut self.entries {
            if let Some(entry) = slot {
                if entry.note == note {
                    return slot.take();
                }
            }
        }
        None
    }

    /// Collect all pending NoteOffs whose off_at_beat falls within
    /// [start_beat, end_beat). Returns entries in a fixed-size array
    /// and clears them from the pending list.
    pub fn take_due(
        &mut self,
        start_beat: f64,
        end_beat: f64,
    ) -> ([Option<PendingNoteOff>; MAX_PENDING], usize) {
        let mut result = [None; MAX_PENDING];
        let mut count = 0;
        for slot in &mut self.entries {
            if let Some(entry) = slot {
                if entry.off_at_beat >= start_beat
                    && entry.off_at_beat < end_beat
                    && count < MAX_PENDING
                {
                    result[count] = slot.take();
                    count += 1;
                }
            }
        }
        (result, count)
    }

    /// Remove and return all pending NoteOffs (for transport stop/jump flush).
    pub fn flush_all(&mut self) -> ([Option<PendingNoteOff>; MAX_PENDING], usize) {
        let mut result = [None; MAX_PENDING];
        let mut count = 0;
        for slot in &mut self.entries {
            if slot.is_some() && count < MAX_PENDING {
                result[count] = slot.take();
                count += 1;
            }
        }
        (result, count)
    }

    /// Clear all pending entries.
    pub fn clear(&mut self) {
        self.entries = [None; MAX_PENDING];
    }

    /// Returns true if no NoteOffs are pending.
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|s| s.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(note: u8, off_at_beat: f64) -> PendingNoteOff {
        PendingNoteOff {
            note,
            channel: 0,
            voice_id: None,
            off_at_beat,
        }
    }

    #[test]
    fn empty_by_default() {
        let pending = PendingNoteOffs::new();
        assert!(pending.is_empty());
    }

    #[test]
    fn add_and_flush() {
        let mut pending = PendingNoteOffs::new();
        pending.add(make_entry(60, 1.0));
        pending.add(make_entry(64, 2.0));
        assert!(!pending.is_empty());

        let (flushed, count) = pending.flush_all();
        assert_eq!(count, 2);
        assert_eq!(flushed[0].unwrap().note, 60);
        assert_eq!(flushed[1].unwrap().note, 64);
        assert!(pending.is_empty());
    }

    #[test]
    fn take_by_note_removes_matching() {
        let mut pending = PendingNoteOffs::new();
        pending.add(make_entry(60, 1.0));
        pending.add(make_entry(64, 2.0));

        let taken = pending.take_by_note(60);
        assert_eq!(taken.unwrap().note, 60);

        // Only 64 remains.
        let (flushed, count) = pending.flush_all();
        assert_eq!(count, 1);
        assert_eq!(flushed[0].unwrap().note, 64);
    }

    #[test]
    fn take_by_note_returns_none_if_absent() {
        let mut pending = PendingNoteOffs::new();
        pending.add(make_entry(60, 1.0));
        assert!(pending.take_by_note(72).is_none());
    }

    #[test]
    fn take_due_collects_in_range() {
        let mut pending = PendingNoteOffs::new();
        pending.add(make_entry(60, 0.5));
        pending.add(make_entry(64, 1.5));
        pending.add(make_entry(67, 2.5));

        // Range [0.0, 2.0) should capture notes at 0.5 and 1.5.
        let (due, count) = pending.take_due(0.0, 2.0);
        assert_eq!(count, 2);
        assert_eq!(due[0].unwrap().note, 60);
        assert_eq!(due[1].unwrap().note, 64);

        // Only 67 at beat 2.5 remains.
        let (flushed, count) = pending.flush_all();
        assert_eq!(count, 1);
        assert_eq!(flushed[0].unwrap().note, 67);
    }

    #[test]
    fn clear_removes_all() {
        let mut pending = PendingNoteOffs::new();
        pending.add(make_entry(60, 1.0));
        pending.add(make_entry(64, 2.0));
        pending.clear();
        assert!(pending.is_empty());
    }
}
