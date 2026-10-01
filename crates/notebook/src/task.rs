//! The sync worker's and background's loops: threads natively, and in the browser, which
//! gives a module one thread, tasks on its event loop. A loop is an `async` block whose only
//! waits are `wait`; natively each wait blocks its thread, so `complete` runs the block to the
//! end in one poll.

use std::{io, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::{
    sync::mpsc::{Receiver, SyncSender as Sender},
    thread::JoinHandle,
};

/// A wake that waits, at most one, as `mpsc::sync_channel(1)` keeps one.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn channel() -> (Sender<()>, Receiver<()>) {
    std::sync::mpsc::sync_channel(1)
}

/// Waits for a wake or `timeout`, without one for ever.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn wait(receiver: &Receiver<()>, timeout: Option<Duration>) {
    let _ = match timeout {
        Some(timeout) => receiver.recv_timeout(timeout).ok(),
        None => receiver.recv().ok(),
    };
}

/// Runs the loop `start` makes, whose waits block, on a thread of its own named `name`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn<T: Send + 'static, F: Future<Output = T>>(
    name: &str,
    start: impl FnOnce() -> F + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || complete(start()))
}

/// `work`'s output, where nothing it awaits is pending.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn complete<T>(work: impl Future<Output = T>) -> T {
    let mut work = std::pin::pin!(work);
    match work
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => unreachable!("Native waits block rather than pend"),
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::*;

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        task::{Poll, Waker},
    };
    use wasm_bindgen::{JsCast, prelude::*};

    #[derive(Default)]
    struct Bell {
        rung: bool,
        waiting: Option<Waker>,
    }

    pub(crate) struct Sender<T>(Arc<Mutex<Bell>>, std::marker::PhantomData<T>);
    pub(crate) struct Receiver<T>(Arc<Mutex<Bell>>, std::marker::PhantomData<T>);

    pub(crate) fn channel() -> (Sender<()>, Receiver<()>) {
        let bell = Arc::new(Mutex::new(Bell::default()));
        (
            Sender(Arc::clone(&bell), Default::default()),
            Receiver(bell, Default::default()),
        )
    }

    impl Sender<()> {
        pub(crate) fn try_send(&self, (): ()) -> Result<(), ()> {
            let waiting = {
                let mut bell = self.0.lock().map_err(|_| ())?;
                bell.rung = true;
                bell.waiting.take()
            };
            if let Some(waiting) = waiting {
                waiting.wake();
            }
            Ok(())
        }
    }

    /// Wakes `waker` after `delay` through the global scope's `setTimeout`.
    fn after(delay: Duration, waker: Waker) {
        let global = js_sys::global();
        let Ok(set_timeout) = js_sys::Reflect::get(&global, &"setTimeout".into())
            .and_then(|function| function.dyn_into::<js_sys::Function>())
        else {
            return waker.wake();
        };
        let wake = Closure::once_into_js(move || waker.wake());
        let _ = set_timeout.call2(&global, &wake, &(delay.as_secs_f64() * 1e3).into());
    }

    pub(crate) async fn wait(receiver: &Receiver<()>, timeout: Option<Duration>) {
        let deadline = timeout.map(|timeout| web_time::Instant::now() + timeout);
        let mut armed = false;
        std::future::poll_fn(|context| {
            let Ok(mut bell) = receiver.0.lock() else {
                return Poll::Ready(());
            };
            if std::mem::take(&mut bell.rung)
                || deadline.is_some_and(|deadline| deadline <= web_time::Instant::now())
            {
                return Poll::Ready(());
            }
            bell.waiting = Some(context.waker().clone());
            if let Some(deadline) = deadline
                && !std::mem::replace(&mut armed, true)
            {
                after(
                    deadline.saturating_duration_since(web_time::Instant::now()),
                    context.waker().clone(),
                );
            }
            Poll::Pending
        })
        .await
    }

    /// A task the browser's event loop runs. It cannot be waited for, and ends on its own.
    pub(crate) struct JoinHandle<T>(Arc<Mutex<Option<T>>>);

    impl<T> JoinHandle<T> {
        /// What the task ended with, if it has ended.
        pub(crate) fn finished(self) -> Option<T> {
            self.0.lock().ok()?.take()
        }
    }

    pub(crate) fn spawn<T: 'static, F: Future<Output = T> + 'static>(
        _: &str,
        start: impl FnOnce() -> F,
    ) -> io::Result<JoinHandle<T>> {
        let output = Arc::new(Mutex::new(None));
        let ended = Arc::clone(&output);
        let work = start();
        wasm_bindgen_futures::spawn_local(async move {
            let result = work.await;
            if let Ok(mut ended) = ended.lock() {
                *ended = Some(result);
            }
        });
        Ok(JoinHandle(output))
    }
}
