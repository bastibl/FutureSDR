use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::Error;
use crate::runtime::PortId;
use crate::runtime::buffer::BufferReader;
use crate::runtime::buffer::BufferWriter;
use crate::runtime::buffer::ConnectionState;
use crate::runtime::buffer::CpuBufferReader;
use crate::runtime::buffer::CpuBufferWriter;
use crate::runtime::buffer::CpuSample;
use crate::runtime::buffer::PortConfig;
use crate::runtime::buffer::PortCore;
use crate::runtime::buffer::PortEndpoint;
use crate::runtime::buffer::Tags;
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;
use crate::runtime::dev::ItemTag;

#[derive(Debug)]
struct BufferEmpty<D: CpuSample> {
    buffer: Box<[D]>,
}

#[derive(Debug)]
struct BufferFull<D: CpuSample> {
    buffer: Box<[D]>,
    items: usize,
    tags: Vec<ItemTag>,
}

#[derive(Debug)]
struct CurrentBuffer<D: CpuSample> {
    buffer: Box<[D]>,
    end_offset: usize,
    offset: usize,
    tags: Vec<ItemTag>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct State<D: CpuSample> {
    writer_input: VecDeque<BufferEmpty<D>>,
    reader_input: VecDeque<BufferFull<D>>,
}

#[doc(hidden)]
pub trait SharedState<D: CpuSample>: Clone + Debug + 'static {
    fn new(state: State<D>) -> Self;
    fn with<R>(&self, f: impl FnOnce(&State<D>) -> R) -> R;
    fn with_mut<R>(&self, f: impl FnOnce(&mut State<D>) -> R) -> R;
}

#[doc(hidden)]
#[derive(Debug)]
pub struct LocalState<D: CpuSample>(Rc<RefCell<State<D>>>);

impl<D: CpuSample> Clone for LocalState<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: CpuSample> SharedState<D> for LocalState<D> {
    fn new(state: State<D>) -> Self {
        Self(Rc::new(RefCell::new(state)))
    }

    fn with<R>(&self, f: impl FnOnce(&State<D>) -> R) -> R {
        f(&self.0.borrow())
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut State<D>) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct SendState<D: CpuSample>(Arc<Mutex<State<D>>>);

impl<D: CpuSample> Clone for SendState<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: CpuSample> SharedState<D> for SendState<D> {
    fn new(state: State<D>) -> Self {
        Self(Arc::new(Mutex::new(state)))
    }

    fn with<R>(&self, f: impl FnOnce(&State<D>) -> R) -> R {
        f(&self.0.lock().unwrap())
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut State<D>) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }
}

/// Queue-backed CPU writer.
#[derive(Debug)]
pub struct Writer<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    core: PortCore,
    state: ConnectionState<ConnectedWriter<D, S>>,
    current: Option<CurrentBuffer<D>>,
    tags: Vec<ItemTag>,
}

#[derive(Debug)]
struct ConnectedWriter<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    state: S,
    reserved_items: usize,
    reader: PortEndpoint,
    _marker: PhantomData<D>,
}

impl<D, S> Writer<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    /// Create a queue-backed CPU writer.
    pub fn new() -> Self {
        Self {
            core: PortCore::with_config(PortConfig::with_min_items(1)),
            state: ConnectionState::disconnected(),
            current: None,
            tags: Vec::new(),
        }
    }
}

