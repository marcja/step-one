# StepOne — Technical Design

**Version:** 0.2  
**Purpose:** Second nih-plug CLAP plugin — Euclidean arpeggiator (MIDI effect)  
**Audience:** The author, building on the SineOne foundation

---

## Overview

StepOne is a transport-synced Euclidean arpeggiator CLAP plugin. It receives MIDI NoteOn/NoteOff
events, derives a rhythmic gate pattern using a Euclidean (Bjorklund) algorithm, and emits new
MIDI notes by cycling through the currently held input notes in ascending pitch order. It does
not pass through its input MIDI notes. There is no audio processing, no GUI, and no Faust — this
is a pure MIDI-to-MIDI plugin.

StepOne's algorithm has two separable components:

1. **When** to emit a gate — governed by a Euclidean pattern (steps, pulses, step duration)
2. **What note** to emit — governed by a sorted list of held input notes, cycled on each gate

Both components are designed for future extension (noted in Open Questions).

---

## Plugin Type & I/O

| Property | Value |
|---|---|
| Type | MIDI Effect |
| Audio input | None |
| Audio output | None |
| Audio I/O layouts | Empty — no audio processing |
| MIDI input | MidiConfig::Basic — NoteOn, NoteOff, PolyPressure, PolyPan |
| MIDI output | MidiConfig::Basic — NoteOn, NoteOff, PolyPan |
| CLAP features | CLAP_FEATURE_NOTE_EFFECT |
| GUI | None — generic host parameter list |
| Transport | Required — beat position, tempo, playing state |

**Why no audio I/O?** StepOne is a pure MIDI processor. In Bitwig, it sits before an instrument
in the device chain (or on a dedicated Note FX layer).

**Why MidiConfig::Basic?** We need NoteOn, NoteOff, PolyPressure (polyphonic aftertouch), and
PolyPan (polyphonic stereo position). Both are note expression events available at
MidiConfig::Basic in nih-plug, as confirmed by SineOne's use of PolyPan. We don't need CCs,
pitch bend, or other polyphonic expression for v1.

**Implementation note:** Verify the exact NoteEvent variant names and field signatures for
PolyPressure and PolyPan against the local nih-plug clone at implementation time (see Build &
Test Plan). SineOne confirms PolyPan works at MidiConfig::Basic.

---

## Algorithm

### Component 1: Euclidean Gate Pattern

The Euclidean rhythm algorithm (Bjorklund, 2005) distributes *pulses* active onsets across
*steps* slots as evenly as possible. For example, E(3, 8) = `[x . . x . . x .]` — 3 pulses
spread across 8 steps.

**Parameters that define the pattern:**

- **Steps** (1–32): total slots in the pattern
- **Pulses** (0–32, clamped to ≤ steps): number of active gates
- **Step Duration** (1–16): length of each step in sixteenth notes

The total pattern length in sixteenth notes is `steps × duration`. At 120 BPM with steps=8 and
duration=1, one full pattern cycle = 8 sixteenth notes = 2 beats = 1 second.

**The Bjorklund algorithm** works by iterative grouping — a binary Euclidean division:

```
bjorklund(steps=8, pulses=3):
  Start:  [1] [1] [1] [0] [0] [0] [0] [0]
  Pass 1: [1 0] [1 0] [1 0] [0] [0]
  Pass 2: [1 0 0] [1 0 0] [1 0]
  Pass 3: [1 0 0] [1 0 0] [1 0]   — remainder < 2, done
  Result: [1, 0, 0, 1, 0, 0, 1, 0]
```

The pattern is stored as a fixed-size array (MAX_STEPS = 32) and recomputed whenever `steps` or
`pulses` changes. No heap allocation.

**Pattern position is derived from the host transport:**

```
sixteenth_position  = pos_beats × 4.0
pattern_length      = steps × duration              (in sixteenth notes)
position_in_pattern = sixteenth_position mod pattern_length
current_step        = floor(position_in_pattern / duration)
```

