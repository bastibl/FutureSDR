use futures::future::Either;
use std::any::Any;
use std::ops::Deref;
use std::ops::DerefMut;

use crate::runtime::BlockDescription;
use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::BlockPortCtx;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::PortId;
use crate::runtime::Result;
use crate::runtime::block::Block;
use crate::runtime::block::BlockObject;
use crate::runtime::block::LocalBlock;
use crate::runtime::block_inbox::BlockInboxReader;
use crate::runtime::buffer::BufferReader;
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;
use crate::runtime::dev::BlockMeta;
use crate::runtime::dev::Kernel;
use crate::runtime::dev::LocalKernel;
use crate::runtime::dev::LocalWorkIo;
use crate::runtime::dev::MessageOutputs;
use crate::runtime::dev::SendKernel;
use crate::runtime::dev::WorkIo;
use crate::runtime::kernel_interface::KernelInterface;
use crate::runtime::kernel_interface::LocalKernelInterface;
use crate::runtime::kernel_interface::SendKernelInterface;
use futuresdr::runtime::channel::mpsc::Sender;

/// Typed block wrapper around a concrete kernel instance.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct WrappedKernel<K> {
    /// Block metadata
    pub meta: BlockMeta,
    /// Message outputs
    pub mo: MessageOutputs,
    /// Kernel
    pub kernel: K,
    /// Block ID
    pub id: BlockId,
    /// Inbox for Actor Model
    pub inbox: BlockInboxReader,
    /// Sending-side of Inbox
    pub inbox_tx: BlockInbox,
}

/// Typed local block wrapper around a concrete local kernel instance.
pub(crate) struct WrappedLocalKernel<K> {
    /// Block metadata
    pub meta: BlockMeta,
    /// Message outputs
    pub mo: MessageOutputs,
    /// Kernel
    pub kernel: K,
    /// Block ID
    pub id: BlockId,
    /// Inbox for Actor Model
    pub inbox: BlockInboxReader,
    /// Sending-side of Inbox
    pub inbox_tx: BlockInbox,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl<K: KernelInterface + 'static> WrappedKernel<K> {
    /// Create typed block wrapper.
    pub fn new(mut kernel: K, id: BlockId) -> Self {
        let (tx, rx) = crate::runtime::block_inbox::channel(config::config().queue_size);
        kernel.stream_ports_init(id, tx.clone());
        Self {
            meta: BlockMeta::new(),
            mo: MessageOutputs::new(
                id,
                K::message_outputs().iter().map(|x| x.to_string()).collect(),
            ),
            kernel,
            id,
            inbox: rx,
            inbox_tx: tx,
        }
    }

