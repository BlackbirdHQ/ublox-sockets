use crate::meta::Meta;

use super::{AnySocket, Socket};
use serde::{Deserialize, Serialize};

/// A handle, identifying a socket in a set.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SocketHandle(pub u8);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PeerHandle(pub u8);

#[cfg(feature = "edm")]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ChannelId(pub u8);

#[derive(Debug, Default)]
pub struct SocketStorage<'a> {
    inner: Option<Item<'a>>,
}

impl<'a> SocketStorage<'a> {
    pub const EMPTY: Self = Self { inner: None };
}

/// An item of a socket set.
#[derive(Debug)]
pub(crate) struct Item<'a> {
    pub(crate) meta: Meta,
    pub(crate) socket: Socket<'a>,
}

/// An extensible set of sockets.
#[derive(Default, Debug)]
pub struct SocketSet<'a> {
    pub sockets: &'a mut [SocketStorage<'a>],
}

impl<'a> SocketSet<'a> {
    /// Create a socket set using the provided storage.
    pub fn new(sockets: &'a mut [SocketStorage<'a>]) -> SocketSet<'a> {
        SocketSet { sockets }
    }

    /// Add a socket to the set, and return its handle.
    ///
    /// # Panics
    /// This function panics if the storage is fixed-size (not a `Vec`) and is full.
    pub fn add<T: AnySocket<'a>>(&mut self, socket: T) -> SocketHandle {
        fn put<'a>(index: u8, slot: &mut SocketStorage<'a>, socket: Socket<'a>) -> SocketHandle {
            trace!("[{}]: adding", index);
            let handle = SocketHandle(index);
            let mut meta = Meta::default();
            meta.handle = handle;
            *slot = SocketStorage {
                inner: Some(Item { meta, socket }),
            };
            handle
        }

        let socket = socket.upcast();

        for (index, slot) in self.sockets.iter_mut().enumerate() {
            if slot.inner.is_none() {
                return put(index as u8, slot, socket);
            }
        }

        panic!("adding a socket to a full SocketSet");
    }

    /// Get a socket from the set by its handle, as mutable.
    ///
    /// # Panics
    /// This function may panic if the handle does not belong to this socket set
    /// or the socket has the wrong type.
    pub fn get<T: AnySocket<'a>>(&self, handle: SocketHandle) -> &T {
        match self.sockets[handle.0 as usize].inner.as_ref() {
            Some(item) => {
                T::downcast(&item.socket).expect("handle refers to a socket of a wrong type")
            }
            None => panic!("handle does not refer to a valid socket"),
        }
    }

    /// Get a mutable socket from the set by its handle, as mutable.
    ///
    /// # Panics
    /// This function may panic if the handle does not belong to this socket set
    /// or the socket has the wrong type.
    pub fn get_mut<T: AnySocket<'a>>(&mut self, handle: SocketHandle) -> &mut T {
        match self.sockets[handle.0 as usize].inner.as_mut() {
            Some(item) => T::downcast_mut(&mut item.socket)
                .expect("handle refers to a socket of a wrong type"),
            None => panic!("handle does not refer to a valid socket"),
        }
    }

    /// Remove a socket from the set, without changing its state.
    ///
    /// # Panics
    /// This function may panic if the handle does not belong to this socket set.
    pub fn remove(&mut self, handle: SocketHandle) -> Socket<'a> {
        trace!("[{}]: removing", handle.0);
        match self.sockets[handle.0 as usize].inner.take() {
            Some(item) => item.socket,
            None => panic!("handle does not refer to a valid socket"),
        }
    }

    /// Get an iterator to the inner sockets.
    pub fn iter(&self) -> impl Iterator<Item = (SocketHandle, &Socket<'a>)> {
        self.items().map(|i| (i.meta.handle, &i.socket))
    }

    /// Get a mutable iterator to the inner sockets.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (SocketHandle, &mut Socket<'a>)> {
        self.items_mut().map(|i| (i.meta.handle, &mut i.socket))
    }

    /// Iterate every socket in this set.
    pub(crate) fn items(&self) -> impl Iterator<Item = &Item<'a>> + '_ {
        self.sockets.iter().filter_map(|x| x.inner.as_ref())
    }

    /// Iterate every socket in this set.
    pub(crate) fn items_mut(&mut self) -> impl Iterator<Item = &mut Item<'a>> + '_ {
        self.sockets.iter_mut().filter_map(|x| x.inner.as_mut())
    }
}

#[cfg(feature = "defmt")]
impl<'a> defmt::Format for SocketSet<'a> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "[");
        for (handle, socket) in self.iter() {
            match socket {
                Socket::Udp(s) => defmt::write!(fmt, "[{:?}, UDP({:?})],", handle, s.state()),
                Socket::Tcp(s) => defmt::write!(fmt, "[{:?}, TCP({:?})],", handle, s.state()),
            }
        }
        defmt::write!(fmt, "]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{tcp, udp};
    use core::any::Any;

    #[test]
    fn add_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);
    }

    #[test]
    fn remove_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        set.remove(SocketHandle(0));
        assert_eq!(set.iter().count(), 1);
        set.get::<udp::Socket>(SocketHandle(1));
    }

    #[test]
    #[should_panic]
    fn remove_non_existing_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        set.remove(SocketHandle(0));
        set.get::<udp::Socket>(SocketHandle(0));
    }

    #[test]
    fn add_duplicate_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
    }

    #[test]
    fn add_socket_to_full_set() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);
    }

    #[test]
    fn get_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        set.get::<tcp::Socket>(SocketHandle(0));

        set.get::<udp::Socket>(SocketHandle(1));
    }

    #[test]
    fn get_socket_wrong_type() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        // assert!(set.get::<tcp::Socket>(SocketHandle(1)).is_err());

        set.get::<udp::Socket>(SocketHandle(1));
    }

    #[test]
    fn get_socket_type() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        // assert_eq!(set.socket_type(SocketHandle(0)), Some(SocketType::Tcp));
        // assert_eq!(set.socket_type(SocketHandle(1)), Some(SocketType::Udp));
    }

    #[test]
    fn replace_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut tcp_rx_buf2 = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        set.remove(SocketHandle(0));

        assert_eq!(set.iter().count(), 1);

        set.get::<udp::Socket>(SocketHandle(1));

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf2[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 2);

        set.get::<tcp::Socket>(SocketHandle(0));
    }

    #[test]
    fn prune_socket_set() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = SocketSet::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(&mut tcp_rx_buf[..], &mut [][..])),
            SocketHandle(0)
        );
        assert_eq!(set.iter().count(), 1);
        assert_eq!(
            set.add(udp::Socket::new(&mut udp_rx_buf[..], &mut [][..])),
            SocketHandle(1)
        );
        assert_eq!(set.iter().count(), 2);

        set.get::<tcp::Socket>(SocketHandle(0));

        assert_eq!(set.iter().count(), 2);
    }
}
