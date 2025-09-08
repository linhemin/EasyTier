use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;

use cidr::Ipv6Inet;

use crate::common::error::Error;
use crate::common::global_ctx::ArcGlobalCtx;
use crate::peers::peer_manager::PeerManager;
use crate::proto::ipv6_delegate::{
    Ipv6DelegateRpc, RequestDelegationRequest, RequestDelegationResponse,
};
use crate::proto::rpc_types;
use crate::proto::rpc_types::controller::BaseController;
use nix::sys::socket::SockaddrLike;

#[derive(Clone)]
pub struct Ipv6DelegateServer {
    peer_mgr: Arc<PeerManager>,
    global_ctx: ArcGlobalCtx,
}

impl Ipv6DelegateServer {
    pub fn new(peer_mgr: Arc<PeerManager>, global_ctx: ArcGlobalCtx) -> Self {
        Self {
            peer_mgr,
            global_ctx,
        }
    }

    fn list_onlink_prefixes(&self) -> Vec<(String, Ipv6Inet)> {
        #[cfg(target_os = "linux")]
        {
            let mut ret = Vec::new();
            // Enumerate interfaces via getifaddrs from netlink ifcfg
            // We reuse IfConfiger Linux impl: list addresses by name requires a name,
            // so we iterate getifaddrs directly here.
            use nix::ifaddrs::getifaddrs;
            if let Ok(addrs) = getifaddrs() {
                for iface in addrs {
                    let name = iface.interface_name;
                    if name.starts_with("lo")
                        || name.starts_with("tun")
                        || name.starts_with("utun")
                        || name.starts_with("wg")
                        || name.starts_with("docker")
                        || name.starts_with("veth")
                        || name.starts_with("br-")
                        || name.starts_with("virbr")
                    {
                        continue;
                    }
                    let (Some(address), Some(netmask)) = (iface.address, iface.netmask) else {
                        continue;
                    };
                    if address.family() == Some(nix::sys::socket::AddressFamily::Inet6)
                        && netmask.family() == Some(nix::sys::socket::AddressFamily::Inet6)
                    {
                        let ip: Ipv6Addr = address.as_sockaddr_in6().unwrap().ip();
                        let mask: Ipv6Addr = netmask.as_sockaddr_in6().unwrap().ip();
                        // Only global-scope IPv6
                        if ip.is_multicast()
                            || ip.is_loopback()
                            || ip.is_unspecified()
                            || ip.is_unique_local()
                            || ip.is_unicast_link_local()
                        {
                            continue;
                        }
                        let prefix =
                            pnet::ipnetwork::ip_mask_to_prefix(IpAddr::V6(mask)).unwrap_or(64);
                        if prefix != 64 {
                            continue;
                        }
                        if let Ok(inet) = Ipv6Inet::new(ip, 64) {
                            ret.push((name, inet));
                        }
                    }
                }
            }
            ret
        }
        #[cfg(not(target_os = "linux"))]
        {
            Vec::new()
        }
    }

    fn get_tun_ifname(&self) -> Option<String> {
        // Heuristic: find the interface that owns our overlay IPv4 address
        let ipv4 = self.global_ctx.get_ipv4()?.address();
        use nix::ifaddrs::getifaddrs;
        for iface in getifaddrs().ok()?.filter(|x| x.address.is_some()) {
            let addr = iface.address.unwrap();
            if addr.family() == Some(nix::sys::socket::AddressFamily::Inet) {
                let ip = addr.as_sockaddr_in().unwrap().ip();
                if ip == ipv4 {
                    return Some(iface.interface_name);
                }
            }
        }
        None
    }

    fn alloc_addrs_for_peer(&self, requester_peer_id: u32, count: u32) -> Vec<(String, Ipv6Inet)> {
        // one per on-link /64 by default
        let prefixes = self.list_onlink_prefixes();
        if prefixes.is_empty() {
            return vec![];
        }
        let want = if count == 0 {
            prefixes.len() as u32
        } else {
            count.min(prefixes.len() as u32)
        } as usize;

        let mut out = Vec::new();
        let mut used_iids: HashSet<u64> = HashSet::new();
        for (idx, (iface, pfx)) in prefixes.into_iter().enumerate() {
            if idx >= want {
                break;
            }
            // deterministic IID based on (peer_id, prefix)
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(requester_peer_id.to_be_bytes());
            hasher.update(pfx.first_address().octets());
            let digest = hasher.finalize();
            let mut iid_bytes = [0u8; 8];
            iid_bytes.copy_from_slice(&digest[0..8]);
            let mut iid = u64::from_be_bytes(iid_bytes);
            iid |= 1; // avoid :: as iid
                      // avoid duplicates just in case
            let mut salt = 0u64;
            while used_iids.contains(&iid) {
                iid = iid.wrapping_add(1 + salt);
                salt = salt.wrapping_add(1);
            }
            used_iids.insert(iid);

            let mut segs = pfx.first_address().segments();
            let hi = ((iid >> 48) & 0xFFFF) as u16;
            let h2 = ((iid >> 32) & 0xFFFF) as u16;
            let h3 = ((iid >> 16) & 0xFFFF) as u16;
            let lo = (iid & 0xFFFF) as u16;
            segs[4] = hi;
            segs[5] = h2;
            segs[6] = h3;
            segs[7] = lo;
            let addr = Ipv6Addr::from(segs);
            let inet = Ipv6Inet::new(addr, 128).unwrap();
            out.push((iface, inet));
        }
        out
    }

