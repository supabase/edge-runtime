// This implementation originated from the link below:
// https://gist.github.com/programatik29/36d371c657392fd7f322e7342957b6d1

use std::{
    pin::Pin,
    task::{ready, Poll},
    time::Duration,
};

use futures_util::Future;
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::{sleep, Instant, Sleep},
};

pub(super) enum State {
    Wait,
    Reset,
}

pub struct Stream<S> {
    inner: S,
    sleep: Pin<Box<Sleep>>,
    duration: Duration,
    waiting: bool,
    finished: bool,
    state: UnboundedReceiver<State>,
}

impl<S> Stream<S> {
    pub(super) fn new(inner: S, duration: Duration, rx: UnboundedReceiver<State>) -> Self {
        Self {
            inner,
            sleep: Box::pin(sleep(duration)),
            duration,
            waiting: false,
            finished: false,
            state: rx,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Stream<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.finished {
            match Pin::new(&mut self.state).poll_recv(cx) {
                Poll::Ready(Some(State::Reset)) => {
                    self.waiting = false;

                    let deadline = Instant::now() + self.duration;

                    self.sleep.as_mut().reset(deadline);
                }

                // enter waiting mode (for response body last chunk)
                Poll::Ready(Some(State::Wait)) => self.waiting = true,
                Poll::Ready(None) => self.finished = true,
                Poll::Pending => (),
            }
        }

        if !self.waiting {
            // return error if timer is elapsed
            if let Poll::Ready(()) = self.sleep.as_mut().poll(cx) {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request header read timed out",
                )));
            }
        }

        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Stream<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

pub struct Service<S> {
    inner: S,
    tx: UnboundedSender<State>,
}

impl<S> Service<S> {
    pub(super) fn new(inner: S, tx: UnboundedSender<State>) -> Self {
        Self { inner, tx }
    }
}

impl<S, B, Request> hyper::service::Service<Request> for Service<S>
where
    S: hyper::service::Service<Request, Response = hyper::Response<B>>,
{
    type Response = hyper::Response<Body<B>>;
    type Error = S::Error;
    type Future = ServiceFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // send timer wait signal
        let _ = self.tx.send(State::Wait);

        ServiceFuture::new(self.inner.call(req), self.tx.clone())
    }
}

#[pin_project]
pub struct ServiceFuture<F> {
    #[pin]
    inner: F,
    tx: Option<UnboundedSender<State>>,
}

impl<F> ServiceFuture<F> {
    fn new(inner: F, tx: UnboundedSender<State>) -> Self {
        Self {
            inner,
            tx: Some(tx),
        }
    }
}

impl<F, B, Error> Future for ServiceFuture<F>
where
    F: Future<Output = Result<hyper::Response<B>, Error>>,
{
    type Output = Result<hyper::Response<Body<B>>, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        this.inner.poll(cx).map(|result| {
            result.map(|response| {
                response
                    .map(|body| Body::new(body, this.tx.take().expect("future polled after ready")))
            })
        })
    }
}

#[pin_project]
pub struct Body<B> {
    #[pin]
    inner: B,
    tx: UnboundedSender<State>,
}

impl<B> Body<B> {
    fn new(inner: B, tx: UnboundedSender<State>) -> Self {
        Self { inner, tx }
    }
}

impl<B> hyper::body::HttpBody for Body<B>
where
    B: hyper::body::HttpBody,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        let this = self.project();
        let option = ready!(this.inner.poll_data(cx));

        if option.is_none() {
            let _ = this.tx.send(State::Reset);
        }

        Poll::Ready(option)
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        self.project().inner.poll_trailers(cx)
    }

    fn is_end_stream(&self) -> bool {
        let is_end_stream = self.inner.is_end_stream();

        if is_end_stream {
            let _ = self.tx.send(State::Reset);
        }

        is_end_stream
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}