This means the pattern position is always in sync with the DAW timeline. When the transport
restarts from beat 0, the pattern naturally restarts from step 0. When the user scrubs to an
arbitrary position, the pattern jumps to the corresponding step. No explicit reset mechanism is
needed.

**Gate timing within a step:**

A gate fires at the beginning of a step where the pattern is active. The gate's NoteOn is emitted
at the exact sample where the step boundary falls. The corresponding NoteOff is scheduled
relative to the **distance to the next active pulse**, not the fixed step duration:

```
distance_steps    = steps_to_next_active_pulse(current_step)
distance_beats    = distance_steps × (duration / 4.0)
gate_length_beats = (gate_length_pct / 100.0) × distance_beats
off_at_beat       = gate_on_beat + gate_length_beats
```

For example, E(3,8) = `[X..X..X.]` with pulses at steps 0, 3, 6 and duration = 1 sixteenth:

| Pulse at step | Next pulse | Distance (steps) | Distance (beats) | 100% gate |
|---------------|-----------|-----------------|-----------------|-----------|
| 0             | 3         | 3               | 0.75            | 0.75      |
| 3             | 6         | 3               | 0.75            | 0.75      |
| 6             | 0 (wraps) | 2               | 0.50            | 0.50      |

At 100% gate length, each note sustains until the next pulse fires (true legato). At >100%,
gates overlap producing layered notes (see Pending NoteOff Management).

### Component 2: Held Notes (Up Arpeggiator)

The held note list is a sorted collection of currently held input MIDI notes in ascending pitch
order. An arp index tracks which note to play next.

Each entry stores (note, velocity, pressure, pan):

- **velocity** (f32, 0.0–1.0): captured from the NoteOn event
- **pressure** (f32, 0.0–1.0): initialized to 1.0 on NoteOn, updated by PolyPressure events
- **pan** (f32, -1.0–1.0): initialized to 0.0 (center) on NoteOn, updated by PolyPan events

**Output velocity formula:**

```
output_velocity = input_velocity × pressure × (velocity_param / 100.0)
```

At defaults (pressure = 1.0, velocity param = 100%), the input velocity passes through unchanged.
A Bitwig Randomize device sending PolyPressure events can dynamically modulate the output
velocity per-note between gates. An MPE keyboard's aftertouch continuously updates pressure for
the held note.

**Output pan:** When a gate fires, StepOne emits a PolyPan event at the same timing as the
NoteOn, using the stored pan value for that note. This allows a Bitwig Randomize device (or MPE
controller) to spatially position individual arp notes in the stereo field. The downstream
instrument receives PolyPan and positions the note accordingly, exactly as it would for a
manually played note.

**Operations:**

- **NoteOn received**: insert note into the list in sorted position (maintaining ascending order).
  Store the note's velocity. Set pressure to 1.0 and pan to 0.0. Then check the expression
  stashes (see below) — if stashed values exist for this note, apply them and clear the entries.
- **NoteOff received**: remove the note from the list. If the arp index now exceeds the list
  length, wrap it to 0. Also clear any stashed values for this note. (Currently sounding output
  notes are not affected — their NoteOffs fire at their scheduled times regardless.)
- **PolyPressure received**: if the note is in the list, update that entry's pressure. If the
  note is NOT in the list, stash the pressure value (see Expression Stash below).
- **PolyPan received**: if the note is in the list, update that entry's pan. If the note is NOT
  in the list, stash the pan value (see Expression Stash below).
- **Gate fires**: if the list is non-empty, emit a NoteOn for held_notes[arp_index] using the
  computed output velocity, emit a PolyPan for the same note using the stored pan value, then
  advance arp_index = (arp_index + 1) % held_notes.len(). If the list is empty, emit nothing.

### Expression Stash

