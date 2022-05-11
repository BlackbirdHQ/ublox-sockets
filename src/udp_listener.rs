use embedded_nal::SocketAddr;
use hash32::Hash;
use heapless::{spsc::Queue, FnvIndexMap};

use crate::SocketHandle;

pub struct UdpListener<const N: usize, const L: usize> {
    /// Maps Server Socket handles to ports
    handles: FnvIndexMap<SocketHandle, u16, N>,
    /// Maps Connection Sockets to remote socket address
    connections: FnvIndexMap<u16, Queue<(SocketHandle, SocketAddr), L>, N>,
}

impl<const N: usize, const L: usize> UdpListener<N, L> {
    pub fn new() -> Self {
        Self {
            handles: FnvIndexMap::new(),
            connections: FnvIndexMap::new(),
        }
    }

    pub fn bind(&mut self, handle: SocketHandle, port: u16) -> Result<(), ()> {
        if self.handles.contains_key(&handle) {
            return Err(());
        }

        self.handles.insert(handle, port).map_err(drop)?;
        self.connections.insert(port, Queue::new()).map_err(drop)?;

        Ok(())
    }

    /// Get incomming connection queue for port
    pub fn incoming(&mut self, port: u16) -> Option<&mut Queue<(SocketHandle, SocketAddr), L>> {
        self.connections.get_mut(&port)
    }

    /// Returns true if port is UDP server port
    pub fn is_port_bound(&self, port: u16) -> bool {
        self.connections.get(&port).is_some()
    }

    /// Returns true if socket is UDP server socket
    pub fn is_bound(&self, handle: SocketHandle) -> bool {
        self.handles.get(&handle).is_some()
    }

    /// See if a connection is available for server
    pub fn available(&mut self, handle: SocketHandle) -> Result<bool, ()> {
        let port = self.handles.get(&handle).ok_or(())?;
        Ok(!self.connections.get_mut(port).ok_or(())?.is_empty())
    }

    /// Gives the first connected socket to receive from.
    /// To peek next socket, send_to for first socket.
    pub fn peek_remote(&mut self, handle: SocketHandle) -> Result<&(SocketHandle, SocketAddr), ()> {
        let port = self.handles.get(&handle).ok_or(())?;
        self.connections.get_mut(port).ok_or(())?.peek().ok_or(())
    }

    /// Gives the first connected socket to receive from.
    /// To get next socket, removing it from queue.
    pub fn get_remote(&mut self, handle: SocketHandle) -> Result<&(SocketHandle, SocketAddr), ()> {
        let port = self.handles.get(&handle).ok_or(())?;
        self.connections.get_mut(port).ok_or(())?.peek().ok_or(())
    }

    /// Gives an outgoing connection, if first in queue matches socketaddr
    /// Removes it from stack.
    pub fn get_outgoing(
        &mut self,
        handle: &SocketHandle,
        addr: SocketAddr,
    ) -> Option<SocketHandle> {
        let port = self.handles.get(handle)?;
        let queue = self.connections.get_mut(port)?;
        let (_, queue_addr) = queue.peek()?;
        if *queue_addr == addr {
            let (handle, _) = queue.dequeue()?;
            return Some(handle);
        }
        None
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SocketAddrWrapper(SocketAddr);

impl Hash for SocketAddrWrapper {
    fn hash<H>(&self, state: &mut H)
    where
        H: hash32::Hasher,
    {
        match self.0 {
            SocketAddr::V4(ip) => {
                ip.ip().octets().hash(state);
                ip.port().hash(state);
            }
            SocketAddr::V6(ip) => {
                ip.ip().octets().hash(state);
                ip.port().hash(state);
            }
        }
    }
}
