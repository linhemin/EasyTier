use std::net::Ipv6Addr;
use std::sync::Mutex;

use cidr::Ipv6Cidr;
use dashmap::DashMap;

use crate::common::PeerId;

#[derive(Debug)]
pub struct Ipv6Allocator {
    prefix: Ipv6Cidr,
    next: Mutex<u128>,
    assigned: DashMap<PeerId, Ipv6Addr>,
}

impl Ipv6Allocator {
    pub fn new(prefix: Ipv6Cidr) -> Self {
        Self {
            prefix,
            next: Mutex::new(1),
            assigned: DashMap::new(),
        }
    }

    pub fn allocate(&self, peer_id: PeerId) -> Option<Ipv6Addr> {
        if let Some(addr) = self.assigned.get(&peer_id) {
            return Some(*addr);
        }
        let host_bits = 128 - self.prefix.network_length() as u8;
        let max_hosts: u128 = 1u128 << host_bits;
        let mut idx = self.next.lock().unwrap();
        if *idx >= max_hosts {
            return None;
        }
        let base: u128 = self.prefix.first_address().into();
        let addr = Ipv6Addr::from(base + *idx);
        *idx += 1;
        self.assigned.insert(peer_id, addr);
        Some(addr)
    }
}
