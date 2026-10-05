//! ClickHouse feature source (native protocol or HTTP), served by the WFS feature store.

use crate::config::{CollectionSourceCfg, ConfiguredCollectionCfg};
use crate::datasource::{CollectionDatasource, CollectionSource, ItemsResult};
use crate::error::{Error, Result};
use crate::filter_params::FilterParams;
use crate::inventory::FeatureCollection;
use crate::wfs::filter::{Filter, GeometryLiteral, ResourceId, SpatialOp, SpatialOperand};
use crate::wfs::formats::geojson_geometry;
use crate::wfs::model::{Feature, QName, Value};
use crate::wfs::store::clickhouse::{ChClient, ChStore};
use crate::wfs::store::{FeatureStore, SourceDesc, StoreQuery};
use async_trait::async_trait;
use bbox_core::config::DsClickhouseCfg;
use bbox_core::ogcapi::*;
use futures::TryStreamExt;
use log::info;
use std::sync::Arc;

#[derive(Clone)]
pub struct ChDatasource {
    pub client: ChClient,
}

pub type Datasource = ChDatasource;

impl ChDatasource {
    pub async fn from_config(cfg: &DsClickhouseCfg, envvar: Option<String>) -> Result<Self> {
        let url = envvar.unwrap_or(cfg.url.clone());
        let client = ChClient::connect(&url)
            .await
            .map_err(|e| Error::DatasourceSetupError(e.to_string()))?;
        Ok(ChDatasource { client })
    }
}

/// OGC API Features source backed by the WFS ClickHouse store
#[derive(Clone)]
pub struct ChCollectionSource {
    client: ChClient,
    cfg: crate::config::ClickhouseCollectionCfg,
    store: Arc<ChStore>,
}

#[async_trait]
impl CollectionDatasource for ChDatasource {
    async fn setup_collection(
        &mut self,
        cfg: &ConfiguredCollectionCfg,
        base_url: &str,
        _extent: Option<CoreExtent>,
    ) -> Result<FeatureCollection> {
        info!("Setup ClickHouse Collection `{}`", cfg.name);
        let CollectionSourceCfg::Clickhouse(ref srccfg) = cfg.source else {
            panic!();
        };
        let store = ChStore::create(
            self.client.clone(),
            srccfg,
            QName::new("", "", &cfg.name),
            cfg.title.clone(),
            None,
        )
        .await
        .map_err(|e| Error::DatasourceSetupError(e.to_string()))?;
        let ft = store.feature_type();
        let extent = ft.wgs84_bbox.map(|r| CoreExtent {
            spatial: Some(CoreExtentSpatial {
                bbox: vec![vec![r.min().x, r.min().y, r.max().x, r.max().y]],
                crs: None,
            }),
            temporal: None,
        });
        let id = &cfg.name;
        let collection = CoreCollection {
            id: id.clone(),
            title: cfg.title.clone().or(Some(id.clone())),
            description: cfg.description.clone(),
            extent,
            item_type: None,
            crs: vec![],
            links: vec![ApiLink {
                href: format!("{base_url}/collections/{id}/items"),
                rel: Some("items".to_string()),
                type_: Some("application/geo+json".to_string()),
                title: cfg.title.clone(),
                hreflang: None,
                length: None,
            }],
        };
        let source = ChCollectionSource {
            client: self.client.clone(),
            cfg: srccfg.clone(),
            store: Arc::new(store),
        };
        Ok(FeatureCollection {
            collection,
            source: Box::new(source),
        })
    }
}

fn core_feature(store: &ChStore, f: &Feature) -> CoreFeature {
    let def = store.feature_type();
    let geom_idx = def.default_geometry().map(|(i, _)| i);
    let geometry = match geom_idx.map(|i| &f.values[i]) {
        Some(Value::Geometry(g)) => {
            let mut s = String::new();
            geojson_geometry(&mut s, &g.geometry);
            serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
        }
        _ => serde_json::Value::Null,
    };
    let mut props = serde_json::Map::new();
    for (i, p) in def.properties.iter().enumerate() {
        if Some(i) == geom_idx || p.name.is_gml() {
            continue;
        }
        let v = match &f.values[i] {
            Value::Null => serde_json::Value::Null,
            Value::Integer(v) => serde_json::json!(v),
            Value::Double(v) => serde_json::json!(v),
            Value::Boolean(v) => serde_json::json!(v),
            v => serde_json::json!(v.lexical().unwrap_or_default()),
        };
        props.insert(p.name.local.clone(), v);
    }
    CoreFeature {
        type_: "Feature".to_string(),
        id: f
            .id
            .split_once('.')
            .map(|(_, k)| k.to_string())
            .or(Some(f.id.clone())),
        geometry,
        properties: Some(serde_json::Value::Object(props)),
        links: vec![],
    }
}

#[async_trait]
impl CollectionSource for ChCollectionSource {
    async fn items(&self, filter: &FilterParams) -> Result<ItemsResult> {
        let f = match filter.bbox() {
            Ok(Some(b)) => Some(Filter::Spatial {
                op: SpatialOp::BBox,
                property: None,
                operand: SpatialOperand::Geometry(GeometryLiteral {
                    geometry: geo::Geometry::Polygon(
                        geo::Rect::new((b[0], b[1]), (b[2], b[3])).to_polygon(),
                    ),
                    srid: Some(4326),
                    srs_name: None,
                }),
                distance: None,
            }),
            Ok(None) => None,
            Err(_) => return Err(Error::QueryParams),
        };
        let def = self.store.feature_type();
        let f = match f {
            Some(f) => {
                Some(crate::wfs::filter::prepare(&f, &[def]).map_err(|_| Error::QueryParams)?)
            }
            None => None,
        };
        let number_matched = self
            .store
            .count(f.clone())
            .await
            .map_err(|e| Error::DatasourceSetupError(e.to_string()))?;
        let q = StoreQuery {
            filter: f,
            offset: filter.offset.unwrap_or(0) as u64,
            limit: Some(filter.limit_or_default() as u64),
            ..Default::default()
        };
        let features: Vec<Feature> = self
            .store
            .query(q)
            .try_collect()
            .await
            .map_err(|e| Error::DatasourceSetupError(e.to_string()))?;
        Ok(ItemsResult {
            number_returned: features.len() as u64,
            number_matched,
            features: features
                .iter()
                .map(|f| core_feature(&self.store, f))
                .collect(),
        })
    }

    async fn item(
        &self,
        _base_url: &str,
        collection_id: &str,
        feature_id: &str,
    ) -> Result<Option<CoreFeature>> {
        let f = Filter::ResourceIds(vec![ResourceId {
            rid: format!("{collection_id}.{feature_id}"),
            version: None,
            start_date: None,
            end_date: None,
        }]);
        let q = StoreQuery {
            filter: Some(f),
            limit: Some(1),
            ..Default::default()
        };
        let features: Vec<Feature> = self
            .store
            .query(q)
            .try_collect()
            .await
            .map_err(|e| Error::DatasourceSetupError(e.to_string()))?;
        Ok(features.first().map(|f| core_feature(&self.store, f)))
    }

    async fn queryables(&self, _collection_id: &str) -> Result<Option<Queryables>> {
        Ok(None)
    }

    fn wfs_source(&self) -> Option<SourceDesc> {
        Some(SourceDesc::Clickhouse {
            client: self.client.clone(),
            cfg: self.cfg.clone(),
            store: Some(self.store.clone()),
        })
    }
}