    async fn run_impl(&mut self, main_inbox: Sender<FlowgraphMessage>) -> Result<(), Error>
    where
        K: Kernel,
    {
        let instance_name = self
            .meta
            .instance_name()
            .unwrap_or(K::type_name())
            .to_owned();
        let WrappedKernel {
            meta,
            mo,
            kernel,
            inbox,
            ..
        } = self;

        kernel.stream_ports_validate()?;

        let mut work_io = WorkIo {
            call_again: true,
            finished: false,
            block_on: None,
        };

        loop {
            match inbox
                .recv()
                .await
                .ok_or_else(|| Error::RuntimeError("no msg".to_string()))?
            {
                BlockMessage::Initialize => {
                    match kernel.init(mo, meta).await {
                        Err(e) => {
                            error!(
                                "{}: Error during initialization. Terminating.",
                                instance_name
                            );
                            return Err(Error::RuntimeError(e.to_string()));
                        }
                        _ => {
                            main_inbox
                                .send(FlowgraphMessage::Initialized)
                                .await
                                .map_err(|e| Error::RuntimeError(e.to_string()))?;
                        }
                    }
                    break;
                }
                t => warn!("{} unhandled message during init {:?}", instance_name, t),
            }
        }

        loop {
            work_io.call_again |= inbox.take_pending();
            let mut msg = inbox.try_recv();
            while let Some(m) = msg {
                match m {
                    BlockMessage::BlockDescription { tx } => {
                        let stream_inputs = kernel.stream_inputs();
                        let stream_outputs = kernel.stream_outputs();
                        let message_inputs =
                            K::message_inputs().iter().map(|n| n.to_string()).collect();
                        let message_outputs =
                            K::message_outputs().iter().map(|n| n.to_string()).collect();

                        let description = BlockDescription {
                            id: self.id,
                            type_name: K::type_name().to_string(),
                            instance_name: instance_name.clone(),
                            stream_inputs,
                            stream_outputs,
                            message_inputs,
                            message_outputs,
                            blocking: K::is_blocking(),
                        };
                        if tx.send(description).is_err() {
                            warn!("failed to return BlockDescription, oneshot receiver dropped");
                        }
                    }
                    BlockMessage::StreamInputDone { input_id } => {
                        kernel.stream_input_finish(input_id)?;
                    }
                    BlockMessage::StreamOutputDone { .. } => {
                        work_io.finished = true;
                    }
                    BlockMessage::Call { port_id, data } => {
                        match kernel
                            .call_handler(&mut work_io, mo, meta, port_id, data)
                            .await
                        {
                            Err(Error::InvalidMessagePort(_, port_id)) => {
                                error!(
                                    "{}: BlockMessage::Call -> Invalid Handler {port_id:?}.",
                                    instance_name
                                );
                            }
                            Err(e @ Error::HandlerError(..)) => {
                                error!(
                                    "{}: BlockMessage::Call -> {e}. Terminating.",
                                    instance_name
                                );
                                return Err(e);
                            }
                            _ => {}
                        }
                    }
                    BlockMessage::Callback { port_id, data, tx } => {
                        match kernel
                            .call_handler(&mut work_io, mo, meta, port_id.clone(), data)
                            .await
                        {
                            Err(e @ Error::HandlerError(..)) => {
                                error!(
                                    "{}: BlockMessage::Callback -> {e}. Terminating.",
                                    instance_name
                                );
                                let _ = tx.send(Err(Error::InvalidMessagePort(
                                    BlockPortCtx::Id(self.id),
                                    port_id,
                                )));
                                return Err(e);
                            }
                            res => {
                                let _ = tx.send(res);
                            }
                        }
                    }
                    BlockMessage::Terminate => work_io.finished = true,
                    t => warn!("block unhandled message in main loop {:?}", t),
                };
                work_io.call_again = true;
                msg = inbox.try_recv();
            }

            if work_io.finished {
                debug!("{} terminating ", instance_name);
                kernel.stream_ports_notify_finished().await;
                mo.notify_finished().await;

                match kernel.deinit(mo, meta).await {
                    Ok(_) => {
                        break;
                    }
                    Err(e) => {
                        error!(
                            "{}: Error in deinit (). Terminating. ({:?})",
                            instance_name, e
                        );
                        return Err(Error::RuntimeError(e.to_string()));
                    }
                };
            }

            if !work_io.call_again {
                match work_io.block_on.take() {
                    Some(f) => {
                        if let Either::Right((_, f)) =
                            futures::future::select(f, inbox.notified()).await
                        {
                            work_io.block_on = Some(f);
                        }
                    }
                    _ => {
                        inbox.notified().await;
                    }
                }
                work_io.call_again = true;
                continue;
            }

            work_io.call_again = false;
            if let Err(e) = kernel.work(&mut work_io, mo, meta).await {
                error!("{}: Error in work(). Terminating. ({:?})", instance_name, e);
                return Err(Error::RuntimeError(e.to_string()));
            }
        }

        Ok(())
    }
}

