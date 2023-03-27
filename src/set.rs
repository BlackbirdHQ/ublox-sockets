use super::{AnySocket, Error, Result, Socket, SocketType};
use atat::atat_derive::AtatLen;
use serde::{Deserialize, Serialize};

/// A handle, identifying a socket in a set.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    AtatLen,
    Ord,
    hash32_derive::Hash32,
    Default,
    Serialize,
    Deserialize,
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Handle(pub u8);

#[derive(Debug, Default)]
pub struct SocketStorage<'a> {
    inner: Option<Socket<'a>>,
}

impl<'a> SocketStorage<'a> {
    pub const EMPTY: Self = Self { inner: None };
}

/// An extensible set of sockets.
#[derive(Default, Debug)]
pub struct Set<'a> {
    pub sockets: &'a mut [SocketStorage<'a>],
}

impl<'a> Set<'a> {
    /// Create a socket set using the provided storage.
    pub fn new(sockets: &'a mut [SocketStorage<'a>]) -> Set<'a> {
        Set { sockets }
    }

    /// Get the maximum number of sockets the set can hold
    pub fn capacity(&self) -> usize {
        self.sockets.len()
    }

    /// Get the current number of initialized sockets, the set is holding
    pub fn len(&self) -> usize {
        self.sockets.iter().filter(|a| a.inner.is_some()).count()
    }

    /// Check if the set is currently holding no active sockets
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the type of a specific socket in the set.
    ///
    /// Returned as a [`SocketType`]
    pub fn socket_type(&self, handle: Handle) -> Option<SocketType> {
        if let Ok(index) = self.index_of(handle) {
            if let Some(socket) = self.sockets.get(index) {
                return socket.inner.as_ref().map(|s| s.get_type());
            }
        }
        None
    }

    /// Add a socket to the set with the reference count 1, and return its handle.
    pub fn add<T>(&mut self, socket: T) -> Result<Handle>
    where
        T: AnySocket<'a>,
    {
        let socket = socket.upcast();
        let handle = socket.handle();

        debug!(
            "[Socket Set] Adding: {} {:?} to: {:?}",
            handle.0,
            socket.get_type(),
            self
        );

        if self.index_of(handle).is_ok() {
            return Err(Error::DuplicateSocket);
        }

        self.sockets
            .iter_mut()
            .find(|s| s.inner.is_none())
            .ok_or(Error::SocketSetFull)?
            .inner
            .replace(socket);

        Ok(handle)
    }

    /// Get a socket from the set by its handle, as mutable.
    pub fn get<T: AnySocket<'a>>(&mut self, handle: Handle) -> Result<&T> {
        let index = self.index_of(handle)?;

        match self
            .sockets
            .get_mut(index)
            .ok_or(Error::InvalidSocket)?
            .inner
        {
            Some(ref socket) => T::downcast(socket).ok_or(Error::InvalidSocket),
            None => Err(Error::InvalidSocket),
        }
    }

    /// Get the index of a given socket in the set.
    fn index_of(&self, handle: Handle) -> Result<usize> {
        self.sockets
            .iter()
            .position(|i| {
                i.inner
                    .as_ref()
                    .map(|s| s.handle().0 == handle.0)
                    .unwrap_or(false)
            })
            .ok_or(Error::InvalidSocket)
    }

    /// Remove a socket from the set
    pub fn remove(&mut self, handle: Handle) -> Result<()> {
        let index = self.index_of(handle)?;
        let storage = self.sockets.get_mut(index).ok_or(Error::InvalidSocket)?;

        let item: &mut Option<Socket<'a>> = &mut storage.inner;

        debug!(
            "[Socket Set] Removing socket! {} {:?}",
            handle.0,
            item.as_ref().map(|i| i.get_type())
        );

        item.take().ok_or(Error::InvalidSocket)?;
        Ok(())
    }

    /// Prune the sockets in this set.
    ///
    /// All sockets are removed and dropped.
    pub fn prune(&mut self) {
        debug!("[Socket Set] Pruning: {:?}", self);
        self.sockets.iter_mut().enumerate().for_each(|(_, slot)| {
            slot.inner.take();
        })
    }

    pub fn recycle(&mut self) -> bool {
        let h = self.iter().find(|(_, s)| s.recycle()).map(|(h, _)| h);
        if h.is_none() {
            return false;
        }
        self.remove(h.unwrap()).is_ok()
    }

    /// Iterate every socket in this set.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &Socket<'a>)> {
        self.items().map(|i| (i.handle(), i))
    }

    /// Iterate every socket in this set, as SocketRef.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Handle, &mut Socket<'a>)> {
        self.items_mut().map(|i| (i.handle(), i))
    }

    /// Iterate every socket in this set.
    pub(crate) fn items(&self) -> impl Iterator<Item = &Socket<'a>> + '_ {
        self.sockets.iter().filter_map(|x| x.inner.as_ref())
    }

    /// Iterate every socket in this set.
    pub(crate) fn items_mut(&mut self) -> impl Iterator<Item = &mut Socket<'a>> + '_ {
        self.sockets.iter_mut().filter_map(|x| x.inner.as_mut())
    }
}

