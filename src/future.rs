//! Asynchronous values.

use core::cell::Cell;
use core::marker::Unpin;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll};
use core::ops::{Drop, Generator, GeneratorState};

#[doc(inline)]
pub use core::future::*;

/// Wrap a generator in a future.
///
/// This function returns a `GenFuture` underneath, but hides it in `impl Trait` to give
/// better error messages (`impl Future` rather than `GenFuture<[closure.....]>`).
#[doc(hidden)]
pub fn from_generator<T: Generator<Yield = ()>>(x: T) -> impl Future<Output = T::Return> {
    GenFuture(x)
}

/// A wrapper around generators used to implement `Future` for `async`/`await` code.
#[doc(hidden)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct GenFuture<T: Generator<Yield = ()>>(T);

// We rely on the fact that async/await futures are immovable in order to create
// self-referential borrows in the underlying generator.
impl<T: Generator<Yield = ()>> !Unpin for GenFuture<T> {}

#[doc(hidden)]
impl<T: Generator<Yield = ()>> Future for GenFuture<T> {
    type Output = T::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safe because we're !Unpin + !Drop mapping to a ?Unpin value
        let gen = unsafe { Pin::map_unchecked_mut(self, |s| &mut s.0) };
        let _guard = unsafe { set_task_context(cx) };
        match gen.resume(()) {
            GeneratorState::Yielded(()) => Poll::Pending,
            GeneratorState::Complete(x) => Poll::Ready(x),
        }
    }
}

#[cfg_attr(feature = "tls", thread_local)]
static mut TLS_CX: Cell<NonNull<Context<'static>>> = Cell::new(NonNull::dangling());

struct SetOnDrop(NonNull<Context<'static>>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        #[allow(unused_unsafe)]
        unsafe {
            TLS_CX.set(self.0);
        }
    }
}

// Safety: the returned guard must drop before `cx` is dropped and before
// any previous guard is dropped.
unsafe fn set_task_context(cx: &mut Context<'_>) -> SetOnDrop {
    // transmute the context's lifetime to 'static so we can store it.
    let cx = core::mem::transmute::<&mut Context<'_>, &mut Context<'static>>(cx);
    let old_cx = TLS_CX.replace(NonNull::from(cx));
    SetOnDrop(old_cx)
}

#[doc(hidden)]
/// Polls a future in the current thread-local task waker.
pub fn poll_with_tls_context<F>(f: Pin<&mut F>) -> Poll<F::Output>
where
    F: Future
{
    // Clear the entry so that nested `get_task_waker` calls
    // will fail or set their own value.
    #[allow(unused_unsafe)]
    let mut cx_ptr = unsafe { TLS_CX.get() };
    let _reset = SetOnDrop(cx_ptr);

    // Safety: we've ensured exclusive access to the context by
    // removing the pointer from TLS, only to be replaced once
    // we're done with it.
    //
    // The pointer that was inserted came from an `&mut Context<'_>`,
    // so it is safe to treat as mutable.
    unsafe { F::poll(f, cx_ptr.as_mut()) }
}
