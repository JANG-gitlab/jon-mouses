use std::net::{IpAddr, SocketAddr};

use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::wire::{Error, Key};

const SERVICE: &str = "_jonmouses._tcp.local.";

pub enum Discovery {
    Found { service: String, key: Key, name: String, addr: SocketAddr },
    Lost { service: String },
}

pub fn advertise(public: &Key, name: &str, port: u16) -> Result<(ServiceDaemon, Receiver<ServiceEvent>), Error> {
    let daemon = ServiceDaemon::new()?;
    let id = hex::encode(&public[..8]);
    let info = ServiceInfo::new(
        SERVICE,
        &id,
        &format!("jm-{id}.local."),
        "",
        port,
        &[("key", hex::encode(public).as_str()), ("name", name)][..],
    )?
    .enable_addr_auto();
    daemon.register(info)?;
    let events = daemon.browse(SERVICE)?;
    Ok((daemon, events))
}

pub fn interpret(event: ServiceEvent, own: &Key) -> Option<Discovery> {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let key: Key = hex::decode(info.get_property_val_str("key")?).ok()?.try_into().ok()?;
            if &key == own {
                return None;
            }
            let ip = info.get_addresses_v4().into_iter().next()?;
            Some(Discovery::Found {
                service: info.fullname.clone(),
                key,
                name: info.get_property_val_str("name").unwrap_or("?").chars().take(64).collect(),
                addr: SocketAddr::new(IpAddr::V4(ip), info.port),
            })
        }
        ServiceEvent::ServiceRemoved(_, service) => Some(Discovery::Lost { service }),
        _ => None,
    }
}
