use super::{Error, Instant, Result, RingBuffer, Socket, SocketHandle, SocketMeta};
use embedded_nal::SocketAddr;
use fugit::{ExtU32, SecsDurationU32};

/// A TCP socket ring buffer.
pub type SocketBuffer<const N: usize> = RingBuffer<u8, N>;

#[derive(Debug, PartialEq, Eq)]
pub enum State<const TIMER_HZ: u32> {
    /// Freshly created, unsullied
    Created,
    WaitingForConnect(SocketAddr),
    /// TCP connected or UDP has an address
    Connected(SocketAddr),
    /// Block all writes (Socket is closed by remote)
    ShutdownForWrite(Instant<TIMER_HZ>),
}

impl<const TIMER_HZ: u32> defmt::Format for State<TIMER_HZ> {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            State::Created => defmt::write!(fmt, "State::Created"),
            State::WaitingForConnect(_) => defmt::write!(fmt, "State::WaitingForConnect"),
            State::Connected(_) => defmt::write!(fmt, "State::Connected"),
            State::ShutdownForWrite(_) => defmt::write!(fmt, "State::ShutdownForWrite"),
        }
    }
}

impl<const TIMER_HZ: u32> Default for State<TIMER_HZ> {
    fn default() -> Self {
        State::Created
    }
}

/// A Transmission Control Protocol socket.
///
/// A TCP socket may passively listen for connections or actively connect to another endpoint.
/// Note that, for listening sockets, there is no "backlog"; to be able to simultaneously
/// accept several connections, as many sockets must be allocated, or any new connection
/// attempts will be reset.
pub struct TcpSocket<const TIMER_HZ: u32, const L: usize> {
    pub(crate) meta: SocketMeta,
    state: State<TIMER_HZ>,
    check_interval: SecsDurationU32,
    read_timeout: Option<SecsDurationU32>,
    available_data: usize,
    rx_buffer: SocketBuffer<L>,
    last_check_time: Option<Instant<TIMER_HZ>>,
}

impl<const TIMER_HZ: u32, const L: usize> TcpSocket<TIMER_HZ, L> {
    /// Create a socket using the given buffers.
    pub fn new(socket_id: u8) -> TcpSocket<TIMER_HZ, L> {
        TcpSocket {
            meta: SocketMeta {
                handle: SocketHandle(socket_id),
            },
            state: State::default(),
            rx_buffer: SocketBuffer::new(),
            available_data: 0,
            check_interval: 15.secs(),
            read_timeout: Some(15.secs()),
            last_check_time: None,
        }
    }

    /// Return the socket handle.
    pub fn handle(&self) -> SocketHandle {
        self.meta.handle
    }

    pub fn update_handle(&mut self, handle: SocketHandle) {
        defmt::debug!("[{:?}] Updating handle {:?}", self.handle(), handle);
        self.meta.update(handle)
    }

    /// Return the bound endpoint.
    pub fn endpoint(&self) -> Option<SocketAddr> {
        match self.state {
            State::Connected(s) | State::WaitingForConnect(s) => Some(s),
            _ => None,
        }
    }

    /// Return the connection state, in terms of the TCP state machine.
    pub fn state(&self) -> &State<TIMER_HZ> {
        &self.state
    }

    pub fn reset(&mut self) {
        self.set_state(State::default());
        self.rx_buffer.clear();
        self.set_available_data(0);
        self.last_check_time = None;
    }

    pub fn should_update_available_data(&mut self, ts: Instant<TIMER_HZ>) -> bool {
        // Cannot request available data on a socket that is closed by the
        // module
        if !self.is_connected() {
            return false;
        }

        let should_update = self
            .last_check_time
            .and_then(|last_check_time| ts.checked_duration_since(last_check_time))
            .map(|dur| dur >= self.check_interval)
            .unwrap_or(true);

        if should_update {
            self.last_check_time.replace(ts);
        }

        should_update
    }

    pub fn recycle(&self, ts: Instant<TIMER_HZ>) -> bool {
        if let Some(read_timeout) = self.read_timeout {
            match self.state {
                State::Created | State::WaitingForConnect(_) | State::Connected(_) => false,
                State::ShutdownForWrite(closed_time) => ts
                    .checked_duration_since(closed_time)
                    .map(|dur| dur >= read_timeout)
                    .unwrap_or(false),
            }
        } else {
            false
        }
    }

    pub fn closed_by_remote(&mut self, ts: Instant<TIMER_HZ>) {
        self.set_state(State::ShutdownForWrite(ts));
        self.set_available_data(0);
    }

    /// Set available data.
    pub fn set_available_data(&mut self, available_data: usize) {
        self.available_data = available_data;
    }

    /// Get the number of bytes available to ingress.
    pub fn get_available_data(&self) -> usize {
        self.available_data
    }

    /// Return whether a connection is active.
    ///
    /// This function returns true if the socket is actively exchanging packets
    /// with a remote endpoint. Note that this does not mean that it is possible
    /// to send or receive data through the socket; for that, use
    /// [can_recv](#method.can_recv).
    pub fn is_connected(&self) -> bool {
        defmt::debug!("[{:?}] State: {:?}", self.handle(), self.state);
        matches!(self.state, State::Connected(_))
    }

