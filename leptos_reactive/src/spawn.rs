use cfg_if::cfg_if;
use std::future::Future;

/// Spawns and runs a thread-local [`Future`] in a platform-independent way.
///
/// This can be used to interface with any `async` code by spawning a task
/// to run a `Future`.
///
/// ## Limitations
///
/// You should not use `spawn_local` to synchronize `async` code with a
/// signal’s value during server rendering. The server response will not
/// be notified to wait for the spawned task to complete, creating a race
/// condition between the response and your task. Instead, use
/// [`create_resource`](crate::create_resource) and `<Suspense/>` to coordinate
/// asynchronous work with the rendering process.
///
/// ```
/// # use leptos::*;
/// # #[cfg(not(any(feature = "csr", feature = "serde-lite", feature = "miniserde", feature = "rkyv")))]
/// # {
///
/// async fn get_user(user: String) -> Result<String, ServerFnError> {
///     Ok(format!("this user is {user}"))
/// }
///
/// // ❌ Write into a signal from `spawn_local` on the serevr
/// #[component]
/// fn UserBad() -> impl IntoView {
///     let signal = create_rw_signal(String::new());
///
///     // ❌ If the rest of the response is already complete,
///     //    `signal` will no longer exist when `get_user` resolves
///     #[cfg(feature = "ssr")]
///     spawn_local(async move {
///         let user_res = get_user("user".into()).await.unwrap_or_default();
///         signal.set(user_res);
///     });
///
///     view! {
///         <p>
///             "This will be empty (hopefully the client will render it) -> "
///             {move || signal.get()}
///         </p>
///     }
/// }
///
/// // ✅ Use a resource and suspense
/// #[component]
/// fn UserGood() -> impl IntoView {
///     // new resource with no dependencies (it will only called once)
///     let user = create_resource(|| (), |_| async { get_user("john".into()).await });
///     view! {
///         // handles the loading
///         <Suspense fallback=move || view! {<p>"Loading User"</p> }>
///             // handles the error from the resource
///             <ErrorBoundary fallback=|_| {view! {<p>"Something went wrong"</p>}}>
///                 {move || {
///                     user.read().map(move |x| {
///                         // the resource has a result
///                         x.map(move |y| {
///                             // successful call from the server fn
///                             view! {<p>"User result filled in server and client: "{y}</p>}
///                         })
///                     })
///                 }}
///             </ErrorBoundary>
///         </Suspense>
///     }
/// }
/// # }
/// ```
pub fn spawn_local<F>(fut: F)
where
    F: Future<Output = ()> + 'static,
{
    cfg_if! {
        if #[cfg(all(target_arch = "wasm32", target_os = "wasi", feature = "ssr"))] {
            wasi_spawn::run(fut)
            // let waker = std::sync::Arc::new(DummyWaker).into();
            // let mut cx = std::task::Context::from_waker(&waker);

            // futures::pin_mut!(fut);
            // let std::task::Poll::Ready(_) = fut.as_mut().poll(&mut cx) else {
            //     // TODO: Provide a proper executor for WASI, based on `poll_list`
            //     panic!("Pending futures are not yet available on WASI.")
            // };
        }
        else if #[cfg(target_arch = "wasm32")] {
            wasm_bindgen_futures::spawn_local(fut)
        }
        else if #[cfg(any(test, doctest))] {
            tokio_test::block_on(fut);
        } else if #[cfg(feature = "ssr")] {
            use crate::Runtime;

            let runtime = Runtime::current();
            tokio::task::spawn_local(async move {
                crate::TASK_RUNTIME.scope(Some(runtime), fut).await
            });
        }  else {
            futures::executor::block_on(fut)
        }
    }
}

#[cfg(feature = "ssr")]
mod wasi_spawn {

pub mod wit {
    #![allow(missing_docs)]

    wit_bindgen::generate!({
        world: "platform",
        path: "../../spin/wit",
    });
    pub use fermyon::spin2_0_0 as v2;
}

use std::future::Future;
use std::mem;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

static WAKERS: Mutex<Vec<(wit::wasi::io::poll::Pollable, Waker)>> = Mutex::new(Vec::new());

/// Run the specified future to completion blocking until it yields a result.
///
/// Based on an executor using `wasi::io/poll/poll-list`,
pub fn run<T>(future: impl Future<Output = T>) -> T {
    futures::pin_mut!(future);
    struct DummyWaker;

    impl Wake for DummyWaker {
        fn wake(self: Arc<Self>) {}
    }

    let waker = Arc::new(DummyWaker).into();

    loop {
        match future.as_mut().poll(&mut Context::from_waker(&waker)) {
            Poll::Pending => {
                let mut new_wakers = Vec::new();

                let wakers = mem::take::<Vec<_>>(&mut WAKERS.lock().unwrap());

                assert!(!wakers.is_empty());

                let pollables = wakers
                    .iter()
                    .map(|(pollable, _)| pollable)
                    .collect::<Vec<_>>();

                let mut ready = vec![false; wakers.len()];

                for index in wit::wasi::io::poll::poll_list(&pollables) {
                    ready[usize::try_from(index).unwrap()] = true;
                }

                for (ready, (pollable, waker)) in ready.into_iter().zip(wakers) {
                    if ready {
                        waker.wake()
                    } else {
                        new_wakers.push((pollable, waker));
                    }
                }

                *WAKERS.lock().unwrap() = new_wakers;
            }
            Poll::Ready(result) => break result,
        }
    }
}

}
