//! Flowgraph Scheduler Trait and Implementations

#[cfg(feature = "flow_scheduler")]
mod flow;
#[cfg(feature = "flow_scheduler")]
pub use crate::runtime::scheduler::flow::FlowScheduler;

#[cfg(not(target_arch = "wasm32"))]
mod smol;
#[cfg(not(target_arch = "wasm32"))]
pub use crate::runtime::scheduler::smol::SmolScheduler;
#[cfg(not(target_arch = "wasm32"))]
mod local;
#[cfg(not(target_arch = "wasm32"))]
pub use local::ThreadLocalScheduler;

#[allow(clippy::module_inception)]
mod scheduler;
#[cfg(not(target_arch = "wasm32"))]
pub use scheduler::LocalScheduler;
#[cfg(not(target_arch = "wasm32"))]
pub use scheduler::LocalTask;
pub use scheduler::Scheduler;

#[cfg(target_arch = "wasm32")]
pub mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::DummyScheduler;
#[cfg(target_arch = "wasm32")]
pub use wasm::WasmLocalScheduler;
#[cfg(target_arch = "wasm32")]
pub use wasm::WasmScheduler;

#[cfg(not(target_arch = "wasm32"))]
pub use async_task::Task;
#[cfg(target_arch = "wasm32")]
pub use wasm::Task;
