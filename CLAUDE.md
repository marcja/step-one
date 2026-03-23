# CLAUDE.md — step-one

Instructions for Claude Code working on this project. Read this file, `README.md`, and
`docs/design.md` before starting any work session.

---

## Project orientation

`step-one` is a transport-synced Euclidean arpeggiator — a pure MIDI-to-MIDI CLAP plugin built
with nih-plug. It is the second plugin in the series after `sine-one`. The primary goal remains
**pedagogical**: every decision should be explainable, every module should be small and readable.
Prefer clarity over cleverness at every tradeoff.

StepOne has **no audio processing** — no oscillators, no filters, no DSP. It receives MIDI notes,
generates a rhythmic gate pattern from the Euclidean (Bjorklund) algorithm, and emits new MIDI
notes synced to the host transport. The sequencer logic lives in `src/seq/`, not `src/dsp/`.

The full technical design (algorithm, parameter rationale, transport sync, pending NoteOff
management, expression stash, open questions) is in `docs/design.md`. Read it before touching
sequencer or parameter code.

---

## nih-plug API reference

**Do not use Context7 for nih-plug documentation.** It was unreliable during SineOne development.

Instead, clone nih-plug locally and query the source directly:

```
git clone --depth 1 https://github.com/robbert-vdh/nih-plug.git /tmp/nih-plug
```

Key files:

| File | What to look up |
|---|---|
| `src/midi.rs` | NoteEvent variants — verify PolyPressure and PolyPan field names |
| `src/context.rs` or `src/context/` | ProcessContext trait, send_event(), transport() |
| `src/context/transport.rs` | Transport struct — playing, pos_beats, tempo |
| `src/plugin.rs` | Plugin trait, ProcessStatus variants, MIDI_OUTPUT, AUDIO_IO_LAYOUTS |
| `plugins/` | Example plugins — especially `midi_inverter` for MIDI-only patterns |

Always verify API details against the clone before writing code that depends on nih-plug types.

---

## Workflow: TDD loop

Every unit of work follows this exact sequence. Do not skip or reorder steps.

```
1. WRITE A FAILING TEST
   Write the test first. Run `cargo test` and confirm it fails (red).
   If the test passes before any implementation exists, the test is wrong — fix it.

2. WRITE THE MINIMUM CODE TO PASS
   Implement only what is needed to make the failing test green.
   Do not implement anything not yet covered by a test.

3. RUN ALL CHECKS
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   All three must be clean. Fix any issues before proceeding.

4. RESOLVE REVIEWS
   Run any applicable review commands (e.g., /simplify, /review).
   Apply all suggestions or document explicitly why a suggestion was declined (as a comment).
   Re-run checks after applying changes.

5. COMMIT
   See commit format below.
   Each commit must be small, concrete, and — whenever possible — deliver something
   externally visible (a test that proves a behavior, a parameter the host can see,
   MIDI output that can be verified in Bitwig's MIDI monitor).

6. ANALYZE AND PROPOSE CLAUDE.md CHANGES
   After committing, review the conversation that led to this commit.
   If any corrections, clarifications, or better patterns emerged, propose a concrete
   diff to this file (CLAUDE.md) and ask the user to confirm before applying.
```

---

## Commit message format

Every commit message follows this structure:

```
[scope] verb: short description (≤72 chars total)

Layer: <Seq | Params | Plugin | Tests | Build | Docs | Config>
Tests: <unit | property | integration | none> — <comma-separated test names, or "n/a">
Why: One sentence. What problem does this commit solve, or what does it teach?
Next: One sentence. What is the logical next commit after this one?
```

### Scope tokens

| Scope | Use for |
|---|---|
| `seq/euclidean` | `src/seq/euclidean.rs` — Bjorklund algorithm, pattern storage |
| `seq/held` | `src/seq/held_notes.rs` — sorted held note list, arp index |
| `seq/clock` | `src/seq/clock.rs` — transport-synced step boundary detection |
| `params` | `src/params.rs` |
| `plugin` | `src/plugin.rs` (Plugin trait impl, process loop) |
| `lib` | `src/lib.rs` (exports, top-level wiring) |
| `build` | `Cargo.toml`, `xtask/`, `bundler.toml` |
| `tests` | `tests/` integration test files |
| `bench` | `benches/` |
| `docs` | `README.md`, `docs/`, `CLAUDE.md` |

### Verb vocabulary

Use exactly one of: `add`, `implement`, `fix`, `refactor`, `test`, `remove`, `document`, `configure`.

### Examples