    /// Return whether the receive half of the full-duplex connection is open.
    ///
    /// This function returns true if it's possible to receive data from the remote endpoint.
    /// It will return true while there is data in the receive buffer, and if there isn't,
    /// as long as the remote endpoint has not closed the connection.
    ///
    /// In terms of the TCP state machine, the socket must be in the `Connected`,
    /// `FIN-WAIT-1`, or `FIN-WAIT-2` state, or have data in the receive buffer instead.
    pub fn may_recv(&self) -> bool {
        match self.state {
            State::Connected(_) | State::ShutdownForWrite(_) => true,
            // If we have something in the receive buffer, we can receive that.
            _ if !self.rx_buffer.is_empty() => true,
            _ => false,
        }
    }

    /// Check whether the receive half of the full-duplex connection buffer is open
    /// (see [may_recv](#method.may_recv), and the receive buffer is not full.
    pub fn can_recv(&self) -> bool {
        if !self.may_recv() {
            return false;
        }

        !self.rx_buffer.is_full()
    }

    fn recv_impl<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut SocketBuffer<L>) -> (usize, R),
    {
        // We may have received some data inside the initial SYN, but until the connection
        // is fully open we must not dequeue any data, as it may be overwritten by e.g.
        // another (stale) SYN. (We do not support TCP Fast Open.)
        if !self.may_recv() {
            return Err(Error::Illegal);
        }

        let (_size, result) = f(&mut self.rx_buffer);
        Ok(result)
    }

    /// Call `f` with the largest contiguous slice of octets in the receive buffer,
    /// and dequeue the amount of elements returned by `f`.
    ///
    /// This function returns `Err(Error::Illegal) if the receive half of
    /// the connection is not open; see [may_recv](#method.may_recv).
    pub fn recv<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut [u8]) -> (usize, R),
    {
        self.recv_impl(|rx_buffer| rx_buffer.dequeue_many_with(f))
    }

    /// Call `f` with a slice of octets in the receive buffer, and dequeue the
    /// amount of elements returned by `f`.
    ///
    /// If the buffer read wraps around, the second argument of `f` will be
    /// `Some()` with the remainder of the buffer, such that the combined slice
    /// of the two arguments, makes up the full buffer.
    ///
    /// This function returns `Err(Error::Illegal) if the receive half of the
    /// connection is not open; see [may_recv](#method.may_recv).
    pub fn recv_wrapping<'b, F>(&'b mut self, f: F) -> Result<usize>
    where
        F: FnOnce(&'b [u8], Option<&'b [u8]>) -> usize,
    {
        self.recv_impl(|rx_buffer| {
            rx_buffer.dequeue_many_with_wrapping(|a, b| {
                let len = f(a, b);
                (len, len)
            })
        })
    }

    /// Dequeue a sequence of received octets, and fill a slice from it.
    ///
    /// This function returns the amount of bytes actually dequeued, which is limited
    /// by the amount of free space in the transmit buffer; down to zero.
    ///
    /// See also [recv](#method.recv).
    pub fn recv_slice(&mut self, data: &mut [u8]) -> Result<usize> {
        self.recv_impl(|rx_buffer| {
            let size = rx_buffer.dequeue_slice(data);
            (size, size)
        })
    }

    /// Peek at a sequence of received octets without removing them from
    /// the receive buffer, and return a pointer to it.
    ///
    /// This function otherwise behaves identically to [recv](#method.recv).
    pub fn peek(&mut self, size: usize) -> Result<&[u8]> {
        // See recv() above.
        if !self.may_recv() {
            return Err(Error::Illegal);
        }

        Ok(self.rx_buffer.get_allocated(0, size))
    }

    pub fn rx_window(&self) -> usize {
        self.rx_buffer.window()
    }

    /// Peek at a sequence of received octets without removing them from
    /// the receive buffer, and fill a slice from it.
    ///
    /// This function otherwise behaves identically to [recv_slice](#method.recv_slice).
    pub fn peek_slice(&mut self, data: &mut [u8]) -> Result<usize> {
        let buffer = self.peek(data.len())?;
        let data = &mut data[..buffer.len()];
        data.copy_from_slice(buffer);
        Ok(buffer.len())
    }

    pub fn rx_enqueue_slice(&mut self, data: &[u8]) -> usize {
        self.rx_buffer.enqueue_slice(data)
    }

    /// Return the amount of octets queued in the receive buffer.
    ///
    /// Note that the Berkeley sockets interface does not have an equivalent of this API.
    pub fn recv_queue(&self) -> usize {
        self.rx_buffer.len()
    }

    pub fn set_state(&mut self, state: State<TIMER_HZ>) {
        defmt::debug!("[{:?}] TCP state change: {:?} -> {:?}", self.handle(), self.state, state);
        self.state = state
    }
}

impl<const TIMER_HZ: u32, const L: usize> Into<Socket<TIMER_HZ, L>> for TcpSocket<TIMER_HZ, L> {
    fn into(self) -> Socket<TIMER_HZ, L> {
        Socket::Tcp(self)
    }
}
