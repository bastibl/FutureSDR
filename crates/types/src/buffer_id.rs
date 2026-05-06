use serde::Deserialize;
use serde::Serialize;

/// Identifier for a stream buffer connection.
///
/// Runtime descriptions expose stream edges by endpoint, but the runtime keeps
/// a buffer id internally so a concrete stream connection can be tracked even
/// when several edges use the same pair of port names over time.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct BufferId(pub usize);

impl From<usize> for BufferId {
    fn from(item: usize) -> Self {
        BufferId(item)
    }
}
