use serde::Deserialize;
use serde::Serialize;

/// Globally unique identifier for a stream buffer connection.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct BufferId(pub usize);

impl From<usize> for BufferId {
    fn from(item: usize) -> Self {
        BufferId(item)
    }
}