    #[cfg(target_os = "linux")]
    fn enable_sysctl(&self, key: &str, val: &str) {
        let _ = std::fs::write(format!("/proc/sys/{key}"), val);
    }

    #[cfg(target_os = "linux")]
    fn install_ndp_proxy_and_route(&self, iface: &str, tun: &str, addr: Ipv6Addr) {
        // Enable forwarding and proxy_ndp
        self.enable_sysctl("net/ipv6/conf/all/forwarding", "1");
        self.enable_sysctl(&format!("net/ipv6/conf/{iface}/proxy_ndp"), "1");
        // ip -6 route add <addr>/128 dev <tun>
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("ip -6 route replace {}/128 dev {}", addr, tun))
            .status();
        // ip -6 neigh add proxy <addr> dev <iface>
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("ip -6 neigh replace proxy {} dev {}", addr, iface))
            .status();
    }
}

#[async_trait::async_trait]
impl Ipv6DelegateRpc for Ipv6DelegateServer {
    type Controller = BaseController;
    async fn request_delegation(
        &self,
        _ctrl: BaseController,
        request: RequestDelegationRequest,
    ) -> Result<RequestDelegationResponse, rpc_types::error::Error> {
        // Check flag
        if !self
            .global_ctx
            .config
            .get_flags()
            .enable_ipv6_delegate_server
        {
            return Ok(RequestDelegationResponse {
                addrs: vec![],
                error: "server disabled".to_string(),
                server_overlay_ipv6: None,
            });
        }
        let addrs = self.alloc_addrs_for_peer(request.requester_peer_id, request.count);
        if addrs.is_empty() {
            return Ok(RequestDelegationResponse {
                addrs: vec![],
                error: "no on-link /64 found".to_string(),
                server_overlay_ipv6: None,
            });
        }
        // Apply OS-level proxying (Linux only)
        #[cfg(target_os = "linux")]
        if let Some(tun) = self.get_tun_ifname() {
            for (iface, inet) in &addrs {
                self.install_ndp_proxy_and_route(iface, &tun, inet.address());
            }
        }
        // Ensure server also has an overlay IPv6 so clients can set exit-node
        let server_overlay_ipv6 = addrs.first().map(|(_, inet)| inet.address());
        // Use a leading underscore to avoid unused-variable warning on non-Linux targets
        if let Some(_overlay) = server_overlay_ipv6 {
            // assign to tun if possible (shell out on Linux)
            #[cfg(target_os = "linux")]
            if let Some(tun) = self.get_tun_ifname() {
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!("ip -6 addr replace {}/128 dev {}", _overlay, tun))
                    .status();
            }
        }
        let resp = RequestDelegationResponse {
            addrs: addrs.into_iter().map(|(_, inet)| inet.into()).collect(),
            error: String::new(),
            server_overlay_ipv6: server_overlay_ipv6.map(Into::into),
        };
        Ok(resp)
    }
}

pub async fn configure_source_policy_for_addr(
    _global_ctx: &ArcGlobalCtx,
    _addr: Ipv6Addr,
) -> Result<(), Error> {
    // Linux only for now
    #[cfg(target_os = "linux")]
    {
        let tun = {
            use nix::ifaddrs::getifaddrs;
            let mut found: Option<String> = None;
            if let Some(ipv4) = _global_ctx.get_ipv4().map(|x| x.address()) {
                if let Ok(addrs) = getifaddrs() {
                    for iface in addrs {
                        if let Some(a) = iface.address {
                            if a.family() == Some(nix::sys::socket::AddressFamily::Inet) {
                                let ip = a.as_sockaddr_in().unwrap().ip();
                                if ip == ipv4 {
                                    found = Some(iface.interface_name);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            found
        };
        if let Some(tun) = tun {
            // Use a fixed table id range for EasyTier
            let table_id =
                50000u32 + (u16::from_be_bytes(_addr.segments()[7].to_be_bytes()) as u32 % 4096);
            let rule_del = format!(
                "ip -6 rule del from {}/128 table {} 2>/dev/null || true",
                _addr, table_id
            );
            let rule_add = format!(
                "ip -6 rule add from {}/128 table {} priority 1000",
                _addr, table_id
            );
            let route = format!("ip -6 route replace default dev {} table {}", tun, table_id);
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(rule_del)
                .status();
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(route)
                .status();
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(rule_add)
                .status();
        }
    }
    Ok(())
}
