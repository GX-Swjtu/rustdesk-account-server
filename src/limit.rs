use axum::http::HeaderMap;
use ipnet::IpNet;
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

// All keys (including absent usernames) share a hard memory bound. A full table fails closed.
pub struct Limiter {
    buckets: Mutex<HashMap<String, (Instant, usize)>>,
    capacity: usize,
}
impl Limiter {
    pub fn new(capacity: usize) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            capacity,
        }
    }
    pub fn allow(&self, keys: &[(String, usize)]) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            return false;
        };
        let now = Instant::now();
        buckets.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        if keys
            .iter()
            .any(|(key, max)| buckets.get(key).is_some_and(|(_, count)| count >= max))
        {
            return false;
        }
        let new_keys = keys
            .iter()
            .filter(|(key, _)| !buckets.contains_key(key))
            .count();
        if buckets.len() + new_keys > self.capacity {
            return false;
        }
        for (key, _) in keys {
            buckets.entry(key.clone()).or_insert((now, 0)).1 += 1;
        }
        true
    }
}

pub fn client_ip(remote: IpAddr, headers: &HeaderMap, trusted: &[IpNet]) -> IpAddr {
    if !trusted.iter().any(|net| net.contains(&remote)) {
        return remote;
    }
    // Walk right to left; only a trusted immediate proxy can vouch for its predecessor.
    let Some(value) = headers.get("x-forwarded-for").and_then(|x| x.to_str().ok()) else {
        return remote;
    };
    if value.len() > 512 {
        return remote;
    }
    let parts: Vec<_> = value.split(',').collect();
    if parts.len() > 16 {
        return remote;
    }
    let mut current = remote;
    for part in parts.iter().rev() {
        if !trusted.iter().any(|net| net.contains(&current)) {
            break;
        }
        match part.trim().parse::<IpAddr>() {
            Ok(ip) => current = ip,
            Err(_) => return remote,
        }
    }
    current
}