#[cfg(feature = "defmt")]
impl<const N: usize, const L: usize> defmt::Format for Set<N, L> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "[");
        for socket in self.iter() {
            match socket.1 {
                Socket::Udp(s) => defmt::write!(fmt, "[{:?}, UDP({:?})],", socket.0, s.state()),
                Socket::Tcp(s) => defmt::write!(fmt, "[{:?}, TCP({:?})],", socket.0, s.state()),
            }
        }
        defmt::write!(fmt, "]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{tcp, udp};

    #[test]
    fn add_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn remove_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        assert!(set.remove(Handle(0)).is_ok());
        assert_eq!(set.len(), 1);

        assert!(set.get::<tcp::Socket>(Handle(0)).is_err());

        set.get::<udp::Socket>(Handle(1))
            .expect("failed to get udp socket");
    }

    #[test]
    fn add_duplicate_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(0, &mut udp_rx_buf[..])),
            Err(Error::DuplicateSocket)
        );
    }

    #[test]
    fn add_socket_to_full_set() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];
        let mut udp_rx_buf2 = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.add(udp::Socket::new(2, &mut udp_rx_buf2[..])),
            Err(Error::SocketSetFull)
        );
    }

    #[test]
    fn get_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        set.get::<tcp::Socket>(Handle(0))
            .expect("failed to get tcp socket");

        set.get::<udp::Socket>(Handle(1))
            .expect("failed to get udp socket");
    }

    #[test]
    fn get_socket_wrong_type() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        assert!(set.get::<tcp::Socket>(Handle(1)).is_err());

        set.get::<udp::Socket>(Handle(1))
            .expect("failed to get udp socket");
    }

    #[test]
    fn get_socket_type() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        assert_eq!(set.socket_type(Handle(0)), Some(SocketType::Tcp));
        assert_eq!(set.socket_type(Handle(1)), Some(SocketType::Udp));
    }

    #[test]
    fn replace_socket() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut tcp_rx_buf2 = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        assert!(set.remove(Handle(0)).is_ok());
        assert_eq!(set.len(), 1);

        assert!(set.get::<tcp::Socket>(Handle(0)).is_err());

        set.get::<udp::Socket>(Handle(1))
            .expect("failed to get udp socket");

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf2[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 2);

        set.get::<tcp::Socket>(Handle(0))
            .expect("failed to get tcp socket");
    }

    #[test]
    fn prune_socket_set() {
        let mut sockets = [SocketStorage::EMPTY; 2];
        let mut set = Set::new(&mut sockets);

        let mut tcp_rx_buf = [0u8; 64];
        let mut udp_rx_buf = [0u8; 48];

        assert_eq!(
            set.add(tcp::Socket::new(0, &mut tcp_rx_buf[..])),
            Ok(Handle(0))
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.add(udp::Socket::new(1, &mut udp_rx_buf[..])),
            Ok(Handle(1))
        );
        assert_eq!(set.len(), 2);

        set.get::<tcp::Socket>(Handle(0))
            .expect("failed to get tcp socket");

        set.prune();
        assert_eq!(set.len(), 0);
    }
}