impl<K: LocalKernelInterface + 'static> WrappedLocalKernel<K> {
    /// Create typed local block wrapper.
    pub fn new(mut kernel: K, id: BlockId) -> Self {
        let (tx, rx) = crate::runtime::block_inbox::channel(config::config().queue_size);
        kernel.stream_ports_init(id, tx.clone());
        Self {
            meta: BlockMeta::new(),
            mo: MessageOutputs::new(
                id,
                K::message_outputs().iter().map(|x| x.to_string()).collect(),
            ),
            kernel,
            id,
            inbox: rx,
            inbox_tx: tx,
        }
    }

    async fn run_local_impl(&mut self, main_inbox: Sender<FlowgraphMessage>) -> Result<(), Error>
    where
        K: LocalKernel,
    {
        let instance_name = self
            .meta
            .instance_name()
            .unwrap_or(K::type_name())
            .to_owned();
        let WrappedLocalKernel {
            meta,
            mo,
            kernel,
            inbox,
            ..
        } = self;

        kernel.stream_ports_validate()?;

        let mut work_io = LocalWorkIo {
            call_again: true,
            finished: false,
            block_on: None,
        };

        loop {
            match inbox
                .recv()
                .await
                .ok_or_else(|| Error::RuntimeError("no msg".to_string()))?
            {
                BlockMessage::Initialize => {
                    match LocalKernel::init(kernel, mo, meta).await {
                        Err(e) => {
                            error!(
                                "{}: Error during initialization. Terminating.",
                                instance_name
                            );
                            return Err(Error::RuntimeError(e.to_string()));
                        }
                        _ => {
                            main_inbox
                                .send(FlowgraphMessage::Initialized)
                                .await
                                .map_err(|e| Error::RuntimeError(e.to_string()))?;
                        }
                    }
                    break;
                }
                t => warn!("{} unhandled message during init {:?}", instance_name, t),
            }
        }

        loop {
            work_io.call_again |= inbox.take_pending();
            let mut msg = inbox.try_recv();
            while let Some(m) = msg {
                match m {
                    BlockMessage::BlockDescription { tx } => {
                        let stream_inputs = kernel.stream_inputs();
                        let stream_outputs = kernel.stream_outputs();
                        let message_inputs =
                            K::message_inputs().iter().map(|n| n.to_string()).collect();
                        let message_outputs =
                            K::message_outputs().iter().map(|n| n.to_string()).collect();

                        let description = BlockDescription {
                            id: self.id,
                            type_name: K::type_name().to_string(),
                            instance_name: instance_name.clone(),
                            stream_inputs,
                            stream_outputs,
                            message_inputs,
                            message_outputs,
                            blocking: K::is_blocking(),
                        };
                        if tx.send(description).is_err() {
                            warn!("failed to return BlockDescription, oneshot receiver dropped");
                        }
                    }
                    BlockMessage::StreamInputDone { input_id } => {
                        kernel.stream_input_finish(input_id)?;
                    }
                    BlockMessage::StreamOutputDone { .. } => {
                        work_io.finished = true;
                    }
                    BlockMessage::Call { port_id, data } => {
                        match kernel
                            .call_handler(&mut work_io, mo, meta, port_id, data)
                            .await
                        {
                            Err(Error::InvalidMessagePort(_, port_id)) => {
                                error!(
                                    "{}: BlockMessage::Call -> Invalid Handler {port_id:?}.",
                                    instance_name
                                );
                            }
                            Err(e @ Error::HandlerError(..)) => {
                                error!(
                                    "{}: BlockMessage::Call -> {e}. Terminating.",
                                    instance_name
                                );
                                return Err(e);
                            }
                            _ => {}
                        }
                    }
                    BlockMessage::Callback { port_id, data, tx } => {
                        match kernel
                            .call_handler(&mut work_io, mo, meta, port_id.clone(), data)
                            .await
                        {
                            Err(e @ Error::HandlerError(..)) => {
                                error!(
                                    "{}: BlockMessage::Callback -> {e}. Terminating.",
                                    instance_name
                                );
                                let _ = tx.send(Err(Error::InvalidMessagePort(
                                    BlockPortCtx::Id(self.id),
                                    port_id,
                                )));
                                return Err(e);
                            }
                            res => {
                                let _ = tx.send(res);
                            }
                        }
                    }
                    BlockMessage::Terminate => work_io.finished = true,
                    t => warn!("block unhandled message in main loop {:?}", t),
                };
                work_io.call_again = true;
                msg = inbox.try_recv();
            }

            if work_io.finished {
                debug!("{} terminating ", instance_name);
                kernel.stream_ports_notify_finished().await;
                mo.notify_finished().await;

                match LocalKernel::deinit(kernel, mo, meta).await {
                    Ok(_) => {
                        break;
                    }
                    Err(e) => {
                        error!(
                            "{}: Error in deinit (). Terminating. ({:?})",
                            instance_name, e
                        );
                        return Err(Error::RuntimeError(e.to_string()));
                    }
                };
            }

            if !work_io.call_again {
                match work_io.block_on.take() {
                    Some(f) => {
                        if let Either::Right((_, f)) =
                            futures::future::select(f, inbox.notified()).await
                        {
                            work_io.block_on = Some(f);
                        }
                    }
                    _ => {
                        inbox.notified().await;
                    }
                }
                work_io.call_again = true;
                continue;
            }

            work_io.call_again = false;
            if let Err(e) = LocalKernel::work(kernel, &mut work_io, mo, meta).await {
                error!("{}: Error in work(). Terminating. ({:?})", instance_name, e);
                return Err(Error::RuntimeError(e.to_string()));
            }
        }

        Ok(())
    }
}

