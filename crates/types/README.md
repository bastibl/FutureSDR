# FutureSDR Types

Shared serializable types used by the FutureSDR runtime, control port, and
remote clients.

This crate contains:

- `Pmt`: the polymorphic message value passed to message handlers and encoded
  by the REST API.
- `BlockId`, `FlowgraphId`, `PortId`, and `BufferId`: lightweight identifiers
  used by control and description APIs.
- `FlowgraphDescription` and `BlockDescription`: serializable descriptions of a
  running or constructed flowgraph.

[![Crates.io][crates-badge]][crates-url]
[![Apache 2.0 licensed][apache-badge]][apache-url]
[![Docs][docs-badge]][docs-url]

[crates-badge]: https://img.shields.io/crates/v/futuresdr-types.svg
[crates-url]: https://crates.io/crates/futuresdr-types
[apache-badge]: https://img.shields.io/badge/license-Apache%202-blue
[apache-url]: https://github.com/futuresdr/futuresdr/blob/master/LICENSE
[docs-badge]: https://img.shields.io/docsrs/futuresdr-types
[docs-url]: https://docs.rs/futuresdr-types/