Bitwig's Randomize device (and some MPE implementations) may send PolyPressure or PolyPan events
*before* the corresponding NoteOn in the same buffer. Without special handling, these events
would be dropped because the note isn't in the held note list yet, and the subsequent NoteOn
would reset pressure to 1.0 and pan to 0.0 — the intended expression would be lost.

SineOne solved the identical problem for PolyPan events. StepOne uses the same pattern for both
pressure and pan: a fixed-size stash array indexed by MIDI note number (one entry per expression
type, 128 entries each).

- When PolyPressure/PolyPan arrives for a note NOT in the list: stash the value
- When NoteOn arrives: insert with pressure = 1.0 and pan = 0.0, then check both stashes. If
  either has a value, apply it to the entry and clear the stash.
- When NoteOff arrives: clear both stashes for the note (no stale values for reused notes)
- On reset(): clear both stashes entirely

No heap allocation. Maximum 128 entries per stash.

**Maximum list size:** 128 (one per MIDI note number). In practice, a human hand holds 1–10
notes. The list is stored as a fixed-size array to avoid heap allocation.

**Example:**

```
Player holds: C4, E4, G4       → held = [C4, E4, G4], arp_index = 0
Gate 1: emit C4, arp_index → 1
Gate 2: emit E4, arp_index → 2
Player adds B3                  → held = [B3, C4, E4, G4], arp_index = 2
Gate 3: emit E4, arp_index → 3
Gate 4: emit G4, arp_index → 0
Player releases C4              → held = [B3, E4, G4], arp_index = 0
Gate 5: emit B3, arp_index → 1
```

### Combined Flow

```
Host transport (playing, pos_beats, tempo)
           │
           ▼
┌─────────────────────────────────┐
│         Step Clock              │
│  sixteenth_pos = beats × 4     │
│  current_step = derived from    │
│    pattern length & duration    │
│  detect step boundary crossings │
│    within this buffer           │
└──────────┬──────────────────────┘
           │ step boundary at sample N?
           ▼
┌─────────────────────────────────┐
│     Euclidean Pattern           │
│  pattern[current_step] == true? │
│     (precomputed Bjorklund)     │
└──────────┬──────────────────────┘
           │ yes (gate fires)
           ▼
┌─────────────────────────────────┐
│       Held Notes                │
│  list non-empty?                │
│  → NoteOn(held[arp_index],      │
│      vel × pressure × scale)    │
│  → PolyPan(held[arp_index].pan) │
│  → advance arp_index            │
│  → schedule NoteOff             │
└─────────────────────────────────┘
```

---

## Transport Sync Details

StepOne reads transport state on every process() call. The relevant fields: `playing` (bool),
`pos_beats` (Option<f64>, current position in quarter notes), `tempo` (Option<f64>, BPM).

**Calculating beat range for a buffer:**

```
beats_per_sample  = tempo / (60.0 × sample_rate)
buffer_start_beat = pos_beats
buffer_end_beat   = pos_beats + (buffer_size × beats_per_sample)
```

**Detecting step boundaries within a buffer:**

A step boundary occurs every `duration / 4.0` beats (since `duration` is in sixteenth notes, and
1 beat = 4 sixteenths). The pattern wraps every `steps × duration / 4.0` beats.

The step clock scans the beat range [buffer_start, buffer_end) and identifies every step boundary
that falls within it, computing the exact sample offset for each. At typical tempos and buffer
sizes (512 samples at 44100 Hz ≈ 11.6ms ≈ 0.023 beats at 120 BPM), most buffers contain 0–1
step boundaries. Fast tempos with short step durations (e.g., 200 BPM, duration=1) could produce
2–3 boundaries per buffer.

**When transport is not playing:** All pending NoteOffs are sent immediately (at sample 0 of the
buffer). No new gates are emitted. The arp index is preserved (not reset) so the arp continues
where it left off when the transport resumes.

