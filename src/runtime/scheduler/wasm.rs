//! WASM Scheduler

use async_task::Runnable;
use concurrent_queue::ConcurrentQueue;
use futures::channel::oneshot;
use futures::future;
use futures::future::Either;
use futures::future::Future;
use futures::future::select;
use slab::Slab;
use std::cell::Cell;
use std::fmt;
use std::panic::RefUnwindSafe;
use std::panic::UnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use web_sys::Worker;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::block::Block;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::scheduler::Scheduler;

static WASM_EXECUTORS: once_cell::sync::Lazy<Mutex<Slab<Arc<WasmExecutor>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Slab::new()));

thread_local! {
    static WASM_WORKER_INDEX: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// Web-worker handle used by the WASM scheduler.
struct WasmWorker(Worker);

// Web workers are created and owned by the scheduler, but the scheduler itself
// has to satisfy the generic `Scheduler: Send` bound. The actual worker handle
// is only used to post the initial message and to terminate the worker at drop.
unsafe impl Send for WasmWorker {}
unsafe impl Sync for WasmWorker {}

impl WasmWorker {
    fn terminate(self) {
        self.0.terminate();
    }
}

/// WASM scheduler worker entry point.
///
/// Application worker scripts should call this after initializing the generated
/// wasm-bindgen module with the `module` and `memory` values sent by
/// [`WasmScheduler::with_worker_script`].
#[wasm_bindgen]
pub fn futuresdr_wasm_scheduler_worker_entry(executor_id: usize, worker_index: usize) {
    WASM_WORKER_INDEX.with(|index| index.set(worker_index));
    let executor = WASM_EXECUTORS.lock().unwrap().get(executor_id).cloned();
    if let Some(executor) = executor {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(executor.run_until_shutdown(worker_index));
        }));
        if result.is_err() {
            error!("WASM scheduler worker panicked: {:?}", result);
        }
    } else {
        error!(
            "WASM scheduler worker got invalid executor id {}",
            executor_id
        );
    }
}

/// WASM scheduler.
///
/// The default scheduler remains single-threaded and uses the browser-local
/// executor. Use [`WasmScheduler::new`] with a thread count greater than one to
/// start a small pool of worker threads. On wasm these threads are backed by
/// web workers when the application is built with wasm thread support.
#[derive(Clone, Debug)]
pub struct WasmScheduler {
    inner: Arc<WasmSchedulerInner>,
}

struct WasmSchedulerInner {
    executor_id: Option<usize>,
    executor: Option<Arc<WasmExecutor>>,
    workers: Vec<WasmWorker>,
}

impl fmt::Debug for WasmSchedulerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmSchedulerInner")
            .field("workers", &self.workers.len())
            .finish()
    }
}

impl Drop for WasmSchedulerInner {
    fn drop(&mut self) {
        if let Some(executor) = &self.executor {
            executor.shutdown();
        }

        for worker in self.workers.drain(..) {
            worker.terminate();
        }

        if let Some(id) = self.executor_id.take() {
            let _ = WASM_EXECUTORS.lock().unwrap().try_remove(id);
        }
    }
}

impl WasmScheduler {
    /// Create a scheduler with `n_threads` worker threads.
    ///
    /// `n_threads <= 1` preserves the old single-threaded behaviour and runs
    /// tasks with `wasm_bindgen_futures::spawn_local`. Larger values start a
    /// web-worker pool and route send-capable tasks to it.
    ///
    /// Multi-threaded mode expects an application-provided worker script at
    /// `./futuresdr-wasm-scheduler-worker.js`. Use
    /// [`WasmScheduler::with_worker_script`] to override the path.
    pub fn new(n_threads: usize) -> WasmScheduler {
        Self::with_worker_script(n_threads, "./futuresdr-wasm-scheduler-worker.js")
    }

