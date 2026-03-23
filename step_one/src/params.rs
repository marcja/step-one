use nih_plug::prelude::*;

/// User-controllable parameters for StepOne.
///
/// All sequencer state (held notes, arp index, pending NoteOffs, expression
/// stashes) lives on the plugin struct, not here. Params holds only what the
/// user and host can see and automate.
#[derive(Default, Params)]
pub struct StepOneParams {}
