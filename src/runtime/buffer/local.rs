//! Local single-thread CPU buffer.

use crate::runtime::buffer::queued;

/// Local single-thread CPU reader.
pub type Reader<D> = queued::Reader<D, queued::LocalState<D>>;

/// Local single-thread CPU writer.
pub type Writer<D> = queued::Writer<D, queued::LocalState<D>>;