impl<K: KernelInterface + 'static> BlockObject for WrappedKernel<K> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn inbox(&self) -> BlockInbox {
        self.inbox_tx.clone()
    }
    fn id(&self) -> BlockId {
        self.id
    }

    fn stream_input(&mut self, id: &PortId) -> Result<&mut dyn BufferReader, Error> {
        self.kernel.stream_input(id)
    }
    fn connect_stream_output(
        &mut self,
        id: &PortId,
        reader: &mut dyn BufferReader,
    ) -> Result<(), Error> {
        self.kernel.connect_stream_output(id, reader)
    }

    fn message_inputs(&self) -> &'static [&'static str] {
        K::message_inputs()
    }
    fn connect(
        &mut self,
        src_port: &PortId,
        dst_box: BlockInbox,
        dst_port: &PortId,
    ) -> Result<(), Error> {
        self.mo.connect(src_port, dst_box, dst_port)
    }

    fn type_name(&self) -> &str {
        K::type_name()
    }
    fn is_blocking(&self) -> bool {
        K::is_blocking()
    }
}

impl<K: LocalKernelInterface + 'static> BlockObject for WrappedLocalKernel<K> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn inbox(&self) -> BlockInbox {
        self.inbox_tx.clone()
    }
    fn id(&self) -> BlockId {
        self.id
    }

    fn stream_input(&mut self, id: &PortId) -> Result<&mut dyn BufferReader, Error> {
        self.kernel.stream_input(id)
    }
    fn connect_stream_output(
        &mut self,
        id: &PortId,
        reader: &mut dyn BufferReader,
    ) -> Result<(), Error> {
        self.kernel.connect_stream_output(id, reader)
    }

    fn message_inputs(&self) -> &'static [&'static str] {
        K::message_inputs()
    }
    fn connect(
        &mut self,
        src_port: &PortId,
        dst_box: BlockInbox,
        dst_port: &PortId,
    ) -> Result<(), Error> {
        self.mo.connect(src_port, dst_box, dst_port)
    }

    fn type_name(&self) -> &str {
        K::type_name()
    }
    fn is_blocking(&self) -> bool {
        K::is_blocking()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<K: SendKernelInterface + SendKernel + 'static> Block for WrappedKernel<K> {
    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>) {
        match self.run_impl(main_inbox.clone()).await {
            Ok(_) => {
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockDone { block_id: self.id })
                    .await;
                return;
            }
            Err(e) => {
                let instance_name = self
                    .meta
                    .instance_name()
                    .unwrap_or("<instance name not set>")
                    .to_string();
                error!("{}: Error in Block.run() {:?}", instance_name, e);
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockError { block_id: self.id })
                    .await;
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<K: KernelInterface + Kernel + 'static> LocalBlock for WrappedKernel<K> {
    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>) {
        match self.run_impl(main_inbox.clone()).await {
            Ok(_) => {
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockDone { block_id: self.id })
                    .await;
                return;
            }
            Err(e) => {
                let instance_name = self
                    .meta
                    .instance_name()
                    .unwrap_or("<instance name not set>")
                    .to_string();
                error!("{}: Error in Block.run() {:?}", instance_name, e);
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockError { block_id: self.id })
                    .await;
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<K: LocalKernelInterface + LocalKernel + 'static> LocalBlock for WrappedLocalKernel<K> {
    async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>) {
        match self.run_local_impl(main_inbox.clone()).await {
            Ok(_) => {
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockDone { block_id: self.id })
                    .await;
                return;
            }
            Err(e) => {
                let instance_name = self
                    .meta
                    .instance_name()
                    .unwrap_or("<instance name not set>")
                    .to_string();
                error!("{}: Error in Block.run() {:?}", instance_name, e);
                let _ = main_inbox
                    .send(FlowgraphMessage::BlockError { block_id: self.id })
                    .await;
            }
        }
    }
}

impl<K> Deref for WrappedKernel<K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.kernel
    }
}

impl<K> DerefMut for WrappedKernel<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.kernel
    }
}

impl<K> Deref for WrappedLocalKernel<K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.kernel
    }
}

impl<K> DerefMut for WrappedLocalKernel<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.kernel
    }
}
