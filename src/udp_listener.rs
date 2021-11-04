use embedded_nal::{IpAddr, SocketAddr};
use heapless::{spsc::Queue, FnvIndexMap};

use crate::SocketHandle;

pub struct UdpListener<const N: usize, const L: usize> {
    handles: FnvIndexMap<SocketHandle, u16, N>,
    connections: FnvIndexMap<u16, Queue<(SocketHandle, SocketAddr), L>, N>,
    /// Maps Socket addresses to handles for send_to()  
    // outgoing: FnvIndexMap<SocketAddr, SocketHandle, N>,
    outgoing: FnvIndexMap<u8, SocketHandle, N>,
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

    pub fn incoming(&mut self, port: u16) -> Option<&mut Queue<(SocketHandle, SocketAddr), L>> {
        self.connections.get_mut(&port)
    }

    pub fn is_bound(&self, handle: SocketHandle) -> bool {
        self.handles.get(&handle).is_some()
    }


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
        if let IpAddr::V4(ip) = addr.ip(){
            if self.outgoing.contains_key(&ip.octets()[3]) {
                return Err(());
            }
            self.outgoing.insert(ip.octets()[3], handle).map_err(|_| () )
        } else {
            Err(())
        }
    }

    pub fn get_outgoing(&mut self, addr: SocketAddr) -> Result<Option<SocketHandle>, ()> {
        if let IpAddr::V4(ip) = addr.ip(){
            Ok(self.outgoing.remove(&ip.octets()[3]))
        } else {
            Err(())
        }
    }
}
