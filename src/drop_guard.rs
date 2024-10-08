use std::future::Future;
use std::marker::PhantomData;
use tokio::runtime::Handle;
use tokio::task;
use tokio::task::JoinHandle;

pub struct DropGuard<'a, T: FnOnce() -> U + 'a, U> {
    f: Option<T>,
    _phantom: PhantomData<&'a U>,
}

impl<'a, T: FnOnce() -> U + 'a, U> DropGuard<'a, T, U> {
    pub fn new(f: T) -> Self {
        Self {
            f: Some(f),
            _phantom: PhantomData,
        }
    }

    pub fn disarm(mut self) {
        self.f.take();
    }
}

pub fn new_async<F: Future + Send + 'static>(
    f: F,
) -> DropGuard<'static, impl FnOnce() -> JoinHandle<()>, JoinHandle<()>> {
    let run = move || {
        task::spawn_blocking(move || {
            Handle::current().block_on(f);
        })
    };

    DropGuard {
        f: Some(run),
        _phantom: PhantomData,
    }
}

impl<'a, T: FnOnce() -> U + 'a, U> Drop for DropGuard<'a, T, U> {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            f();
        }
    }
}
