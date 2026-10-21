// DURT 引擎链入口 (依赖链 spike): 暴露最小探针, 验证 durt-wire / durt-sidecar 可链入。
pub mod engine;
pub mod admission;
pub mod session;
pub mod wire;
pub mod bridge;

pub use engine::engine_probe;
