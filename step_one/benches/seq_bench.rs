use criterion::{black_box, criterion_group, criterion_main, Criterion};

use step_one::seq::clock;
use step_one::seq::euclidean::EuclideanPattern;
use step_one::seq::held_notes::HeldNotes;
use step_one::seq::pending::{PendingNoteOff, PendingNoteOffs};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the beat position at the end of a buffer, given sample count, rate, and tempo.
fn buffer_end_beat(num_samples: f64, sample_rate: f64, tempo: f64) -> f64 {
    let beats_per_sample = tempo / (60.0 * sample_rate);
    num_samples * beats_per_sample
}

/// Create a HeldNotes list with the C-major scale (C4–C5): 8 notes.
fn held_c_major() -> HeldNotes {
    let mut held = HeldNotes::new();
    for &note in &[60, 62, 64, 65, 67, 69, 71, 72] {
        held.note_on(note, 0.8);
    }
    held
}

fn make_pending(note: u8, off_at_beat: f64) -> PendingNoteOff {
    PendingNoteOff {
        note,
        channel: 0,
        voice_id: None,
        off_at_beat,
    }
}

/// Mirrors the process() hot path: collect due NoteOffs, scan boundaries,
/// then for each active step: retrigger check, arp lookup, schedule NoteOff.
fn run_pipeline(
    pattern: &EuclideanPattern,
    held: &mut HeldNotes,
    buffer_end: f64,
    sample_rate: f32,
    tempo: f64,
    step_duration: u32,
    total_steps: u32,
) {
    let step_length_beats = step_duration as f64 / 4.0;
    let mut pending = PendingNoteOffs::new();

    black_box(pending.take_due(0.0, buffer_end));

    let boundaries = clock::find_boundaries(
        black_box(0.0),
        black_box(buffer_end),
        black_box(sample_rate),
        black_box(tempo),
        black_box(step_duration),
        black_box(total_steps),
    );

    for boundary in boundaries.iter() {
        if pattern.is_active(boundary.step_index) {
            let note = held.next_note().unwrap();
            black_box(pending.take_by_note(note.note));
            // Gate length relative to distance to next pulse (matches plugin emit_gates).
            let distance = pattern.distance_to_next_pulse(boundary.step_index);
            pending.add(make_pending(
                note.note,
                boundary.beat_position + distance as f64 * step_length_beats,
            ));
        }
    }
    black_box(&pending);
}

// ---------------------------------------------------------------------------
// Group 1: component — micro-benchmarks for individual sequencer primitives
// ---------------------------------------------------------------------------

fn bjorklund_worst_case(c: &mut Criterion) {
    let mut group = c.benchmark_group("component");

    // Worst case: E(16,32) — maximum steps with non-trivial distribution.
    group.bench_function("bjorklund_e16_32", |b| {
        let mut pattern = EuclideanPattern::new();
        b.iter(|| {
            pattern.recompute(black_box(32), black_box(16));
            black_box(&pattern);
        });
    });

    // Typical case: E(4,8) — the default pattern.
    group.bench_function("bjorklund_e4_8", |b| {
        let mut pattern = EuclideanPattern::new();
        b.iter(|| {
            pattern.recompute(black_box(8), black_box(4));
            black_box(&pattern);
        });
    });

    group.finish();
}

fn held_notes_churn(c: &mut Criterion) {
    let mut group = c.benchmark_group("component");

    // 100 on/off operations — simulates rapid chord changes.
    group.bench_function("held_notes_100_on_off", |b| {
        b.iter(|| {
            let mut held = HeldNotes::new();
            for note in 0_u8..100 {
                held.note_on(note, 0.8);
            }
            for note in (0_u8..100).rev() {
                held.note_off(note);
            }
            black_box(&held);
        });
    });

    // Arp cycling through 8 held notes — 100 next_note() calls.
    group.bench_function("held_notes_arp_cycle_100", |b| {
        let mut held = held_c_major();
        b.iter(|| {
            for _ in 0..100 {
                black_box(held.next_note());
            }
        });
    });

    group.finish();
}

