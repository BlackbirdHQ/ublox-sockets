use embedded_nal::SocketAddr;
use heapless::{spsc::Queue, FnvIndexMap};
use hash32::Hash;

use crate::SocketHandle;

pub struct UdpListener<const N: usize, const L: usize> {
    handles: FnvIndexMap<SocketHandle, u16, N>,
    connections: FnvIndexMap<u16, Queue<(SocketHandle, SocketAddr), L>, N>,
    /// Maps Socket addresses to handles for send_to()  
    outgoing: FnvIndexMap<SocketAddrWrapper, SocketHandle, N>,
}

impl<const N: usize, const L: usize> UdpListener<N, L> {
    pub fn new() -> Self {
        Self {
            handles: FnvIndexMap::new(),
            connections: FnvIndexMap::new(),
            outgoing: FnvIndexMap::new(),
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

    pub fn is_bound(&self, handle: SocketHandle) -> bool {
        self.handles.get(&handle).is_some()
    }

    /// See if a connection is available for server
    pub fn available(&mut self, handle: SocketHandle) -> Result<bool, ()> {
        let port = self.handles.get(&handle).ok_or(())?;
        Ok(!self.connections.get_mut(port).ok_or(())?.is_empty())
    }

    pub fn accept(&mut self, handle: SocketHandle) -> Result<(SocketHandle, SocketAddr), ()> {
        let port = self.handles.get(&handle).ok_or(())?;
        self.connections
            .get_mut(port)
            .ok_or(())?
            .dequeue()
            .ok_or(())
    }

    pub fn outgoing_connection(&mut self, handle: SocketHandle, addr: SocketAddr) -> Result<Option<SocketHandle>, ()> {
        self.outgoing.insert(SocketAddrWrapper(addr), handle).map_err(|_| () )
    }

    pub fn get_outgoing(&mut self, addr: SocketAddr) -> Option<SocketHandle> {
        self.outgoing.remove(&SocketAddrWrapper(addr))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SocketAddrWrapper(SocketAddr);

impl Hash for SocketAddrWrapper{
    fn hash<H>(&self, state: &mut H)
    where
            H: hash32::Hasher {
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