//! intent-vim: a pure, UI-independent modal editing engine implementing the
//! Vim grammar subset defined in SPEC §4.2 — motions, operators, text
//! objects, counts, visual modes, undo/redo, repeat, and the two-slot
//! clipboard model from SPEC §4.5. Nothing here touches a windowing system.

pub mod buffer;
pub mod engine;
pub mod key;
pub mod register;

pub use buffer::Pos;
pub use engine::{Engine, EngineEvent, Mode};
pub use key::Key;
pub use register::{Clipboard, MemClipboard, Register};
