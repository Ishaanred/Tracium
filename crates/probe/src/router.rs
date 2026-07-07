//! Optional router monitoring over SNMP v2c (`csnmp`).
//!
//! On-demand and config-gated: the user supplies the router IP and community
//! string (SNMP is off by default on most consumer routers). We read only
//! universal SNMPv2-MIB OIDs (description, name, uptime) so it works on any
//! SNMP-capable device; vendor-specific CPU/memory OIDs can be added later.
//! Any failure (unreachable, SNMP disabled, wrong community) yields `None`.

use std::net::SocketAddr;
use std::time::Duration;

use csnmp::{ObjectIdentifier, ObjectValue, Snmp2cClient};

/// Basic router facts read over SNMP.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouterInfo {
    pub descr: Option<String>,
    pub name: Option<String>,
    pub uptime_secs: Option<u64>,
}

const OID_SYS_DESCR: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 1, 0];
const OID_SYS_NAME: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 5, 0];
const OID_SYS_UPTIME: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 3, 0];

/// Query `target` (usually `router_ip:161`) with the given community string.
pub async fn query_router(target: SocketAddr, community: &str, timeout: Duration) -> Option<RouterInfo> {
    let client =
        Snmp2cClient::new(target, community.as_bytes().to_vec(), None, Some(timeout)).await.ok()?;

    let info = RouterInfo {
        descr: get_string(&client, OID_SYS_DESCR).await,
        name: get_string(&client, OID_SYS_NAME).await,
        uptime_secs: get_uptime_secs(&client, OID_SYS_UPTIME).await,
    };
    (info != RouterInfo::default()).then_some(info)
}

async fn get(client: &Snmp2cClient, oid: &[u32]) -> Option<ObjectValue> {
    let oid = ObjectIdentifier::try_from(oid).ok()?;
    client.get(oid).await.ok()
}

async fn get_string(client: &Snmp2cClient, oid: &[u32]) -> Option<String> {
    match get(client, oid).await? {
        ObjectValue::String(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        _ => None,
    }
}

async fn get_uptime_secs(client: &Snmp2cClient, oid: &[u32]) -> Option<u64> {
    // sysUpTime is TimeTicks in hundredths of a second.
    match get(client, oid).await? {
        ObjectValue::TimeTicks(t) => Some(t as u64 / 100),
        _ => None,
    }
}