    /// Create a scheduler with `n_threads` web workers using `worker_script`.
    ///
    /// The worker script has to initialize the same wasm module/memory and call
    /// [`futuresdr_wasm_scheduler_worker_entry`] with the `executor_id` and
    /// `worker_index` values from the init message.
    pub fn with_worker_script(n_threads: usize, worker_script: &str) -> WasmScheduler {
        if n_threads <= 1 {
            return WasmScheduler::single_threaded();
        }

        let executor = Arc::new(WasmExecutor::new(n_threads));
        let executor_id = WASM_EXECUTORS.lock().unwrap().insert(executor.clone());
        let mut workers = Vec::with_capacity(n_threads);

        for worker_index in 0..n_threads {
            match spawn_worker(worker_script, executor_id, worker_index) {
                Ok(worker) => workers.push(worker),
                Err(e) => {
                    warn!("failed to spawn WASM scheduler web worker: {:?}", e);
                    break;
                }
            }
        }

        if workers.is_empty() {
            let _ = WASM_EXECUTORS.lock().unwrap().try_remove(executor_id);
            return WasmScheduler::single_threaded();
        }

        WasmScheduler {
            inner: Arc::new(WasmSchedulerInner {
                executor_id: Some(executor_id),
                executor: Some(executor),
                workers,
            }),
        }
    }

    /// Create a single-threaded scheduler that runs tasks on the local browser executor.
    pub fn single_threaded() -> WasmScheduler {
        WasmScheduler {
            inner: Arc::new(WasmSchedulerInner {
                executor_id: None,
                executor: None,
                workers: Vec::new(),
            }),
        }
    }

    /// Return the number of worker threads used by this scheduler.
    pub fn n_threads(&self) -> usize {
        self.inner.workers.len().max(1)
    }

    fn map_block(block: usize, n_blocks: usize, n_threads: usize) -> usize {
        let n = n_blocks / n_threads;
        let r = n_blocks % n_threads;

        for x in 1..n_threads {
            if block < (x * n) + std::cmp::min(x, r) {
                return x - 1;
            }
        }

        n_threads - 1
    }
}

impl Scheduler for WasmScheduler {
    fn run_domain(
        &self,
        blocks: Vec<Box<dyn Block>>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, Box<dyn Block>)>> {
        let n_blocks = blocks.len();
        let n_threads = self.inner.workers.len();
        let mut tasks = Vec::with_capacity(n_blocks);

        for block in blocks {
            debug_assert!(
                !block.is_blocking(),
                "blocking blocks must remain on the local runtime path"
            );
            let main_channel = main_channel.clone();
            let executor = if n_threads > 0 {
                Some(WasmScheduler::map_block(block.id().0, n_blocks, n_threads))
            } else {
                None
            };
            tasks.push(spawn_wasm_block(
                self.inner.executor.as_deref(),
                block,
                main_channel,
                executor,
            ));
        }

        tasks
    }

    fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        if let Some(executor) = &self.inner.executor {
            Task::executor(executor.spawn(future))
        } else {
            Task::spawn_local(future)
        }
    }
}

impl Default for WasmScheduler {
    fn default() -> Self {
        Self::single_threaded()
    }
}

fn spawn_worker(
    worker_script: &str,
    executor_id: usize,
    worker_index: usize,
) -> Result<WasmWorker, JsValue> {
    let worker = Worker::new(worker_script)?;
    let init = js_sys::Object::new();

    js_sys::Reflect::set(
        &init,
        &JsValue::from_str("type"),
        &JsValue::from_str("futuresdr-wasm-scheduler-init"),
    )?;
    js_sys::Reflect::set(&init, &JsValue::from_str("module"), &wasm_bindgen::module())?;
    js_sys::Reflect::set(&init, &JsValue::from_str("memory"), &wasm_bindgen::memory())?;
    js_sys::Reflect::set(
        &init,
        &JsValue::from_str("executor_id"),
        &JsValue::from_f64(executor_id as f64),
    )?;
    js_sys::Reflect::set(
        &init,
        &JsValue::from_str("worker_index"),
        &JsValue::from_f64(worker_index as f64),
    )?;

    if let Err(e) = worker.post_message(&init) {
        worker.terminate();
        return Err(e);
    }

    Ok(WasmWorker(worker))
}

fn spawn_wasm_block(
    executor: Option<&WasmExecutor>,
    block: Box<dyn Block>,
    main_channel: Sender<FlowgraphMessage>,
    queue_index: Option<usize>,
) -> Task<(BlockId, Box<dyn Block>)> {
    let future = async move {
        let mut block = block;
        let id = block.id();
        block.run(main_channel).await;
        (id, block)
    };

    if let (Some(executor), Some(queue_index)) = (executor, queue_index) {
        Task::executor(executor.spawn_executor(future, queue_index))
    } else {
        Task::spawn_local(future)
    }
}

/// WASM async task.
pub struct Task<T>(TaskInner<T>);

enum TaskInner<T> {
    Executor(async_task::Task<T>),
    Local(oneshot::Receiver<T>),
}

