#![cfg_attr(not(test), no_std)]

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

mod ring_buffer;
mod set;
pub mod tcp;
pub mod tcp_listener;
pub mod udp;
pub mod udp_listener;

#[cfg(feature = "async")]
mod waker;

pub use self::ring_buffer::RingBuffer;

#[cfg(feature = "socket-tcp")]
pub use tcp::{Socket as TcpSocket, State as TcpState};

#[cfg(feature = "socket-udp")]
pub use udp::{Socket as UdpSocket, State as UdpState};

pub use self::set::{Handle as SocketHandle, Set as SocketSet};

/// The error type for the networking stack.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// An operation cannot proceed because a buffer is empty or full.
    Exhausted,
    /// An operation is not permitted in the current state.
    Illegal,
    /// An endpoint or address of a remote host could not be translated to a lower level address.
    /// E.g. there was no an Ethernet address corresponding to an IPv4 address in the ARP cache,
    /// or a TCP connection attempt was made to an unspecified endpoint.
    Unaddressable,

    SocketClosed,
    BadLength,

    NotBound,

    ListenerError,

    SocketSetFull,
    InvalidSocket,
    DuplicateSocket,
    Timeout,
}

type Result<T> = core::result::Result<T, Error>;

/// A network socket.
///
/// This enumeration abstracts the various types of sockets based on the IP protocol.
/// To downcast a `Socket` value to a concrete socket, use the [AnySocket] trait,
/// e.g. to get `UdpSocket`, call `UdpSocket::downcast(socket)`.
///
/// It is usually more convenient to use [SocketSet::get] instead.
///
/// [AnySocket]: trait.AnySocket.html
/// [SocketSet::get]: struct.SocketSet.html#method.get
#[non_exhaustive]
#[derive(Debug)]
pub enum Socket<'a> {
    #[cfg(feature = "socket-udp")]
    Udp(UdpSocket<'a>),
    #[cfg(feature = "socket-tcp")]
    Tcp(TcpSocket<'a>),
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SocketType {
    Udp,
    Tcp,
}

impl<'a> Socket<'a> {
    /// Return the socket handle.
    #[inline]
    pub fn handle(&self) -> SocketHandle {
        match self {
            #[cfg(feature = "socket-udp")]
            Socket::Udp(ref socket) => socket.handle(),
            #[cfg(feature = "socket-tcp")]
            Socket::Tcp(ref socket) => socket.handle(),
        }
    }

    pub fn get_type(&self) -> SocketType {
        match self {
            Socket::Tcp(_) => SocketType::Tcp,
            Socket::Udp(_) => SocketType::Udp,
        }
    }

    pub fn should_update_available_data(&mut self) -> bool {
        match self {
            Socket::Tcp(s) => s.should_update_available_data(),
            Socket::Udp(s) => s.should_update_available_data(),
        }
    }

    pub fn available_data(&self) -> usize {
        match self {
            Socket::Tcp(s) => s.get_available_data(),
            Socket::Udp(s) => s.get_available_data(),
        }
    }

    pub fn recycle(&self) -> bool {
        match self {
            Socket::Tcp(s) => s.recycle(),
            Socket::Udp(s) => s.recycle(),
        }
    }

    pub fn closed_by_remote(&mut self) {
        match self {
            Socket::Tcp(s) => s.closed_by_remote(),
            Socket::Udp(s) => s.closed_by_remote(),
        }
    }

    pub fn set_available_data(&mut self, available_data: usize) {
        match self {
            Socket::Tcp(s) => s.set_available_data(available_data),
            Socket::Udp(s) => s.set_available_data(available_data),
        }
    }

    pub fn rx_enqueue_slice(&mut self, data: &[u8]) -> usize {
        match self {
            Socket::Tcp(s) => s.rx_enqueue_slice(data),
            Socket::Udp(s) => s.rx_enqueue_slice(data),
        }
    }

    pub fn rx_window(&self) -> usize {
        match self {
            Socket::Tcp(s) => s.rx_window(),
            Socket::Udp(s) => s.rx_window(),
        }
    }

    pub fn can_recv(&self) -> bool {
        match self {
            Socket::Tcp(s) => s.can_recv(),
            Socket::Udp(s) => s.can_recv(),
        }
    }
}

/// A conversion trait for network sockets.
pub trait AnySocket<'a> {
    fn upcast(self) -> Socket<'a>;
    fn downcast<'c>(socket: &'c Socket<'a>) -> Option<&'c Self>
    where
        Self: Sized;
    fn downcast_mut<'c>(socket: &'c mut Socket<'a>) -> Option<&'c mut Self>
    where
        Self: Sized;
}

macro_rules! from_socket {
    ($socket:ty, $variant:ident) => {
        impl<'a> AnySocket<'a> for $socket {
            fn upcast(self) -> Socket<'a> {
                Socket::$variant(self)
            }

            fn downcast<'c>(socket: &'c Socket<'a>) -> Option<&'c Self> {
                #[allow(unreachable_patterns)]
                match socket {
                    Socket::$variant(socket) => Some(socket),
                    _ => None,
                }
            }

            fn downcast_mut<'c>(socket: &'c mut Socket<'a>) -> Option<&'c mut Self> {
                #[allow(unreachable_patterns)]
                match socket {
                    Socket::$variant(socket) => Some(socket),
                    _ => None,
                }
            }
        }
    };
}

#[cfg(feature = "socket-udp")]
from_socket!(udp::Socket<'a>, Udp);
#[cfg(feature = "socket-tcp")]
from_socket!(tcp::Socket<'a>, Tcp);

#[cfg(test)]
#[cfg(feature = "defmt")]
mod test_helpers {
    use core::ptr::NonNull;

    #[defmt::global_logger]
    struct Logger;
    impl defmt::Write for Logger {
        fn write(&mut self, _bytes: &[u8]) {}
    }

    unsafe impl defmt::Logger for Logger {
        fn acquire() -> Option<NonNull<dyn defmt::Write>> {
            Some(NonNull::from(&Logger as &dyn defmt::Write))
        }

        unsafe fn release(_: NonNull<dyn defmt::Write>) {}
    }

    defmt::timestamp!("");

    #[export_name = "_defmt_panic"]
    fn panic() -> ! {
        panic!()
    }
}