impl<D, S> Default for Writer<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<D, S> BufferWriter for Writer<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    type Reader = Reader<D, S>;

    fn init(&mut self, block_id: BlockId, port_id: PortId, inbox: BlockInbox) {
        self.core.init(block_id, port_id, inbox);
    }

    fn validate(&self) -> Result<(), Error> {
        if self.state.is_connected() {
            Ok(())
        } else {
            Err(self.core.not_connected_error())
        }
    }

    fn connect(&mut self, dest: &mut Self::Reader) {
        let buffer_size_configured = self.core.min_buffer_size_in_items().is_some()
            || dest.core.min_buffer_size_in_items().is_some();
        let reserved_items = dest.core.min_items().unwrap_or(0);

        let mut min_items = if buffer_size_configured {
            let min_self = self.core.min_buffer_size_in_items().unwrap_or(0);
            let min_reader = dest.core.min_buffer_size_in_items().unwrap_or(0);
            std::cmp::max(min_self, min_reader)
        } else {
            config::config().buffer_size / size_of::<D>()
        };

        min_items = std::cmp::max(min_items, reserved_items + 1);

        let state = S::new(State {
            writer_input: VecDeque::new(),
            reader_input: VecDeque::new(),
        });
        state.with_mut(|state| {
            for _ in 0..4 {
                state.writer_input.push_back(BufferEmpty {
                    buffer: vec![D::default(); min_items].into_boxed_slice(),
                });
            }
        });

        self.core
            .set_min_buffer_size_in_items(min_items - reserved_items);
        dest.core
            .set_min_buffer_size_in_items(min_items - reserved_items);

        self.state.set_connected(ConnectedWriter {
            state: state.clone(),
            reserved_items,
            reader: PortEndpoint::new(dest.core.inbox(), dest.core.port_id()),
            _marker: PhantomData,
        });
        dest.state.set_connected(ConnectedReader {
            state,
            reserved_items,
            writer: PortEndpoint::new(self.core.inbox(), self.core.port_id()),
            _marker: PhantomData,
        });
    }

    async fn notify_finished(&mut self) {
        let connected = self.state.connected();
        let reserved_items = connected.reserved_items;
        if let Some(CurrentBuffer {
            buffer,
            offset,
            tags,
            ..
        }) = self.current.take()
            && offset > reserved_items
        {
            connected.state.with_mut(|state| {
                state.reader_input.push_back(BufferFull {
                    buffer,
                    items: offset - reserved_items,
                    tags,
                });
            });
        }

        let _ = connected
            .reader
            .inbox()
            .send(BlockMessage::StreamInputDone {
                input_id: connected.reader.port_id(),
            })
            .await;
    }

    fn block_id(&self) -> BlockId {
        self.core.block_id()
    }

    fn port_id(&self) -> PortId {
        self.core.port_id()
    }
}

impl<D, S> CpuBufferWriter for Writer<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    type Item = D;

    fn slice_with_tags(&mut self) -> (&mut [Self::Item], Tags<'_>) {
        if self.current.is_none() {
            let connected = self.state.connected();
            let next = connected
                .state
                .with_mut(|state| state.writer_input.pop_front());
            match next {
                Some(b) => {
                    let end_offset = b.buffer.len();
                    self.current = Some(CurrentBuffer {
                        buffer: b.buffer,
                        offset: connected.reserved_items,
                        end_offset,
                        tags: Vec::new(),
                    });
                }
                None => return (&mut [], Tags::new(&mut self.tags, 0)),
            }
        }

        let c = self.current.as_mut().unwrap();
        (&mut c.buffer[c.offset..], Tags::new(&mut self.tags, 0))
    }

    fn produce(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        let connected = self.state.connected();
        let reserved_items = connected.reserved_items;
        let c = self.current.as_mut().unwrap();
        debug_assert!(n <= c.end_offset - c.offset);
        for t in self.tags.iter_mut() {
            t.index += c.offset;
        }
        c.tags.append(&mut self.tags);
        c.offset += n;

        if (c.end_offset - c.offset) < self.core.min_items().unwrap_or(1) {
            let c = self.current.take().unwrap();
            let has_writer_input = connected.state.with_mut(|state| {
                state.reader_input.push_back(BufferFull {
                    buffer: c.buffer,
                    items: c.offset - reserved_items,
                    tags: c.tags,
                });
                !state.writer_input.is_empty()
            });

            connected.reader.inbox().notify();
            if has_writer_input {
                self.core.inbox().notify();
            }
        }
    }

    fn set_min_items(&mut self, n: usize) {
        if self.state.is_connected() {
            warn!("set_min_items called after buffer is created. this has no effect");
        }
        self.core.set_min_items(n);
    }

    fn set_min_buffer_size_in_items(&mut self, n: usize) {
        if self.state.is_connected() {
            warn!(
                "set_min_buffer_size_in_items called after buffer is created. this has no effect"
            );
        }
        self.core.set_min_buffer_size_in_items(n);
    }

    fn max_items(&self) -> usize {
        self.core.min_buffer_size_in_items().unwrap_or(usize::MAX)
    }
}

/// Queue-backed CPU reader.
#[derive(Debug)]
pub struct Reader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    core: PortCore,
    state: ConnectionState<ConnectedReader<D, S>>,
    current: Option<CurrentBuffer<D>>,
    finished: bool,
}

#[derive(Debug)]
struct ConnectedReader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    state: S,
    reserved_items: usize,
    writer: PortEndpoint,
    _marker: PhantomData<D>,
}