impl<T: 'static> Task<T> {
    fn executor(task: async_task::Task<T>) -> Self {
        Task(TaskInner::Executor(task))
    }

    pub(crate) fn spawn_local(future: impl Future<Output = T> + 'static) -> Self {
        let (tx, rx) = oneshot::channel::<T>();
        wasm_bindgen_futures::spawn_local(async move {
            let t = future.await;
            if tx.send(t).is_err() {
                debug!("task cannot deliver final result");
            }
        });

        Task(TaskInner::Local(rx))
    }
}

impl<T> Task<T> {
    /// Detach from task.
    pub fn detach(self) {
        if let TaskInner::Executor(task) = self.0 {
            task.detach();
        }
    }
}

impl<T> std::future::Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().0 {
            TaskInner::Executor(task) => Pin::new(task).poll(cx),
            TaskInner::Local(rx) => match Pin::new(rx).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(v)) => Poll::Ready(v),
                Poll::Ready(Err(_)) => panic!("Task canceled"),
            },
        }
    }
}

/// A small async executor with per-worker local queues.
struct WasmExecutor {
    state: once_cell::sync::OnceCell<Arc<State>>,
    worker_count: usize,
}

const LOCAL_QUEUE_CAPACITY: usize = 512;

impl UnwindSafe for WasmExecutor {}
impl RefUnwindSafe for WasmExecutor {}

impl WasmExecutor {
    const fn new(worker_count: usize) -> WasmExecutor {
        WasmExecutor {
            state: once_cell::sync::OnceCell::new(),
            worker_count,
        }
    }

    fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> async_task::Task<T> {
        let mut active = self.state().active.lock().unwrap();

        let entry = active.vacant_entry();
        let key = entry.key();
        let state = self.state().clone();
        let future = async move {
            let _guard = CallOnDrop(move || drop(state.active.lock().unwrap().try_remove(key)));
            future.await
        };

        let (runnable, task) = unsafe { async_task::spawn_unchecked(future, self.schedule()) };
        entry.insert(runnable.waker());

        runnable.schedule();
        task
    }

    fn spawn_executor<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
        executor: usize,
    ) -> async_task::Task<T> {
        let mut active = self.state().active.lock().unwrap();

        let entry = active.vacant_entry();
        let key = entry.key();
        let state = self.state().clone();
        let future = async move {
            let _guard = CallOnDrop(move || drop(state.active.lock().unwrap().try_remove(key)));
            future.await
        };

        let local = self
            .state()
            .local_queues
            .get(executor)
            .cloned()
            .expect("executor queue not initialized");

        let (runnable, task) =
            unsafe { async_task::spawn_unchecked(future, self.schedule_executor(local, executor)) };
        entry.insert(runnable.waker());

        runnable.schedule();
        task
    }

    async fn run_on<T>(&self, worker_index: usize, future: impl Future<Output = T>) -> T {
        let mut runner = Runner::new(self.state(), worker_index);

        let run_forever = async {
            loop {
                for _ in 0..200 {
                    let runnable = runner.runnable().await;
                    runnable.run();
                }
                yield_now().await;
            }
        };

        futures::pin_mut!(future);
        futures::pin_mut!(run_forever);

        match select(future, run_forever).await {
            Either::Left((v, _other)) => v,
            Either::Right((v, _other)) => v,
        }
    }

    async fn run_until_shutdown(&self, worker_index: usize) {
        let state = self.state().clone();
        self.run_on(
            worker_index,
            future::poll_fn(move |cx| {
                if state.shutdown.load(Ordering::Acquire) {
                    return Poll::Ready(());
                }

                if let Some(signal) = state.worker_signals.get(worker_index) {
                    signal.waker.register(cx.waker());
                }

                if state.shutdown.load(Ordering::Acquire) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )
        .await;
    }

    fn shutdown(&self) {
        let state = self.state();
        state.shutdown.store(true, Ordering::Release);
        for signal in &state.worker_signals {
            signal.sleeping.store(false, Ordering::Release);
            signal.waker.wake();
        }
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.state().clone();

        move |runnable| {
            state.queue.push(runnable).unwrap();
            state.notify();
        }
    }

    fn schedule_executor(
        &self,
        local: Arc<ConcurrentQueue<Runnable>>,
        executor: usize,
    ) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.state().clone();

        move |runnable| {
            if let Err(err) = local.push(runnable) {
                state.queue.push(err.into_inner()).unwrap();
                state.notify();
                return;
            }
            let _ = state.wake_worker(executor);
        }
    }

    fn state(&self) -> &Arc<State> {
        self.state
            .get_or_init(|| Arc::new(State::new(self.worker_count)))
    }
}

