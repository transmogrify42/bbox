//! WFS service state: feature type registry, stored queries, cached documents.

use crate::config::WfsCfg;
use crate::inventory::Inventory;
use crate::wfs::model::*;
use crate::wfs::store::{create_store, FeatureStore, SchemaMapping};
use crate::wfs::version::Version;
use crate::wfs::xsd::read_feature_types_from_file;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Feature type with its store
pub struct FeatureTypeEntry {
    pub def: Arc<FeatureTypeDef>,
    pub store: Arc<dyn FeatureStore>,
}

/// Namespace with prefix
#[derive(Clone, Debug)]
pub struct Namespace {
    pub prefix: String,
    pub uri: String,
    /// Application schema document (served by DescribeFeatureType)
    pub schema: Option<String>,
}

pub struct WfsService {
    pub cfg: WfsCfg,
    /// Public URL of the `/wfs` endpoint
    pub endpoint_url: String,
    pub types: Vec<FeatureTypeEntry>,
    pub namespaces: Vec<Namespace>,
    /// Value of the updateSequence capabilities attribute
    pub update_sequence: String,
    pub stored_queries: RwLock<Vec<crate::wfs::storedquery::StoredQueryDef>>,
    /// Rendered capabilities documents by (version, sections)
    pub caps_cache: Mutex<HashMap<(Version, String), Arc<String>>>,
    /// Response cache (enabled by cache.response_ttl)
    pub response_cache: Option<crate::wfs::cache::ResponseCache>,
    /// Feature count cache (enabled by cache.count_ttl)
    pub count_cache: Option<crate::wfs::cache::CountCache>,
}

impl WfsService {
    pub async fn create(cfg: &WfsCfg, inventory: &Inventory) -> WfsService {
        let endpoint_url = format!("{}/wfs", inventory.href_prefix());
        // application schemas per namespace
        let mut namespaces = vec![Namespace {
            prefix: cfg.namespace_prefix.clone(),
            uri: cfg.namespace_uri.clone(),
            schema: None,
        }];
        let mut schema_types: Vec<(usize, FeatureTypeDef, Vec<String>)> = Vec::new();
        for (ns_idx, ns) in cfg.namespaces.iter().enumerate() {
            let schema_doc = ns.schema.as_ref().and_then(|path| {
                let path = bbox_core::config::app_dir(path);
                match std::fs::read_to_string(&path) {
                    Ok(s) => Some((path, s)),
                    Err(e) => {
                        warn!("WFS: cannot read schema `{}`: {e}", path.display());
                        None
                    }
                }
            });
            if let Some((path, _)) = &schema_doc {
                match read_feature_types_from_file(&path.to_string_lossy(), &ns.prefix) {
                    Ok(fts) => {
                        for mut ft in fts {
                            if ft.name.ns != ns.uri {
                                warn!(
                                    "WFS: schema namespace `{}` differs from configured `{}`",
                                    ft.name.ns, ns.uri
                                );
                            }
                            ft.name = QName::new(&ns.uri, &ns.prefix, &ft.name.local);
                            schema_types.push((ns_idx, ft, ns.collections.clone()));
                        }
                    }
                    Err(e) => warn!("WFS: invalid schema `{}`: {e}", path.display()),
                }
            }
            if namespaces.iter().all(|n| n.prefix != ns.prefix) {
                namespaces.push(Namespace {
                    prefix: ns.prefix.clone(),
                    uri: ns.uri.clone(),
                    schema: schema_doc.map(|(_, s)| s),
                });
            }
        }
        let mut entries: Vec<(usize, usize, FeatureTypeEntry)> = Vec::new();
        for fc in inventory.feature_collections() {
            let coll = &fc.collection;
            let Some(desc) = fc.source.wfs_source() else {
                continue;
            };
            // schema mapped feature type by collection name
            let mapped = schema_types.iter().enumerate().find(|(_, (_, ft, colls))| {
                if colls.is_empty() {
                    ft.name.local == coll.id
                } else {
                    colls.contains(&coll.id) && ft.name.local == coll.id
                }
            });
            let ns_for_list = cfg
                .namespaces
                .iter()
                .find(|ns| ns.schema.is_none() && ns.collections.contains(&coll.id));
            let (name, mapping, order) = match mapped {
                Some((pos, (_, ft, _))) => (
                    ft.name.clone(),
                    Some(SchemaMapping {
                        feature_type: ft.clone(),
                    }),
                    (0, pos),
                ),
                None => {
                    let (uri, prefix) = match ns_for_list {
                        Some(ns) => (ns.uri.as_str(), ns.prefix.as_str()),
                        None => (cfg.namespace_uri.as_str(), cfg.namespace_prefix.as_str()),
                    };
                    (QName::new(uri, prefix, &ncname(&coll.id)), None, (1, 0))
                }
            };
            match create_store(desc, name.clone(), coll.title.clone(), mapping).await {
                Ok(store) => {
                    let mut def = store.feature_type().clone();
                    if def.abstract_.is_none() {
                        def.abstract_ = coll.description.clone();
                    }
                    entries.push((
                        order.0,
                        order.1,
                        FeatureTypeEntry {
                            def: Arc::new(def),
                            store,
                        },
                    ))
                }
                Err(e) => warn!("WFS: feature type `{}` disabled: {e}", name.prefixed()),
            }
        }
        entries
            .sort_by(|a, b| (a.0, a.1, &a.2.def.name.local).cmp(&(b.0, b.1, &b.2.def.name.local)));
        let types: Vec<FeatureTypeEntry> = entries.into_iter().map(|(_, _, e)| e).collect();
        info!("WFS: {} feature types", types.len());
        let update_sequence = chrono::Utc::now().timestamp().to_string();
        WfsService {
            cfg: cfg.clone(),
            endpoint_url,
            types,
            namespaces,
            update_sequence,
            stored_queries: RwLock::new(crate::wfs::storedquery::builtin()),
            caps_cache: Mutex::new(HashMap::new()),
            response_cache: (cfg.cache.response_ttl > 0).then(|| {
                crate::wfs::cache::ResponseCache::new(
                    std::time::Duration::from_secs(cfg.cache.response_ttl),
                    cfg.cache.max_response_bytes,
                    cfg.cache.capacity_bytes,
                )
            }),
            count_cache: (cfg.cache.count_ttl > 0).then(|| {
                crate::wfs::cache::CountCache::new(std::time::Duration::from_secs(
                    cfg.cache.count_ttl,
                ))
            }),
        }
    }

