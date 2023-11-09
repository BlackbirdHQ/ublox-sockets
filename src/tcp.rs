use super::{Error, PeerHandle, Result, RingBuffer};
use embassy_time::{Duration, Instant};
use no_std_net::SocketAddr;

/// A TCP socket ring buffer.
pub type SocketBuffer<'a> = RingBuffer<'a, u8>;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum State {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

#[cfg(feature = "defmt")]
impl defmt::Format for State {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            State::Closed => defmt::write!(fmt, "CLOSED"),
            State::Listen => defmt::write!(fmt, "LISTEN"),
            State::SynSent => defmt::write!(fmt, "SYN-SENT"),
            State::SynReceived => defmt::write!(fmt, "SYN-RECEIVED"),
            State::Established => defmt::write!(fmt, "ESTABLISHED"),
            State::FinWait1 => defmt::write!(fmt, "FIN-WAIT-1"),
            State::CloseWait => defmt::write!(fmt, "CLOSE-WAIT"),
            State::Closing => defmt::write!(fmt, "CLOSING"),
            State::LastAck => defmt::write!(fmt, "LAST-ACK"),
            State::TimeWait => defmt::write!(fmt, "TIME-WAIT"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum Timer {
    Retry { expires_at: Instant },
    Close { expires_at: Instant },
}

const RETRY_DELAY: Duration = Duration::from_secs(5);
const CLOSE_DELAY: Duration = Duration::from_secs(10);

impl Timer {
    fn new_retry() -> Self {
        Timer::Retry {
            expires_at: Instant::now() + RETRY_DELAY,
        }
    }

    fn new_close() -> Self {
        Timer::Close {
            expires_at: Instant::now() + CLOSE_DELAY,
        }
    }

    fn should_retry(&self) -> bool {
        match *self {
            Timer::Retry { expires_at } if Instant::now() >= expires_at => true,
            _ => false,
        }
    }

    fn should_close(&self) -> bool {
        match *self {
            Timer::Close { expires_at } if Instant::now() >= expires_at => true,
            _ => false,
        }
    }

    fn poll_at(&self) -> Instant {
        match *self {
            Timer::Retry { expires_at, .. } => expires_at,
            Timer::Close { expires_at } => expires_at,
        }
    }
}

/// A Transmission Control Protocol socket.
///
/// A TCP socket may passively listen for connections or actively connect to another endpoint.
/// Note that, for listening sockets, there is no "backlog"; to be able to simultaneously
/// accept several connections, as many sockets must be allocated, or any new connection
/// attempts will be reset.
#[derive(Debug)]
pub struct Socket<'a> {
    pub peer_handle: Option<PeerHandle>,
    #[cfg(feature = "edm")]
    pub edm_channel: Option<super::ChannelId>,
    state: State,
    pub remote_endpoint: Option<SocketAddr>,
    pub local_port: Option<u16>,
    rx_buffer: SocketBuffer<'a>,

    tx_buffer: SocketBuffer<'a>,
    /// Interval after which, if no inbound packets are received, the connection is aborted.
    timeout: Option<Duration>,
    /// Interval at which keep-alive packets will be sent.
    keep_alive: Option<Duration>,
    /// The timestamp of the last packet received.
    remote_last_ts: Option<Instant>,
    timer: Option<Timer>,

    #[cfg(feature = "async")]
    rx_waker: crate::waker::WakerRegistration,
    #[cfg(feature = "async")]
    tx_waker: crate::waker::WakerRegistration,
}

#[cfg(feature = "defmt")]
impl<'a> defmt::Format for Socket<'a> {
    fn format(&self, fmt: defmt::Formatter) {
        #[cfg(feature = "edm")]
        let edm_chan = self.edm_channel;
        #[cfg(not(feature = "edm"))]
        let edm_chan = "N/A";

        defmt::write!(
            fmt,
            "{{ peer_handle: {}, edm_channel: {}, state: {}, remote_endpoint: {}, local_port: {}}}",
            self.peer_handle,
            edm_chan,
            self.state,
            defmt::Debug2Format(&self.remote_endpoint),
            self.local_port
        )
    }
}

impl<'a> Socket<'a> {
    /// Create a socket using the given buffers.
    pub fn new(
        rx_buffer: impl Into<SocketBuffer<'a>>,
        tx_buffer: impl Into<SocketBuffer<'a>>,
    ) -> Socket<'a> {
        Socket {
            peer_handle: None,
            #[cfg(feature = "edm")]
            edm_channel: None,
            state: State::Closed,
            remote_endpoint: None,
            rx_buffer: rx_buffer.into(),
            tx_buffer: tx_buffer.into(),
            timeout: None,
            keep_alive: None,
            remote_last_ts: None,
            timer: None,
            local_port: None,

            #[cfg(feature = "async")]
            rx_waker: crate::waker::WakerRegistration::new(),
            #[cfg(feature = "async")]
            tx_waker: crate::waker::WakerRegistration::new(),
        }
    }

    /// Register a waker for receive operations.
    ///
    /// The waker is woken on state changes that might affect the return value
    /// of `recv` method calls, such as receiving data, or the socket closing.
    ///
    /// Notes:
    ///
    /// - Only one waker can be registered at a time. If another waker was previously registered,
    ///   it is overwritten and will no longer be woken.
    /// - The Waker is woken only once. Once woken, you must register it again to receive more wakes.
    /// - "Spurious wakes" are allowed: a wake doesn't guarantee the result of `recv` has
    ///   necessarily changed.
    #[cfg(feature = "async")]
    pub fn register_recv_waker(&mut self, waker: &core::task::Waker) {
        self.rx_waker.register(waker)
    }

    /// Register a waker for send operations.
    ///
    /// The waker is woken on state changes that might affect the return value
    /// of `send` method calls, such as space becoming available in the transmit
    /// buffer, or the socket closing.
    ///
    /// Notes:
    ///
    /// - Only one waker can be registered at a time. If another waker was previously registered,
    ///   it is overwritten and will no longer be woken.
    /// - The Waker is woken only once. Once woken, you must register it again to receive more wakes.
    /// - "Spurious wakes" are allowed: a wake doesn't guarantee the result of `send` has
    ///   necessarily changed.
    #[cfg(feature = "async")]
    pub fn register_send_waker(&mut self, waker: &core::task::Waker) {
        self.tx_waker.register(waker)
    }

    //// Return the remote endpoint, or None if not connected.
    #[inline]
    pub fn remote_endpoint(&self) -> Option<SocketAddr> {
        self.remote_endpoint
    }

    /// Return the connection state, in terms of the TCP state machine.
    #[inline]
    pub fn state(&self) -> State {
        self.state
    }

    pub fn reset(&mut self) {
        self.set_state(State::Closed);
        self.peer_handle = None;

        #[cfg(feature = "edm")]
        {
            self.edm_channel = None;
        }

        self.rx_buffer.clear();
        self.tx_buffer.clear();
        self.remote_endpoint = None;
        self.remote_last_ts = None;
        self.timer = None;

        #[cfg(feature = "async")]
        {
            self.rx_waker.wake();
            self.tx_waker.wake();
        }
    }

    /// Connect to a given endpoint.
    ///
    /// The local port may optionally be provided.
    ///
    /// This function returns an error if the socket was open; see [is_open](#method.is_open).
    /// It also returns an error if the local or remote port is zero, or if the remote address
    /// is unspecified.
    pub fn connect<T>(
        &mut self,
        // cx: &mut Context,
        remote_endpoint: T,
        local_port: Option<u16>,
    ) -> Result<()>
    where
        T: Into<SocketAddr>,
    {
        let remote_endpoint: SocketAddr = remote_endpoint.into();

        if self.is_open() {
            return Err(Error::InvalidState);
        }
        if remote_endpoint.port() == 0 || remote_endpoint.ip().is_unspecified() {
            return Err(Error::Unaddressable);
        }
        if local_port == Some(0) {
            return Err(Error::Unaddressable);
        }

        self.reset();
        self.remote_endpoint = Some(remote_endpoint);
        self.local_port = local_port;

        Ok(())
    }

    /// Close the transmit half of the full-duplex connection.
    ///
    /// Note that there is no corresponding function for the receive half of the full-duplex
    /// connection; only the remote end can close it. If you no longer wish to receive any
    /// data and would like to reuse the socket right away, use [abort](#method.abort).
    pub fn close(&mut self) {
        match self.state {
            // In the LISTEN state there is no established connection.
            State::Listen => self.set_state(State::Closed),
            // In the SYN-SENT state the remote endpoint is not yet synchronized and, upon
            // receiving an RST, will abort the connection.
            State::SynSent => self.set_state(State::Closed),
            // In the SYN-RECEIVED, ESTABLISHED and CLOSE-WAIT states the transmit half
            // of the connection is open, and needs to be explicitly closed with a FIN.
            State::SynReceived | State::Established => self.set_state(State::FinWait1),
            State::CloseWait => self.set_state(State::LastAck),
            // In the FIN-WAIT-1, FIN-WAIT-2, CLOSING, LAST-ACK, TIME-WAIT and CLOSED states,
            // the transmit half of the connection is already closed, and no further
            // action is needed.
            State::FinWait1 | State::Closing | State::TimeWait | State::LastAck | State::Closed => {
                ()
            }
        }
    }

    /// Aborts the connection, if any.
    ///
    /// This function instantly closes the socket. One reset packet will be sent to the remote
    /// endpoint.
    ///
    /// In terms of the TCP state machine, the socket may be in any state and is moved to
    /// the `CLOSED` state.
    pub fn abort(&mut self) {
        self.set_state(State::FinWait1);
    }

    /// Return whether the socket is passively listening for incoming connections.
    ///
    /// In terms of the TCP state machine, the socket must be in the `LISTEN` state.
    #[inline]
    pub fn is_listening(&self) -> bool {
        match self.state {
            State::Listen => true,
            _ => false,
        }
    }

    /// Return whether the socket is open.
    ///
    /// This function returns true if the socket will process incoming or dispatch outgoing
    /// packets. Note that this does not mean that it is possible to send or receive data through
    /// the socket; for that, use [can_send](#method.can_send) or [can_recv](#method.can_recv).
    ///
    /// In terms of the TCP state machine, the socket must not be in the `CLOSED`
    /// or `TIME-WAIT` states.
    #[inline]
    pub fn is_open(&self) -> bool {
        match self.state {
            State::Closed => false,
            State::TimeWait => false,
            _ => true,
        }
    }

    /// Return whether a connection is active.
    ///
    /// This function returns true if the socket is actively exchanging packets with
    /// a remote endpoint. Note that this does not mean that it is possible to send or receive
    /// data through the socket; for that, use [can_send](#method.can_send) or
    /// [can_recv](#method.can_recv).
    ///
    /// If a connection is established, [abort](#method.close) will send a reset to
    /// the remote endpoint.
    ///
    /// In terms of the TCP state machine, the socket must not be in the `CLOSED`, `TIME-WAIT`,
    /// or `LISTEN` state.
    #[inline]
    pub fn is_active(&self) -> bool {
        match self.state {
            State::Closed => false,
            State::TimeWait => false,
            State::Listen => false,
            _ => true,
        }
    }

    /// Return whether the transmit half of the full-duplex connection is open.
    ///
    /// This function returns true if it's possible to send data and have it arrive
    /// to the remote endpoint. However, it does not make any guarantees about the state
    /// of the transmit buffer, and even if it returns true, [send](#method.send) may
    /// not be able to enqueue any octets.
    ///
    /// In terms of the TCP state machine, the socket must be in the `ESTABLISHED` or
    /// `CLOSE-WAIT` state.
    #[inline]
    pub fn may_send(&self) -> bool {
        match self.state {
            State::Established => true,
            // In CLOSE-WAIT, the remote endpoint has closed our receive half of the connection
            // but we still can transmit indefinitely.
            State::CloseWait => true,
            _ => false,
        }
    }

    /// Return whether the receive half of the full-duplex connection is open.
    ///
    /// This function returns true if it's possible to receive data from the remote endpoint.
    /// It will return true while there is data in the receive buffer, and if there isn't,
    /// as long as the remote endpoint has not closed the connection.
    ///
    /// In terms of the TCP state machine, the socket must be in the `ESTABLISHED`,
    /// `FIN-WAIT-1`, or `FIN-WAIT-2` state, or have data in the receive buffer instead.
    #[inline]
    pub fn may_recv(&self) -> bool {
        match self.state {
            State::Established => true,
            // In FIN-WAIT-1/2, we have closed our transmit half of the connection but
            // we still can receive indefinitely.
            State::FinWait1 => true,
            // If we have something in the receive buffer, we can receive that.
            _ if !self.rx_buffer.is_empty() => true,
            _ => false,
        }
    }

    /// Check whether the transmit half of the full-duplex connection is open
    /// (see [may_send](#method.may_send)), and the transmit buffer is not full.
    #[inline]
    pub fn can_send(&self) -> bool {
        if !self.may_send() {
            return false;
        }

        !self.tx_buffer.is_full()
    }

    /// Return the maximum number of bytes inside the recv buffer.
    #[inline]
    pub fn recv_capacity(&self) -> usize {
        self.rx_buffer.capacity()
    }

    /// Return the maximum number of bytes inside the transmit buffer.
    #[inline]
    pub fn send_capacity(&self) -> usize {
        self.tx_buffer.capacity()
    }

    /// Check whether the receive half of the full-duplex connection buffer is open
    /// (see [may_recv](#method.may_recv)), and the receive buffer is not empty.
    #[inline]
    pub fn can_recv(&self) -> bool {
        if !self.may_recv() {
            return false;
        }

        !self.rx_buffer.is_empty()
    }

    fn send_impl<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut SocketBuffer<'a>) -> (usize, R),
    {
        if !self.may_send() {
            return Err(Error::Illegal);
        }

        // The connection might have been idle for a long time, and so remote_last_ts
        // would be far in the past. Unless we clear it here, we'll abort the connection
        // down over in dispatch() by erroneously detecting it as timed out.
        if self.tx_buffer.is_empty() {
            self.remote_last_ts = None
        }

        let _old_length = self.tx_buffer.len();
        let (_size, result) = f(&mut self.tx_buffer);
        // if size > 0 {
        // #[cfg(any(test, feature = "verbose"))]
        // trace!(
        //     "tx buffer: enqueueing {} octets (now {})",
        //     size,
        //     _old_length + size
        // );
        // }
        Ok(result)
    }

    /// Call `f` with the largest contiguous slice of octets in the transmit buffer,
    /// and enqueue the amount of elements returned by `f`.
    ///
    /// This function returns `Err(Error::Illegal)` if the transmit half of
    /// the connection is not open; see [may_send](#method.may_send).
    pub fn send<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut [u8]) -> (usize, R),
    {
        self.send_impl(|tx_buffer| tx_buffer.enqueue_many_with(f))
    }

    /// Enqueue a sequence of octets to be sent, and fill it from a slice.
    ///
    /// This function returns the amount of octets actually enqueued, which is limited
    /// by the amount of free space in the transmit buffer; down to zero.
    ///
    /// See also [send](#method.send).
    pub fn send_slice(&mut self, data: &[u8]) -> Result<usize> {
        self.send_impl(|tx_buffer| {
            let size = tx_buffer.enqueue_slice(data);
            (size, size)
        })
    }

    fn recv_error_check(&mut self) -> Result<()> {
        // We may have received some data inside the initial SYN, but until the connection
        // is fully open we must not dequeue any data, as it may be overwritten by e.g.
        // another (stale) SYN. (We do not support TCP Fast Open.)
        if !self.may_recv() {
            // if self.rx_fin_received {
            //     return Err(Error::Finished);
            // }
            return Err(Error::InvalidState);
        }

        Ok(())
    }

    fn recv_impl<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut SocketBuffer<'a>) -> (usize, R),
    {
        self.recv_error_check()?;

        let _old_length = self.rx_buffer.len();
        let (_size, result) = f(&mut self.rx_buffer);
        // if size > 0 {
        //     // #[cfg(any(test, feature = "verbose"))]
        //     trace!(
        //         "rx buffer: dequeueing {} octets (now {})",
        //         size,
        //         _old_length - size
        //     );
        // }

        Ok(result)
    }

    /// Call `f` with the largest contiguous slice of octets in the receive buffer,
    /// and dequeue the amount of elements returned by `f`.
    ///
    /// This function errors if the receive half of the connection is not open.
    ///
    /// If the receive half has been gracefully closed (with a FIN packet), `Err(Error::Finished)`
    /// is returned. In this case, the previously received data is guaranteed to be complete.
    ///
    /// In all other cases, `Err(Error::Illegal)` is returned and previously received data (if any)
    /// may be incomplete (truncated).
    pub fn recv<'b, F, R>(&'b mut self, f: F) -> Result<R>
    where
        F: FnOnce(&'b mut [u8]) -> (usize, R),
    {
        self.recv_impl(|rx_buffer| rx_buffer.dequeue_many_with(f))
    }

    /// Dequeue a sequence of received octets, and fill a slice from it.
    ///
    /// This function returns the amount of octets actually dequeued, which is limited
    /// by the amount of occupied space in the receive buffer; down to zero.
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
        self.recv_error_check()?;

        let buffer = self.rx_buffer.get_allocated(0, size);
        if !buffer.is_empty() {
            // #[cfg(any(test, feature = "verbose"))]
            trace!("rx buffer: peeking at {} octets", buffer.len());
        }
        Ok(buffer)
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

    /// Return the amount of octets queued in the transmit buffer.
    ///
    /// Note that the Berkeley sockets interface does not have an equivalent of this API.
    pub fn send_queue(&self) -> usize {
        self.tx_buffer.len()
    }

    /// Return the amount of octets queued in the receive buffer. This value can be larger than
    /// the slice read by the next `recv` or `peek` call because it includes all queued octets,
    /// and not only the octets that may be returned as a contiguous slice.
    ///
    /// Note that the Berkeley sockets interface does not have an equivalent of this API.
    pub fn recv_queue(&self) -> usize {
        self.rx_buffer.len()
    }

    pub fn rx_enqueue_slice(&mut self, data: &[u8]) -> usize {
        let n = self.rx_buffer.enqueue_slice(data);
        self.remote_last_ts = Some(Instant::now());
        if n > 0 {
            trace!("[{}] Enqueued {:?} bytes to RX buffer", self.peer_handle, n);
            #[cfg(feature = "async")]
            self.rx_waker.wake();
        }
        n
    }

    pub fn tx_dequeue<'b, F, R>(&'b mut self, f: F) -> R
    where
        F: FnOnce(&'b mut [u8]) -> (usize, R),
    {
        let (n, res) = self.tx_buffer.dequeue_many_with(f);
        if n > 0 {
            trace!(
                "[{}] Dequeued {:?} bytes from TX buffer",
                self.peer_handle,
                n
            );

            #[cfg(feature = "async")]
            self.tx_waker.wake();
        }

        res
    }

    #[cfg(feature = "async")]
    pub async fn async_tx_dequeue<'b, F, FUT, R>(&'b mut self, f: F) -> R
    where
        F: FnOnce(&'b mut [u8]) -> FUT,
        FUT: core::future::Future<Output = (usize, R)>,
    {
        let (n, res) = self.tx_buffer.async_dequeue_many_with(f).await;
        if n > 0 {
            defmt::trace!(
                "[{}] Dequeued {:?} bytes from TX buffer",
                self.peer_handle,
                n
            );

            #[cfg(feature = "async")]
            self.tx_waker.wake();
        }

        res
    }

    pub fn set_state(&mut self, state: State) {
        #[cfg(feature = "defmt")]
        debug!(
            "[TCP Socket {}] [{:?}] state change: {:?} -> {:?}",
            defmt::Debug2Format(&self.remote_endpoint),
            self.peer_handle,
            self.state,
            state
        );
        #[cfg(not(feature = "defmt"))]
        debug!(
            "[TCP Socket {}] [{:?}] state change: {:?} -> {:?}",
            &self.remote_endpoint, self.peer_handle, self.state, state
        );
        match state {
            State::TimeWait => {
                self.timer = Some(Timer::new_close());
            }
            State::SynSent => {
                self.timer = Some(Timer::new_retry());
            }
            _ => {}
        }

        self.state = state;

        #[cfg(feature = "async")]
        {
            // Wake all tasks waiting. Even if we haven't received/sent data, this
            // is needed because return values of functions may change depending on the state.
            // For example, a pending read has to fail with an error if the socket is closed.
            self.rx_waker.wake();
            self.tx_waker.wake();
        }
    }

    pub fn poll_at(&self) -> Option<Instant> {
        self.timer.map(|t| t.poll_at())
    }

    pub fn poll(&mut self) {
        if self.remote_endpoint.is_none() {
            return;
        }

        match (self.timer, self.state) {
            (Some(timer), State::SynSent) if timer.should_retry() => self.set_state(State::Closed),
            (Some(timer), State::TimeWait) if timer.should_close() => {
                self.reset();
            }
            _ => {}
        }

        // // Check if any state needs to be changed because of a timer.
        // if self.timed_out(Instant::now()) {
        //     // If a timeout expires, we should abort the connection.
        //     defmt::debug!("timeout exceeded");
        //     self.set_state(State::Closed);
        // }
    }
}

impl<'a> core::fmt::Write for Socket<'a> {
    fn write_str(&mut self, slice: &str) -> core::fmt::Result {
        let slice = slice.as_bytes();
        if self.send_slice(slice) == Ok(slice.len()) {
            Ok(())
        } else {
            Err(core::fmt::Error)
        }
    }
}
