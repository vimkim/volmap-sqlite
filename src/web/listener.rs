//! Bounds transport residency, including clients that never finish HTTP headers or reads.
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::serve::Listener;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Sleep;

/// Allows at most 64 accepted connections, each with a 30-second transport lifetime.
/// Browser polling reconnects normally; this is not an inspection-work deadline.
pub fn bounded_listener(listener: TcpListener) -> impl Listener<Addr = SocketAddr> {
    BoundedListener {
        listener,
        slots: Arc::new(Semaphore::new(64)),
    }
}

struct BoundedListener {
    listener: TcpListener,
    slots: Arc<Semaphore>,
}

impl Listener for BoundedListener {
    type Io = Connection;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Connection, SocketAddr) {
        // Only this listener owns the semaphore; it is never closed.
        let slot = Arc::clone(&self.slots)
            .acquire_owned()
            .await
            .expect("listener slots stay open");
        let (stream, address) = Listener::accept(&mut self.listener).await;
        (
            Connection {
                stream,
                _slot: slot,
                deadline: Box::pin(tokio::time::sleep(Duration::from_secs(30))),
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

struct Connection {
    stream: TcpStream,
    _slot: OwnedSemaphorePermit,
    deadline: Pin<Box<Sleep>>,
}

impl Connection {
    fn check_deadline(&mut self, context: &mut Context<'_>) -> io::Result<()> {
        if self.deadline.as_mut().poll(context).is_ready() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "connection_lifetime_budget",
            ))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check_deadline(cx)?;
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check_deadline(cx)?;
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check_deadline(cx)?;
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