```
[seq/euclidean] add: Bjorklund algorithm with fixed-size pattern array

Layer: Seq
Tests: unit — e_3_8_tresillo, e_5_8_cinquillo, e_0_8_all_rests, e_8_8_all_pulses, pulses_clamped
Why: Euclidean pattern is the first leaf module; no dependencies on params or plugin.
Next: [seq/held] add HeldNotes — sorted note list with arp index.
```

```
[seq/held] add: HeldNotes — sorted list with arp cycling and expression storage

Layer: Seq
Tests: unit — ascending_order, arp_index_wraps, pressure_defaults_to_one, pan_defaults_to_center
Why: Note list is the second leaf module; combined with Euclidean pattern, provides the full gate+note algorithm.
Next: [seq/clock] add step boundary detection from transport beat range.
```

```
[plugin] implement: process() — input drain, transport sync, gate emission

Layer: Plugin
Tests: integration — single_note_arp, two_notes_alternate, silence_when_no_notes_held
Why: First commit where the plugin emits MIDI output; validates the full sequencer path.
Next: [build] configure xtask bundle and deploy so the plugin can be loaded in Bitwig.
```

### Why this format?

- **Scope** tells you where to look in the file tree immediately.
- **Layer** gives Claude Code a quick map of what's been built and what's still missing.
- **Tests** makes the test suite self-documenting in the git log.
- **Why/Next** create an explicit chain of reasoning across commits — useful when resuming a
  session and the conversation context has been lost.

---

## Commit granularity rules

Prefer commits that are **one thing**. Use these as guides:

- One sequencer struct per commit (EuclideanPattern is one commit, HeldNotes is another)
- One layer per commit (sequencer logic and Plugin trait wiring should not be in the same commit)
- Tests for a struct ship **in the same commit** as the struct — never ahead, never behind
- Build/config changes (Cargo.toml, bundler.toml, xtask/) are their own commit
- `CLAUDE.md` changes are always their own separate commit with scope `[docs]`

**Do not batch.** A commit that says "add euclidean, held notes, and clock" makes the git log
useless for learning and makes rollback difficult.

---

## Codetags

Use codetags as inline comments when an implementation is intentionally incomplete, approximate,
or requires revisiting. Always include a reason.

| Tag | Meaning |
|---|---|
| `TODO` | Known missing behavior; should be implemented in a subsequent commit |
| `FIXME` | Known bug or incorrect behavior being deferred |
| `HACK` | Working but fragile, non-obvious, or non-idiomatic; should be cleaned up |
| `NOTE` | Pedagogical explanation for the author; not a defect |
| `REVIEW` | A design decision that should be revisited once the plugin is testable in Bitwig |

Example contexts for StepOne:

```rust
// TODO(interleave): input events are drained before step boundary scan;
//   a future version could interleave them sample-accurately.
//   See docs/design.md "Open Questions #1".

// NOTE: gate_length > 100% produces overlapping output notes.
//   The pending NoteOff list must support multiple simultaneous entries.

// REVIEW(keepalive): using ProcessStatus::KeepAlive so the host calls
//   process() continuously for transport-synced step detection.
//   Verify this works correctly in Bitwig with no audio I/O.
```

Codetags are searchable: `grep -r "TODO\|FIXME\|HACK\|REVIEW" step_one/src/`

---

## Code style

- **Explicit over implicit**: name variables for what they represent (`beats_per_sample`, not `b`).
- **Comment the math**: when a formula appears (e.g., `pos_beats * 4.0` to convert beats to
  sixteenths), add a comment that states what it computes in plain English.
- **No magic numbers**: all numeric constants should be named (`const MAX_STEPS: usize = 32`) or
  accompanied by a comment explaining their origin.
- **Keep functions short**: if a function body exceeds ~20 lines, consider splitting it.
- **`#[allow(...)]` is forbidden** without a comment explaining why the lint is wrong for this case.
- **Fixed-size arrays for all sequencer state**: held notes, pending NoteOffs, expression stashes,
  Euclidean pattern, step boundary lists. No Vec, no heap allocation in any code reachable from
  process().

---

## nih-plug conventions to follow

These are rules specific to nih-plug that are easy to get wrong. Includes lessons learned from
SineOne development.

**General:**

- Sequencer state (held notes, arp index, pending NoteOffs, expression stashes) lives on the
  **plugin struct**, NOT in `Params`. `Params` holds only what the user/host controls.
- `#[id = "stable-string"]` on every param — this string is persisted in DAW sessions and must
  never change once any real session has been saved.
- `initialize()` is where sample-rate-dependent values are computed. Do not do this in
  `Default::default()`.
- `reset()` must clear all sequencer state: held notes, arp index, pending NoteOffs, expression
  stashes. The host calls `reset()` after `initialize()`, so anything set in `initialize()` but
  not re-set in `reset()` will be lost.
