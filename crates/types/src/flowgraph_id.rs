use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Identifier of a flowgraph known to a runtime control handle.
///
/// Runtime handles assign these ids as flowgraphs are registered with the
/// control plane. They are used by the native REST API and by remote clients.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowgraphId(pub usize);

impl From<usize> for FlowgraphId {
    fn from(item: usize) -> Self {
        FlowgraphId(item)
    }
}

impl fmt::Display for FlowgraphId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FlowgraphId({})", self.0)
    }
}