**When transport jumps (scrub/loop):** If a transport position discontinuity is detected (current
pos_beats is earlier than expected or jumps by more than one buffer's worth), all pending NoteOffs
are sent immediately at sample 0 of the buffer, before normal step-boundary processing resumes
at the new position.

---

## Pending NoteOff Management

Because gate length ranges from 0% to 400%, output notes can overlap — a gate's NoteOff may
arrive well after the next gate fires. StepOne must track multiple pending NoteOffs
simultaneously.

**Maximum overlap:** At 400% gate length, each note lasts 4× the step duration. If every step is
a pulse, at most 4 notes are sounding at any given time (the 4 most recently fired gates). A
fixed-size pending list of at least 4 entries is sufficient. Use a conservative bound (e.g., 8)
for safety.

**NoteOff scheduling:**

When a gate fires a NoteOn at beat position B:

```
distance_steps    = steps_to_next_active_pulse(current_step)
distance_beats    = distance_steps × (duration / 4.0)
gate_length_beats = (gate_length_pct / 100.0) × distance_beats
off_at_beat       = B + gate_length_beats
```

Add a pending NoteOff entry with (note, channel, off_at_beat). On each process() call, scan the
pending list for any entries whose off_at_beat falls within [buffer_start, buffer_end) and emit
NoteOff at the computed sample offset.

**Same-pitch retrigger:** If the arp cycles back to a pitch that already has a pending NoteOff
(i.e., the same MIDI note number is about to be re-triggered), the old pending NoteOff must be
emitted **before** the new NoteOn at the same sample offset. This prevents the downstream
instrument from seeing two NoteOns for the same pitch without an intervening NoteOff.

**NoteOff at gate_length = 0%:** When gate length is 0%, no NoteOn is emitted — effectively a
mute. This provides a clean way to silence the arp output without changing the pattern.

**NoteOff at gate_length = 100%:** The NoteOff coincides exactly with the next step boundary. If
the next step is also a pulse, the NoteOff fires at that boundary, immediately followed by the
next NoteOn. Output is legato.

**NoteOff at gate_length > 100%:** The NoteOff extends past the next step boundary. Multiple
output notes overlap. At 200%, each note lasts 2 steps; at 400%, 4 steps. The downstream
instrument receives polyphonic overlapping NoteOn/NoteOff pairs, producing chordal or sustain
effects from a single arp voice.

**Flush on transport stop or jump:** All pending NoteOffs are emitted immediately at sample 0.

---

## Process Loop Behavior

The process() function follows this sequence on each call:

1. **Read parameters.** If steps or pulses changed since last call, recompute the Euclidean
   pattern.
2. **Drain all input MIDI events** — NoteOn, NoteOff, PolyPressure, PolyPan — updating the held
   note list and expression stashes. Do NOT forward any input events to output.
3. **Check transport.** If not playing, flush all pending NoteOffs at sample 0 and return
   KeepAlive. If transport jumped, flush all pending NoteOffs at sample 0 before continuing.
4. **Compute the beat range** for this buffer from pos_beats, tempo, and sample_rate.
5. **Scan the beat range** for pending NoteOff deadlines and step boundaries, processing them in
   chronological order by beat position:
   - For each pending NoteOff whose off_at_beat falls in [buffer_start, buffer_end): emit NoteOff
     at the computed sample offset and remove the entry.
   - For each step boundary in [buffer_start, buffer_end): if the pattern is active at that step
     and gate_length > 0 and the held note list is non-empty: first emit any pending NoteOff for
     the same pitch (same-pitch retrigger), then emit NoteOn + PolyPan and add a new pending
     NoteOff entry. Advance the arp index.
6. **Return KeepAlive** (not Normal) — the plugin needs continuous process() calls to detect step
   boundaries even when no input events are pending.

**Why drain all input events first?** Input MIDI events and step boundaries are on different
timelines. By draining all input events first (updating the held note list), then scanning for
step boundaries, we avoid interleaving two different event streams. The trade-off: a NoteOn at
sample 200 and a step boundary at sample 100 within the same buffer won't interact. This is at
most one buffer of latency (5–12ms), matches the behavior of virtually all hardware and software
arpeggiators, and keeps the code simple. Noted as an Open Question for potential v2 refinement.

---

## Parameter System

| Parameter | #[id] | Type | Range | Default | Smoothing | Unit | Notes |
|---|---|---|---|---|---|---|---|
| Steps | "steps" | IntParam | 1–32 | 8 | None | steps | Euclidean pattern length |
| Pulses | "pulses" | IntParam | 0–32 | 4 | None | pulses | Active gates; clamped ≤ steps at read time |
| Step Duration | "step_dur" | IntParam | 1–16 | 1 | None | 16ths | Sixteenth notes per step |
| Gate Length | "gate_len" | FloatParam | 0.0–400.0 | 100.0 | None | % | Gate duration as % of step duration |
| Velocity | "velocity" | FloatParam | 0.0–100.0 | 100.0 | None | % | Scales input note velocity |

**Notes on parameter choices:**

- **Steps, Pulses, Step Duration** use IntParam because they are discrete musical values.
- **Pulses** is declared with range 0–32 but clamped to min(pulses, steps) wherever it is read.
  This avoids invalid Euclidean patterns without complex parameter interdependencies.
- **Gate Length** ranges from 0% to 400% (matching Bitwig's built-in arpeggiator). At 0%, no
  NoteOn is emitted (mute). At 100% (default), notes are legato — each NoteOff coincides with
  the next step boundary. Above 100%, notes overlap, producing polyphonic output. At 400%, each
  note lasts 4× the step duration.
- **Velocity** scales the output velocity in combination with the note's stored pressure:
  output_velocity = input_velocity × pressure × (velocity_pct / 100.0). Pressure is updated live
  by PolyPressure events, so the same held note can produce different output velocities on
  successive gates.
- **No smoothing on any parameter.** All parameters are read at event boundaries, not per-sample.

**The #[id] contract:** Same as SineOne — these strings are persisted in DAW sessions and must
never change after the first real session is saved.

**Pattern recomputation:** When steps or pulses changes, the Euclidean pattern is recomputed.
This is a lightweight O(steps) operation on a fixed-size array. Triggered by comparing current
parameter values against cached copies at the top of each process() call.

---

## State & Preset Design

All plugin state is captured by the five Params fields — no custom serialization needed.

Sequencer state (held note list, arp index, pending NoteOffs, cached pattern, expression stashes)
is **not** persisted. On reload, the held note list is empty, the arp index is 0, pending NoteOffs
are cleared, both expression stashes are cleared, and the pattern is recomputed from the current
parameter values. This is correct behavior — the arp starts fresh and waits for the player to
hold notes.

---

## File & Module Structure

```
step-one/
├── .cargo/
│   └── config.toml         # alias: xtask = "run --package xtask --"
├── Cargo.toml              # workspace manifest
├── Cargo.lock
├── bundler.toml            # [step_one] name = "StepOne"
├── xtask/
│   ├── Cargo.toml
│   └── src/
│       └── main.rs         # "deploy" subcommand + nih_plug_xtask delegation
└── step_one/
    ├── Cargo.toml
    ├── benches/
    │   └── seq_bench.rs    # criterion benchmarks (Bjorklund, held notes, process)
    └── src/
        ├── lib.rs          # nih_export_clap! macro
        ├── plugin.rs       # StepOne struct + Plugin trait impl
        ├── params.rs       # StepOneParams struct
        ├── seq/
        │   ├── mod.rs
        │   ├── euclidean.rs    # Bjorklund algorithm + pattern storage
        │   ├── held_notes.rs   # Sorted held note list with arp index
        │   └── clock.rs        # Transport-synced step boundary detection
        └── main.rs         # standalone binary
```

**Why `seq/` instead of `dsp/`?** StepOne has no DSP — no audio processing, no filters, no
oscillators. The module name reflects what the plugin does: sequence MIDI events in time.

**Standalone binary:** Included for structural consistency with SineOne, but a MIDI-only plugin
has limited standalone utility (no audio output, and MIDI routing requires a host). May be useful
for testing with an external MIDI monitor.

---

## Testing Strategy

### Layer 1 — Unit tests (cargo test)

Tests live in #[cfg(test)] blocks in the same file as the struct under test.

**euclidean.rs:**

- E(0, 8) = all rests; E(8, 8) = all pulses; E(1, 1) = single pulse
- E(3, 8) = tresillo [1,0,0,1,0,0,1,0]
- E(5, 8) = cinquillo [1,0,1,1,0,1,1,0]
- E(5, 12) = [1,0,0,1,0,1,0,0,1,0,1,0]
- E(3, 4) = cumbia [1,0,1,1]
- E(4, 12) = [1,0,0,1,0,0,1,0,0,1,0,0]
- Pulses clamped to steps: E(10, 4) behaves as E(4, 4)
- Recompute writes to existing array (no heap allocation)

**held_notes.rs:**

- Empty list returns None for next note
- Single note repeats on every gate
- Notes inserted as [E4, C4, G4] produce sorted [C4, E4, G4]
- Arp index wraps after last note
- Removing a note before current index adjusts index correctly
- Removing the only held note returns to empty
- NoteOff for unheld note is a no-op
- Duplicate NoteOn for same pitch is ignored
- Velocity stored and retrievable per note
- Index wraps to 0 when list shrinks below current index
- Pressure defaults to 1.0; set_pressure updates held note; ignored for absent note
- Pressure survives other note changes; resets to 1.0 on re-add
- Pan defaults to 0.0 (center); set_pan updates held note; ignored for absent note
- Pan survives other note changes; resets to 0.0 on re-add

**clock.rs:**

- Buffer with no step boundary returns empty list
- Single boundary at expected sample offset
- Multiple boundaries at fast tempo with short duration
- Boundary at exact buffer start (sample 0) is included
- Boundary at exact buffer end is excluded (half-open interval)
- Step index wraps at pattern length

**params.rs:**

- Each param's default is within its declared min/max

### Layer 2 — Property-based tests (proptest)

**euclidean.rs:** For all (steps in 1..=32, pulses in 0..=32): the number of true entries in the
first `steps` slots equals min(pulses, steps). No true entries exist beyond slot `steps`.

**held_notes.rs:** For any sequence of note_on/note_off operations: the list is always sorted
ascending, never exceeds 128 entries, and all pressure values are in [0.0, 1.0] and finite.
All pan values are in [-1.0, 1.0] and finite.

**clock.rs:** For any (pos_beats, tempo, duration, steps): all returned step boundary sample
offsets are within [0, buffer_size).

### Layer 3 — Integration tests (plugin lifecycle)

These exercise the full plugin lifecycle (initialize → reset → process) using mock contexts,
without a real DAW or audio driver.

- Plugin can be constructed; params() returns valid Arc
- initialize() stores sample rate
- reset() clears held notes, all pending NoteOffs, and both expression stashes
- No output events when transport is stopped
- No output events when no notes are held (even with active pattern)
- Single held note produces repeated NoteOn for that note across multiple gates
- Two held notes (C4, E4) with E(2,2) produce alternating C4, E4, C4, E4
- NoteOff fires at correct beat offset for 50% gate length (halfway through step)
- E(0, N) produces no gates regardless of held notes
- Transport restart from beat 0 restarts pattern from step 0
- All pending NoteOffs sent on transport stop
- At 100% gate length with consecutive pulses: NoteOff(old) precedes NoteOn(new) at same sample
- Gate length 0% produces no NoteOn events
- Velocity param at 50% halves output velocity relative to input
- Pressure modulates velocity: PolyPressure(0.5) → output is input_vel × 0.5 × vel_scale
- Pressure stash: PolyPressure then NoteOn in same buffer → stashed pressure applied
- Pressure resets to 1.0 on re-add (not old value)
- Stashed pressure cleared by reset()
- Pan forwarded on gate: PolyPan(-0.5) → output includes PolyPan(-0.5) at same timing as NoteOn
- Pan defaults to center: no PolyPan input → output includes PolyPan(0.0)
- Pan stash: PolyPan then NoteOn in same buffer → stashed pan applied
- Pan resets to 0.0 on re-add
- Stashed pan cleared by reset()
- **Gate length > 100% produces overlapping notes:** with gate_length = 200% and E(4,4)
  duration=1, two output notes overlap at any given time
- **Same-pitch retrigger sends NoteOff before NoteOn:** single held note, all pulses,
  gate_length = 200% → when arp re-triggers the same pitch, NoteOff(old) precedes NoteOn(new)

### Layer 4 — CLAP compliance (clap-validator)

Run after every build. Same checks as SineOne: scan time, parameter round-trips, state
save/load, threading invariants, fuzz pass, descriptor validity.

### Performance Benchmarks (criterion)

StepOne's process loop is lightweight — no audio DSP. The primary concern is that step boundary
detection and note event emission are fast enough to never be a bottleneck.

**Component benchmarks:** Bjorklund recompute at worst case (E(16, 32)); held note list churn
(100 on/off operations); step boundary detection in a 512-sample buffer at 120 BPM.

**Process benchmarks:** Full process() call at typical (8 steps, 4 pulses, 3 held notes, 512
samples, 120 BPM) and worst case (32 steps, 32 pulses, 400% gate length). All benchmarks report
throughput in samples/second.

### Realtime Safety Checklist

- assert_process_allocs feature enabled (abort on any heap allocation in process())
- No Vec, String, or any allocation in process()
- No Mutex or RwLock in process()
- Held note list: fixed-size array, no heap
- Expression stashes: fixed-size arrays, no heap
- Euclidean pattern: fixed-size array, no heap
- Pending NoteOff list: fixed-size array, no heap
- Step boundary list: fixed-size array (bounded by max boundaries per buffer)
- Parameter change detection: cached integer comparison (no string ops)

---

## Build & Test Plan

### Day-0 Setup (one-time)

Same toolchain and tools as SineOne (rustup aarch64-apple-darwin target, cargo-watch,
clap-validator). Skip if already installed.

### nih-plug Local Clone (API reference)

During SineOne development, Context7 proved unreliable for nih-plug documentation. Clone nih-plug
into /tmp and query it directly for API details (NoteEvent variants, ProcessContext methods,
Transport fields, etc.):

```
git clone --depth 1 https://github.com/robbert-vdh/nih-plug.git /tmp/nih-plug
```

Key files: src/midi.rs (NoteEvent enum — verify PolyPressure and PolyPan variants),
src/context.rs or context/ (ProcessContext trait, Transport struct), src/plugin.rs (Plugin trait,
ProcessStatus variants), plugins/ (example plugins, especially midi_inverter).

**Do not use Context7 for nih-plug.** Use the local clone as the source of truth.

### Development Loop

cargo check → cargo clippy → cargo test → cargo xtask bundle step_one --release

### Deploy (cargo xtask deploy)

Builds, validates, and installs in one step:
1. cargo xtask bundle step_one --release
2. clap-validator validate target/bundled/StepOne.clap --only-failed
3. Copy bundle to ~/Library/Audio/Plug-Ins/CLAP/

### Gatekeeper (first install only)

xattr -d com.apple.quarantine ~/Library/Audio/Plug-Ins/CLAP/StepOne.clap

### Bitwig Smoke Tests (manual, after install)

1. **Plugin loads** — appears in Bitwig browser under Note FX (or MIDI effects)
2. **Parameters visible** — Steps, Pulses, Step Duration, Gate Length, Velocity appear with
   correct ranges and units
3. **No output without notes** — play transport with no MIDI input; no notes on output MIDI
   monitor
4. **Single held note arpeggiated** — hold C4 with Steps=4, Pulses=4, Duration=1; hear evenly
   spaced C4 at sixteenth-note intervals
5. **Euclidean pattern audible** — Steps=8, Pulses=3; hear tresillo [x . . x . . x .]
6. **Up arp cycle** — hold C4, E4, G4 with Steps=8, Pulses=8; hear ascending C-E-G-C-E-G-C-E
7. **Gate length staccato** — Gate Length = 10%; notes should be very short
8. **Gate length legato** — Gate Length = 100%; notes should be seamlessly connected
9. **Gate length overlapping** — Gate Length = 200%; notes should overlap (verify with MIDI
   monitor that two NoteOns are active simultaneously)
10. **Gate length max** — Gate Length = 400%; four notes overlap at once with all-pulse pattern
11. **Step Duration** — increase from 1 to 4; same pattern plays 4× slower
12. **Transport sync** — stop and restart; pattern restarts in sync with beat grid
13. **State save/load** — save project, close, reopen; parameters restore correctly
14. **Dynamic chord changes** — hold a chord, change it while running; arp incorporates new notes
    within the next gate or two
15. **Pressure modulates velocity** — Randomize device before StepOne; randomize Pressure; output
    velocity varies gate to gate
16. **Pan forwarded to output** — Randomize device before StepOne; randomize Pan; arp notes
    positioned across stereo field (downstream instrument must respond to PolyPan)

### Pre-Commit Hook

Same as SineOne: cargo fmt --check, cargo clippy -D warnings, cargo check, cargo test.

---

## Assumptions Made (Requiring Confirmation)

1. **Output velocity** — output_velocity = input_velocity × pressure × (velocity_param / 100.0).
   Pressure defaults to 1.0 on NoteOn, updated by PolyPressure. Velocity param (0–100%, default
   100%) scales uniformly.

1a. **Output pan** — each emitted NoteOn is accompanied by a PolyPan event carrying the stored
   pan value. Pan defaults to 0.0 (center) on NoteOn, updated by PolyPan events.

2. **Pattern position** — derived from host transport beat position. No explicit reset control.
   Transport restart from beat 0 naturally restarts the pattern.

3. **Output MIDI channel** — all output on channel 0. Input channel ignored.

4. **Input event draining** — all input MIDI events consumed before step-boundary scanning. At
   most one buffer (5–12ms) of interaction latency. (See Open Questions.)

---

## Open Questions

1. **Sample-accurate input event interleaving** — The current design drains all input events
   before processing step boundaries. An alternative interleaves them in sample order. Practical
   difference is ≤1 buffer (5–12ms). Defer to v2 if needed.

2. **Arp index behavior on note set change** — Current design preserves and wraps. Alternative:
   reset to 0. Test both in practice.

3. **Output MIDI channel** — Currently hardcoded to channel 0. Future: parameter or echo input.

4. **Euclidean pattern rotation (shift) parameter** — Shifts the pattern start point (e.g.,
   `[x . . x . . x .]` shifted by 1 = `[. . x . . x . x]`). Natural future parameter.

5. **Note ordering modes beyond "Up"** — Down, Up-Down, Random, Order (insertion order), etc.

6. **Gate pattern modes beyond Euclidean** — Straight, random probability, accents, polymetric.

7. **Swing / groove** — Offset every other step boundary by a percentage of step duration.

8. **MIDI clock output** — Not currently emitted. Would be needed for hardware sync.

9. **~~ProcessStatus~~** *(decided)*: Use KeepAlive. Verify against local nih-plug clone and test
   early in Bitwig. Fall back to Normal if Bitwig exhibits unexpected behavior.
