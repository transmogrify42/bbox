//! Response cache (LRU with TTL and size limits), feature count cache and ETags.

use actix_web::web::Bytes;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 64 bit FNV-1a hash
fn fnv1a(data: &[u8], seed: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Strong entity tag of a response body
pub fn etag(body: &[u8]) -> String {
    format!("\"{:016x}{:04x}\"", fnv1a(body, 0), body.len() & 0xffff)
}

/// Whether an If-None-Match header value matches the entity tag
pub fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t == etag || t.strip_prefix("W/") == Some(etag))
}

/// Cache key of a request body (hashed if large)
pub fn body_key(content_type: &str, body: &[u8]) -> String {
    if body.len() <= 4096 {
        format!("POST {content_type}\n{}", String::from_utf8_lossy(body))
    } else {
        format!(
            "POST {content_type}\n#{}:{:016x}:{:016x}",
            body.len(),
            fnv1a(body, 0),
            fnv1a(body, 0x9e37_79b9_7f4a_7c15)
        )
    }
}

/// Cached response
#[derive(Clone)]
pub struct CachedResponse {
    pub content_type: String,
    pub body: Bytes,
    pub etag: String,
}

struct Entry {
    response: CachedResponse,
    expires: Instant,
    tick: u64,
}

#[derive(Default)]
struct Lru {
    entries: HashMap<String, Entry>,
    /// access order: tick -> key
    order: BTreeMap<u64, String>,
    tick: u64,
    size: usize,
}

/// LRU response cache with time to live and size limits
pub struct ResponseCache {
    ttl: Duration,
    pub max_entry: usize,
    capacity: usize,
    lru: Mutex<Lru>,
}

impl ResponseCache {
    pub fn new(ttl: Duration, max_entry: usize, capacity: usize) -> Self {
        ResponseCache {
            ttl,
            max_entry: max_entry.min(capacity),
            capacity,
            lru: Mutex::new(Lru::default()),
        }
    }

    pub fn get(&self, key: &str) -> Option<CachedResponse> {
        let mut lru = self.lru.lock().expect("cache lock");
        let lru = &mut *lru;
        let now = Instant::now();
        let entry = lru.entries.get_mut(key)?;
        if entry.expires <= now {
            let size = entry.response.body.len();
            let tick = entry.tick;
            lru.entries.remove(key);
            lru.order.remove(&tick);
            lru.size -= size;
            return None;
        }
        lru.tick += 1;
        lru.order.remove(&entry.tick);
        entry.tick = lru.tick;
        lru.order.insert(lru.tick, key.to_string());
        Some(entry.response.clone())
    }

    pub fn insert(&self, key: String, response: CachedResponse) {
        let size = response.body.len();
        if size > self.max_entry {
            return;
        }
        let mut lru = self.lru.lock().expect("cache lock");
        let lru = &mut *lru;
        if let Some(old) = lru.entries.remove(&key) {
            lru.order.remove(&old.tick);
            lru.size -= old.response.body.len();
        }
        // evict least recently used entries
        while lru.size + size > self.capacity {
            let Some((_, k)) = lru.order.pop_first() else {
                break;
            };
            if let Some(e) = lru.entries.remove(&k) {
                lru.size -= e.response.body.len();
            }
        }
        lru.tick += 1;
        lru.order.insert(lru.tick, key.clone());
        lru.size += size;
        lru.entries.insert(
            key,
            Entry {
                response,
                expires: Instant::now() + self.ttl,
                tick: lru.tick,
            },
        );
    }

    pub fn clear(&self) {
        let mut lru = self.lru.lock().expect("cache lock");
        *lru = Lru::default();
    }
}

/// Feature count cache (numberMatched) with time to live
pub struct CountCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (u64, Instant)>>,
}

/// Maximum number of cached counts
const MAX_COUNTS: usize = 100_000;

impl CountCache {
    pub fn new(ttl: Duration) -> Self {
        CountCache {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<u64> {
        let entries = self.entries.lock().expect("count cache lock");
        entries
            .get(key)
            .filter(|(_, expires)| *expires > Instant::now())
            .map(|(n, _)| *n)
    }

    pub fn insert(&self, key: String, count: u64) {
        let mut entries = self.entries.lock().expect("count cache lock");
        if entries.len() >= MAX_COUNTS {
            let now = Instant::now();
            entries.retain(|_, (_, expires)| *expires > now);
            if entries.len() >= MAX_COUNTS {
                entries.clear();
            }
        }
        entries.insert(key, (count, Instant::now() + self.ttl));
    }

    pub fn clear(&self) {
        self.entries.lock().expect("count cache lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(body: &str) -> CachedResponse {
        CachedResponse {
            content_type: "text/xml".to_string(),
            body: Bytes::from(body.to_string()),
            etag: etag(body.as_bytes()),
        }
    }

    #[test]
    fn lru_eviction_and_limits() {
        let cache = ResponseCache::new(Duration::from_secs(60), 4, 10);
        cache.insert("a".into(), resp("aaaa"));
        cache.insert("b".into(), resp("bbbb"));
        assert!(cache.get("a").is_some()); // a is now most recent
        cache.insert("c".into(), resp("cccc")); // evicts b
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
        cache.insert("big".into(), resp("toolarge")); // over max entry size
        assert!(cache.get("big").is_none());
        cache.insert("a".into(), resp("a2")); // replace
        assert_eq!(cache.get("a").unwrap().body, Bytes::from("a2"));
        cache.clear();
        assert!(cache.get("c").is_none());
    }

    #[test]
    fn ttl_expiry() {
        let cache = ResponseCache::new(Duration::from_millis(0), 100, 100);
        cache.insert("a".into(), resp("x"));
        assert!(cache.get("a").is_none());
        let counts = CountCache::new(Duration::from_millis(0));
        counts.insert("k".into(), 3);
        assert_eq!(counts.get("k"), None);
        let counts = CountCache::new(Duration::from_secs(60));
        counts.insert("k".into(), 3);
        assert_eq!(counts.get("k"), Some(3));
    }

    #[test]
    fn etags() {
        let e = etag(b"hello");
        assert_eq!(e, etag(b"hello"));
        assert_ne!(e, etag(b"hellp"));
        assert!(etag_matches(&e, &e));
        assert!(etag_matches(&format!("\"x\", W/{e}"), &e));
        assert!(etag_matches("*", &e));
        assert!(!etag_matches("\"x\"", &e));
    }
}