- nih-plug's built-in `Smoother` on params is not initialized in the test harness (always returns
  0.0 from `.smoothed.next()`). Use `.value()` for all params — StepOne reads params at event
  boundaries, not per-sample, so smoothing is never needed.
- Never allocate in `process()`. The `assert_process_allocs` Cargo feature will abort in debug
  builds if you do.

**MIDI-specific (new for StepOne):**

- `AUDIO_IO_LAYOUTS = &[]` — no audio ports. This is a pure MIDI effect.
- `MIDI_INPUT` and `MIDI_OUTPUT` both set to `MidiConfig::Basic`.
- Use `context.send_event()` to emit output MIDI events (NoteOn, NoteOff, PolyPan).
- Input events are consumed via `context.next_event()`. Do NOT forward input events to output —
  StepOne replaces its input, it doesn't transform it.
- Use `ProcessStatus::KeepAlive` (not `Normal`) — the plugin needs continuous process() calls to
  detect transport-synced step boundaries even when no input events are pending. Verify this
  works in Bitwig early.

**Transport:**

- Read `context.transport()` on every process() call for playing, pos_beats, and tempo.
- pos_beats and tempo are `Option<f64>` — handle `None` gracefully (return KeepAlive, emit no
  gates).
- Detect transport jumps by comparing the current buffer's start beat against the previous
  buffer's expected end beat. On jump, flush all pending NoteOffs at sample 0.

**Expression stash pattern (from SineOne):**

- Bitwig's Randomize device may send PolyPressure or PolyPan *before* the corresponding NoteOn
  in the same buffer. Stash these in a fixed-size array indexed by note number. Apply the stashed
  value when the NoteOn arrives, then clear the stash entry. Clear stash entries on NoteOff and
  on reset().

---

## Quality gates (must all pass before any commit)

```bash
cargo fmt                  # formatting — no diff allowed
cargo clippy -- -D warnings   # zero warnings
cargo test                 # all tests green
```

After bundle builds, additionally:

```bash
clap-validator validate target/bundled/StepOne.clap --only-failed   # zero failures
```

---

## Suggested build order

Follow this sequence. Each step is a candidate commit boundary.

```
 1. [build]         Cargo workspace scaffold (Cargo.toml, xtask/, step_one/Cargo.toml, bundler.toml)
 2. [lib]           Stub lib.rs + plugin.rs that compiles (Plugin trait with empty impls, no audio I/O)
 3. [seq/euclidean] Bjorklund algorithm + pattern storage + unit tests (+ proptest)
 4. [seq/held]      HeldNotes — sorted list, arp index, velocity/pressure/pan + unit tests (+ proptest)
 5. [seq/clock]     Step boundary detection from transport beat range + unit tests (+ proptest)
 6. [params]        StepOneParams — Steps, Pulses, Step Duration, Gate Length, Velocity + unit tests
 7. [plugin]        initialize() and reset() — wire sample rate, clear all sequencer state
 8. [plugin]        process() — input drain, transport check, step clock, gate emission, NoteOff mgmt
 9. [tests]         Lifecycle integration tests (silence, arp cycling, gate length, pressure, pan)
10. [build]         cargo xtask deploy + standalone binary feature
11. [bench]         criterion benchmarks (Bjorklund, held notes churn, process block)
12. [docs]          README and design.md updates reflecting any implementation deltas
```

This order ensures every commit builds on a green test suite and no step requires two things
to exist simultaneously before either is testable. The three `seq/` modules (steps 3–5) are
independent leaf modules with no dependencies on each other — they can be built in any order.

---

## Post-commit CLAUDE.md review

After every commit, before ending the session:

1. Re-read the conversation since the last commit.
2. Check whether any of the following occurred:
   - A correction was made to code Claude Code produced
   - A convention was established that isn't yet in this file
   - A nih-plug behavior was discovered that differs from what's documented here
   - A workflow step was added, skipped, or modified by the user
3. If yes to any of the above: draft a specific proposed change to this file (CLAUDE.md) using a
   `diff`-style or before/after block. Present it to the user and wait for confirmation before
   applying. Do not self-apply CLAUDE.md edits.
4. If no changes are warranted, say so briefly ("No CLAUDE.md updates needed after this commit.")

---

## Reading the git log

```bash
git log --oneline          # scan scope + verb + description
git log                    # full messages with Layer / Tests / Why / Next
git log --grep="seq/"      # filter to sequencer commits only
git log --grep="TODO"      # find commits that introduced deferred work
grep -r "TODO\|FIXME\|HACK\|REVIEW" step_one/src/   # find all open codetags
```