fn step_boundary_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("component");

    // Typical: 512 samples, 120 BPM, duration=1 sixteenth, 8 steps.
    // Step every 0.25 beats → usually 0 or 1 boundary per buffer.
    group.bench_function("find_boundaries_512_120bpm", |b| {
        let buffer_end = buffer_end_beat(512.0, 44100.0, 120.0);
        b.iter(|| {
            let result = clock::find_boundaries(
                black_box(0.0),
                black_box(buffer_end),
                black_box(44100.0),
                black_box(120.0),
                black_box(1),
                black_box(8),
            );
            black_box(&result);
        });
    });

    // Worst case: large buffer at high tempo with short step duration.
    // 2048 samples at 300 BPM → ~1.65 beats → ~6.6 sixteenths → ~6 boundaries.
    group.bench_function("find_boundaries_2048_300bpm", |b| {
        let buffer_end = buffer_end_beat(2048.0, 44100.0, 300.0);
        b.iter(|| {
            let result = clock::find_boundaries(
                black_box(0.0),
                black_box(buffer_end),
                black_box(44100.0),
                black_box(300.0),
                black_box(1),
                black_box(32),
            );
            black_box(&result);
        });
    });

    group.finish();
}

fn pending_noteoffs(c: &mut Criterion) {
    let mut group = c.benchmark_group("component");

    // Fill to capacity, then collect all due NoteOffs in a beat range.
    group.bench_function("pending_add_take_due_8", |b| {
        b.iter(|| {
            let mut pending = PendingNoteOffs::new();
            for i in 0..8u8 {
                pending.add(make_pending(60 + i, i as f64 * 0.25));
            }
            black_box(pending.take_due(0.0, 2.0));
        });
    });

    // Retrigger path: add entries, then take_by_note for each.
    group.bench_function("pending_retrigger_8", |b| {
        b.iter(|| {
            let mut pending = PendingNoteOffs::new();
            for i in 0..8u8 {
                pending.add(make_pending(60 + i, i as f64 * 0.25));
            }
            for i in 0..8u8 {
                black_box(pending.take_by_note(60 + i));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Group 2: pipeline — full sequencer path (minus process context)
// ---------------------------------------------------------------------------

fn sequencer_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline");

    // Typical: 8 steps, 4 pulses, 3 held notes, 512 samples, 120 BPM.
    group.bench_function("typical_8s_4p_3n_512", |b| {
        let mut pattern = EuclideanPattern::new();
        pattern.recompute(8, 4);
        let mut held = HeldNotes::new();
        held.note_on(60, 0.8);
        held.note_on(64, 0.8);
        held.note_on(67, 0.8);
        let buffer_end = buffer_end_beat(512.0, 44100.0, 120.0);

        b.iter(|| run_pipeline(&pattern, &mut held, buffer_end, 44100.0, 120.0, 1, 8));
    });

    // Worst case: 32 steps, 32 pulses, 8 held notes, 2048 samples, 300 BPM.
    group.bench_function("worst_32s_32p_8n_2048", |b| {
        let mut pattern = EuclideanPattern::new();
        pattern.recompute(32, 32);
        let mut held = held_c_major();
        let buffer_end = buffer_end_beat(2048.0, 44100.0, 300.0);

        b.iter(|| run_pipeline(&pattern, &mut held, buffer_end, 44100.0, 300.0, 1, 32));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Group 3: realtime — deadline measurement
// ---------------------------------------------------------------------------

/// Measures the worst-case pipeline against the audio deadline.
/// At 44100 Hz / 512 samples, the deadline is ~11.6 ms (11,609,977 ns).
fn realtime_deadline(c: &mut Criterion) {
    let mut group = c.benchmark_group("realtime");
    group.measurement_time(std::time::Duration::from_secs(3));

    // Same workload as worst-case pipeline, with extended measurement
    // for stable percentile estimates.
    group.bench_function("worst_case_vs_512_deadline", |b| {
        let mut pattern = EuclideanPattern::new();
        pattern.recompute(32, 32);
        let mut held = held_c_major();
        let buffer_end = buffer_end_beat(2048.0, 44100.0, 300.0);

        b.iter(|| run_pipeline(&pattern, &mut held, buffer_end, 44100.0, 300.0, 1, 32));
    });

    group.finish();
}

criterion_group!(
    component,
    bjorklund_worst_case,
    held_notes_churn,
    step_boundary_detection,
    pending_noteoffs,
);
criterion_group!(pipeline, sequencer_pipeline);
criterion_group!(realtime, realtime_deadline);
criterion_main!(component, pipeline, realtime);
