# StepOne

A transport-synced Euclidean arpeggiator — CLAP plugin built with [nih-plug](https://github.com/robbert-vdh/nih-plug).

StepOne receives MIDI notes, generates a rhythmic gate pattern using the Euclidean (Bjorklund) algorithm, and emits new MIDI notes by cycling through held input notes in ascending pitch order. It does not pass through its input notes.

## Parameters

| Parameter | Range | Default | Description |
|---|---|---|---|
| Steps | 1–32 | 8 | Total slots in the Euclidean pattern |
| Pulses | 0–32 | 4 | Active gates distributed across steps |
| Step Duration | 1–16 | 1 | Length of each step in sixteenth notes |
| Gate Length | 0–400% | 100% | Gate duration relative to step duration |
| Velocity | 0–100% | 100% | Scales output velocity |

Gate lengths above 100% produce overlapping (polyphonic) output. At 100%, output is legato. Below 100%, output is staccato. At 0%, output is muted.

Output velocity is further modulated by polyphonic pressure (aftertouch) when available.

## Expressions

StepOne forwards PolyPan and responds to PolyPressure from upstream devices (e.g., Bitwig Randomize, MPE controllers). Pan is forwarded per-note to the downstream instrument. Pressure modulates output velocity per-note between gates.

## Building

Requires Rust (stable) and the aarch64-apple-darwin target.

```
cargo xtask bundle step_one --release
```

## Installing

```
cargo xtask deploy
```

This builds, runs clap-validator, and copies the bundle to `~/Library/Audio/Plug-Ins/CLAP/`.

On first install:
```
xattr -d com.apple.quarantine ~/Library/Audio/Plug-Ins/CLAP/StepOne.clap
```

## Testing

```
cargo test          # unit + property-based tests
cargo bench         # criterion benchmarks
```

## Design

See [design.md](design.md) for the full technical design document.

## License

CLAP-only (no VST3). Personal use.
