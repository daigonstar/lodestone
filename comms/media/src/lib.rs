//! Lodestone Comms media: the call engine and end-to-end frame encryption. Audio input and output come
//! from GStreamer in Comms (see main.rs for the phase 1 checks); the engine itself doesn't depend on
//! where sound comes from, which keeps it portable (Windows later) and testable.

pub mod call;
pub mod frames;
pub mod audio;