    /// Number of features matching a filter, using the count cache if enabled
    pub async fn count(
        &self,
        def: &crate::wfs::model::FeatureTypeDef,
        store: &Arc<dyn crate::wfs::store::FeatureStore>,
        filter: Option<crate::wfs::filter::Filter>,
    ) -> Result<u64, crate::wfs::store::StoreError> {
        let Some(cache) = &self.count_cache else {
            return store.count(filter).await;
        };
        let key = format!("{}|{:?}", def.name.prefixed(), filter);
        if let Some(n) = cache.get(&key) {
            return Ok(n);
        }
        let n = store.count(filter).await?;
        cache.insert(key, n);
        Ok(n)
    }

    /// Discard cached responses and counts (e.g. after stored query changes)
    pub fn clear_caches(&self) {
        if let Some(c) = &self.response_cache {
            c.clear();
        }
        if let Some(c) = &self.count_cache {
            c.clear();
        }
    }

    /// Namespaces used by feature types
    pub fn used_namespaces(&self) -> Vec<&Namespace> {
        let mut result: Vec<&Namespace> = Vec::new();
        for t in &self.types {
            if let Some(ns) = self.namespaces.iter().find(|n| n.uri == t.def.name.ns) {
                if !result.iter().any(|r| r.uri == ns.uri) {
                    result.push(ns);
                }
            }
        }
        result
    }

    pub fn prefix_map(&self) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = self
            .namespaces
            .iter()
            .map(|n| (n.prefix.clone(), n.uri.clone()))
            .collect();
        for (p, u) in [
            ("gml", "http://www.opengis.net/gml"),
            ("xlink", "http://www.w3.org/1999/xlink"),
            ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
        ] {
            map.entry(p.to_string()).or_insert_with(|| u.to_string());
        }
        map
    }

    /// Find feature type by namespace/prefix/local name
    pub fn find_type(
        &self,
        ns: Option<&str>,
        prefix: Option<&str>,
        local: &str,
    ) -> Option<&FeatureTypeEntry> {
        let by_local = || self.types.iter().filter(move |t| t.def.name.local == local);
        if let Some(ns) = ns {
            if let Some(t) = by_local().find(|t| t.def.name.ns == ns) {
                return Some(t);
            }
            return None;
        }
        if let Some(prefix) = prefix {
            if let Some(t) = by_local().find(|t| t.def.name.prefix == prefix) {
                return Some(t);
            }
            return None;
        }
        let mut it = by_local();
        let first = it.next();
        if it.next().is_some() {
            // ambiguous: prefer default namespace
            return self
                .types
                .iter()
                .find(|t| t.def.name.local == local && t.def.name.ns == self.cfg.namespace_uri)
                .or(first);
        }
        first
    }

    /// Feature types whose supertypes include the given abstract type (inheritance)
    pub fn find_subtypes(
        &self,
        ns: Option<&str>,
        prefix: Option<&str>,
        local: &str,
    ) -> Vec<&FeatureTypeEntry> {
        self.types
            .iter()
            .filter(|t| {
                t.def.supertypes.iter().any(|s| {
                    s.local == local
                        && ns.map(|n| n == s.ns).unwrap_or(true)
                        && prefix.map(|p| p == s.prefix).unwrap_or(true)
                })
            })
            .collect()
    }
}

/// Make a valid XML NCName from a collection id
pub fn ncname(id: &str) -> String {
    let mut s: String = id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.chars()
        .next()
        .map(|c| !(c.is_alphabetic() || c == '_'))
        .unwrap_or(true)
    {
        s.insert(0, '_');
    }
    s
}
