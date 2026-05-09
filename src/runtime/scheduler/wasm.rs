//! WASM Scheduler

use async_task::Runnable;
use concurrent_queue::ConcurrentQueue;
use futures::channel::oneshot;
use futures::future::Future;
use gloo_timers::future::TimeoutFuture;
use slab::Slab;
use std::fmt;
use std::panic::RefUnwindSafe;
use std::panic::UnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use web_sys::Worker;
use web_sys::WorkerOptions;
use web_sys::WorkerType;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::block::Block;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::scheduler::Scheduler;

static WASM_EXECUTORS: once_cell::sync::Lazy<Mutex<Slab<Arc<WasmExecutor>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Slab::new()));

unsafe extern "C" {
    static __heap_base: u8;
}

fn reset_wasm_thread_metadata() {
    let base = core::ptr::addr_of!(__heap_base) as usize;
    if base < 4 {
        return;
    }

    unsafe {
        // wasm-bindgen's thread transform stores its data-init guard at
        // __heap_base - 4, the thread count at __heap_base, and a malloc lock
        // at __heap_base + 4. With current rust/wasm-bindgen output, the main
        // thread's TLS allocation can overwrite these words. Restore them
        // before starting scheduler workers.
        AtomicU32::from_ptr((base - 4) as *mut u32).store(2, Ordering::Release);
        AtomicU32::from_ptr(base as *mut u32).store(1, Ordering::Release);
        AtomicU32::from_ptr((base + 4) as *mut u32).store(0, Ordering::Release);
    }
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
    let executor = WASM_EXECUTORS.lock().unwrap().get(executor_id).cloned();
    if let Some(executor) = executor {
        wasm_bindgen_futures::spawn_local(async move {
            executor.run_until_shutdown(worker_index).await;
        });
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

        info!("WASM scheduler starting {n_threads} web workers with script {worker_script}");
        reset_wasm_thread_metadata();

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

        info!("WASM scheduler started {} web workers", workers.len());
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

        for (block_index, block) in blocks.into_iter().enumerate() {
            debug_assert!(
                !block.is_blocking(),
                "blocking blocks must remain on the local runtime path"
            );
            let main_channel = main_channel.clone();
            let worker_index = (n_threads > 0).then(|| block_index * n_threads / n_blocks);
            tasks.push(spawn_wasm_block(
                self.inner.executor.as_deref(),
                block,
                main_channel,
                worker_index,
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
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let worker = Worker::new_with_options(worker_script, &options)?;
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
            unsafe { async_task::spawn_unchecked(future, self.schedule_executor(local)) };
        entry.insert(runnable.waker());

        runnable.schedule();
        task
    }

    async fn run_until_shutdown(&self, worker_index: usize) {
        let state = self.state().clone();
        let mut runner = Runner::new(self.state(), worker_index);

        while !state.shutdown.load(Ordering::Acquire) {
            let mut ran = false;
            for _ in 0..200 {
                let Some(runnable) = runner.try_runnable() else {
                    break;
                };
                runnable.run();
                ran = true;
            }

            if ran {
                yield_now().await;
            } else {
                TimeoutFuture::new(1).await;
            }
        }
    }

    fn shutdown(&self) {
        self.state().shutdown.store(true, Ordering::Release);
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.state().clone();

        move |runnable| {
            state.queue.push(runnable).unwrap();
        }
    }

    fn schedule_executor(
        &self,
        local: Arc<ConcurrentQueue<Runnable>>,
    ) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.state().clone();

        move |runnable| {
            if let Err(err) = local.push(runnable) {
                state.queue.push(err.into_inner()).unwrap();
            }
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
    active: Mutex<Slab<Waker>>,
}

impl State {
    fn new(worker_count: usize) -> State {
        let local_queues: Vec<_> = (0..worker_count)
            .map(|_| Arc::new(ConcurrentQueue::bounded(LOCAL_QUEUE_CAPACITY)))
            .collect();

        State {
            queue: ConcurrentQueue::unbounded(),
            shutdown: AtomicBool::new(false),
            local_queues,
            active: Mutex::new(Slab::new()),
        }
    }
}

struct Runner<'a> {
    state: &'a State,
    local: Arc<ConcurrentQueue<Runnable>>,
}

impl Runner<'_> {
    fn new(state: &State, worker_index: usize) -> Runner<'_> {
        let local = state
            .local_queues
            .get(worker_index)
            .cloned()
            .expect("worker local queue not initialized");

        Runner { state, local }
    }

    fn try_runnable(&mut self) -> Option<Runnable> {
        if let Ok(r) = self.local.pop() {
            return Some(r);
        }

        if let Ok(r) = self.state.queue.pop() {
            return Some(r);
        }

        None
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
