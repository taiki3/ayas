pub mod dataset;
pub mod evaluator;
pub mod judge;
pub mod runner;

pub mod prelude {
    pub use crate::dataset::{Dataset, Example};
    pub use crate::evaluator::{
        ContainsEvaluator, EvalResult, EvalScore, Evaluator, ExactMatchEvaluator,
    };
    pub use crate::judge::LlmJudge;
    pub use crate::runner::{EvalReport, EvalRunner};
}
