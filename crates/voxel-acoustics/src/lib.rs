pub mod ai_hearing;
pub mod bands;
pub mod occlusion;
pub mod pathtracer;
pub mod solver;

pub use ai_hearing::{AcousticEvent, AcousticEventKind, PerceivedSound, evaluate_ai_hearing};
pub use bands::{Bands3, SPEED_OF_SOUND_M_S, db_to_linear, linear_to_db};
pub use occlusion::{OcclusionResult, compute_direct_occlusion};
pub use pathtracer::{
    EarlyTap, MAX_EARLY_TAPS, PathTraceConfig, ReverbEstimate, trace_early_reflections,
    trace_listener_reverb,
};
pub use solver::{AcousticSnapshot, AcousticSourceQuery, SourceAcousticParams, solve_acoustics};
