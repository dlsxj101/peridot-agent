//! Re-exports the LLM-based grader from `peridot-grader`.
//!
//! The grader was relocated to its own crate so that `peridot-verify` can
//! invoke it from the deterministic verify pipeline without forcing a
//! `peridot-verify → peridot-core` dependency edge (which would have
//! created a cycle through `peridot-core → peridot-llm` plus the shared
//! grader). This shim keeps the `peridot_core::grader::*` import path
//! working for existing callers (see [`crate::agent`] and the
//! `auto_grade_on_done` integration test).

pub use peridot_grader::{GraderVerdict, grade_work};