impl<D, S> Reader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    /// Create a queue-backed CPU reader.
    pub fn new() -> Self {
        Self {
            core: PortCore::new_disconnected(),
            state: ConnectionState::disconnected(),
            current: None,
            finished: false,
        }
    }
}

impl<D, S> Default for Reader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<D, S> BufferReader for Reader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn init(&mut self, block_id: BlockId, port_id: PortId, inbox: BlockInbox) {
        self.core.init(block_id, port_id, inbox);
    }

    fn validate(&self) -> Result<(), Error> {
        if self.state.is_connected() {
            Ok(())
        } else {
            Err(self.core.not_connected_error())
        }
    }

    async fn notify_finished(&mut self) {
        let connected = self.state.connected();
        let _ = connected
            .writer
            .inbox()
            .send(BlockMessage::StreamOutputDone {
                output_id: connected.writer.port_id(),
            })
            .await;
    }

    fn finish(&mut self) {
        self.finished = true;
    }

    fn finished(&self) -> bool {
        self.finished
            && self
                .state
                .as_ref()
                .is_none_or(|state| state.state.with(|state| state.reader_input.is_empty()))
    }

    fn block_id(&self) -> BlockId {
        self.core.block_id()
    }

    fn port_id(&self) -> PortId {
        self.core.port_id()
    }
}

impl<D, S> CpuBufferReader for Reader<D, S>
where
    D: CpuSample,
    S: SharedState<D>,
{
    type Item = D;

    fn slice_with_tags(&mut self) -> (&[Self::Item], &Vec<ItemTag>) {
        let connected = self.state.connected();
        let reserved_items = connected.reserved_items;

        if let Some(cur) = self.current.as_mut() {
            let left = cur.end_offset - cur.offset;
            debug_assert!(left > 0);
            if left <= reserved_items {
                let next = connected
                    .state
                    .with_mut(|state| state.reader_input.pop_front());
                if let Some(BufferFull {
                    mut buffer,
                    mut tags,
                    items,
                }) = next
                {
                    buffer[(reserved_items - left)..reserved_items]
                        .clone_from_slice(&cur.buffer[cur.offset..(cur.offset + left)]);

                    for t in tags.iter_mut() {
                        t.index += left;
                    }
                    cur.tags.append(&mut tags);

                    let old = std::mem::replace(&mut cur.buffer, buffer);
                    connected.state.with_mut(|state| {
                        state.writer_input.push_back(BufferEmpty { buffer: old })
                    });
                    connected.writer.inbox().notify();

                    cur.end_offset = reserved_items + items;
                    cur.offset = reserved_items - left;
                }
            }
        } else {
            let next = connected
                .state
                .with_mut(|state| state.reader_input.pop_front());
            match next {
                Some(b) => {
                    let end_offset = b.items + reserved_items;
                    self.current = Some(CurrentBuffer {
                        buffer: b.buffer,
                        offset: reserved_items,
                        end_offset,
                        tags: b.tags,
                    });
                }
                None => {
                    static V: Vec<ItemTag> = vec![];
                    return (&[], &V);
                }
            }
        }

        let c = self.current.as_mut().unwrap();
        (&c.buffer[c.offset..c.end_offset], &c.tags)
    }

    fn consume(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        let connected = self.state.connected();
        let reserved_items = connected.reserved_items;
        let c = self.current.as_mut().unwrap();
        debug_assert!(n <= c.end_offset - c.offset);
        c.offset += n;

        if c.offset == c.end_offset {
            let b = self.current.take().unwrap();
            let has_reader_input = connected.state.with_mut(|state| {
                state
                    .writer_input
                    .push_back(BufferEmpty { buffer: b.buffer });
                !state.reader_input.is_empty()
            });

            connected.writer.inbox().notify();
            if has_reader_input {
                self.core.inbox().notify();
            }
        } else if c.end_offset - c.offset <= reserved_items
            && connected.state.with(|state| !state.reader_input.is_empty())
        {
            self.core.inbox().notify();
        }
    }

    fn set_min_items(&mut self, n: usize) {
        if self.state.is_connected() {
            warn!("buffer size configured after buffer is connected. This has no effect");
        }
        self.core.set_min_items(n);
    }

    fn set_min_buffer_size_in_items(&mut self, n: usize) {
        if self.state.is_connected() {
            warn!("buffer size configured after buffer is connected. This has no effect");
        }
        self.core.set_min_buffer_size_in_items(n);
    }

    fn max_items(&self) -> usize {
        self.core.min_buffer_size_in_items().unwrap_or(usize::MAX)
    }
}