impl Drop for WasmExecutor {
    fn drop(&mut self) {
        debug!("dropping WASM executor");
        if let Some(state) = self.state.get() {
            let active = state.active.lock().unwrap();

            for (_, w) in active.iter() {
                w.wake_by_ref();
            }

            drop(active);

            while state.queue.pop().is_ok() {}

            for q in state.local_queues.iter() {
                while q.pop().is_ok() {}
            }
        }
    }
}

struct State {
    queue: ConcurrentQueue<Runnable>,
    shutdown: AtomicBool,
    local_queues: Vec<Arc<ConcurrentQueue<Runnable>>>,
    worker_signals: Vec<Arc<WorkerSignal>>,
    next_wake: AtomicUsize,
    active: Mutex<Slab<Waker>>,
}

impl State {
    fn new(worker_count: usize) -> State {
        let local_queues: Vec<_> = (0..worker_count)
            .map(|_| Arc::new(ConcurrentQueue::bounded(LOCAL_QUEUE_CAPACITY)))
            .collect();
        let worker_signals: Vec<_> = (0..worker_count)
            .map(|_| Arc::new(WorkerSignal::default()))
            .collect();

        State {
            queue: ConcurrentQueue::unbounded(),
            shutdown: AtomicBool::new(false),
            local_queues,
            worker_signals,
            next_wake: AtomicUsize::new(0),
            active: Mutex::new(Slab::new()),
        }
    }

    #[inline]
    fn notify(&self) {
        let n = self.worker_signals.len();
        if n == 0 {
            return;
        }
        let start = self.next_wake.fetch_add(1, Ordering::Relaxed) % n;
        for off in 0..n {
            let idx = (start + off) % n;
            if self.wake_worker(idx) {
                break;
            }
        }
    }

    #[inline]
    fn wake_worker(&self, queue_index: usize) -> bool {
        if queue_index >= self.worker_signals.len() {
            return false;
        }
        let signal = &self.worker_signals[queue_index];
        if signal.sleeping.swap(false, Ordering::AcqRel) {
            signal.waker.wake();
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
struct WorkerSignal {
    sleeping: AtomicBool,
    waker: futures::task::AtomicWaker,
}

struct Ticker<'a> {
    signal: &'a WorkerSignal,
}

impl Ticker<'_> {
    fn new(signal: &WorkerSignal) -> Ticker<'_> {
        Ticker { signal }
    }

    async fn runnable_with(&mut self, mut search: impl FnMut() -> Option<Runnable>) -> Runnable {
        future::poll_fn(|cx| {
            loop {
                if let Some(r) = search() {
                    self.signal.sleeping.store(false, Ordering::Release);
                    return Poll::Ready(r);
                }

                self.signal.sleeping.store(true, Ordering::Release);
                self.signal.waker.register(cx.waker());

                if !self.signal.sleeping.load(Ordering::Acquire) {
                    continue;
                }

                if let Some(r) = search() {
                    self.signal.sleeping.store(false, Ordering::Release);
                    return Poll::Ready(r);
                }

                return Poll::Pending;
            }
        })
        .await
    }
}

impl Drop for Ticker<'_> {
    fn drop(&mut self) {
        self.signal.sleeping.store(false, Ordering::Release);
    }
}

struct Runner<'a> {
    state: &'a State,
    ticker: Ticker<'a>,
    local: Arc<ConcurrentQueue<Runnable>>,
}

impl Runner<'_> {
    fn new(state: &State, worker_index: usize) -> Runner<'_> {
        let local = state
            .local_queues
            .get(worker_index)
            .cloned()
            .expect("worker local queue not initialized");
        let signal = state
            .worker_signals
            .get(worker_index)
            .expect("worker signal not initialized");

        Runner {
            state,
            ticker: Ticker::new(signal),
            local,
        }
    }

    async fn runnable(&mut self) -> Runnable {
        self.ticker
            .runnable_with(|| {
                if let Ok(r) = self.local.pop() {
                    return Some(r);
                }

                if let Ok(r) = self.state.queue.pop() {
                    return Some(r);
                }

                None
            })
            .await
    }
}

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn yield_now() -> YieldNow {
    YieldNow(false)
}

#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
