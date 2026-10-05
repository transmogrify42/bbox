use bbox_core::config::{from_config_root_or_exit, ConfigError, DsPostgisCfg, NamedDatasourceCfg};
use bbox_core::service::ServiceConfig;
use clap::ArgMatches;
use serde::Deserialize;

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct FeatureServiceCfg {
    #[serde(rename = "datasource")]
    pub datasources: Vec<NamedDatasourceCfg>,
    #[serde(rename = "collections")]
    pub auto_collections: CollectionsCfg,
    #[serde(rename = "collection")]
    pub collections: Vec<ConfiguredCollectionCfg>,
    /// WFS service settings
    pub wfs: WfsCfg,
}

/// WFS service configuration
#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct WfsCfg {
    /// Enable WFS endpoint `/wfs`
    pub enabled: bool,
    pub title: String,
    #[serde(rename = "abstract")]
    pub abstract_: Option<String>,
    pub keywords: Vec<String>,
    pub fees: String,
    pub access_constraints: String,
    pub provider: WfsProviderCfg,
    /// Namespace prefix of feature types without explicit namespace
    pub namespace_prefix: String,
    /// Namespace URI of feature types without explicit namespace
    pub namespace_uri: String,
    /// Namespaces with optional GML application schema
    #[serde(rename = "namespace")]
    pub namespaces: Vec<WfsNamespaceCfg>,
    /// Default number of features returned (CountDefault). None: unlimited
    pub count_default: Option<u64>,
    /// Additional CRS (EPSG codes) offered for output
    pub other_crs: Vec<u16>,
    /// Allow resolving xlinks to remote resources
    pub remote_resolve: bool,
    /// Default resolve timeout in seconds
    pub resolve_timeout: u64,
    /// Response and count caching
    pub cache: WfsCacheCfg,
    /// Strict OGC CITE compliance (GeoServer `citeCompliant`): require the SERVICE parameter.
    /// Otherwise a missing SERVICE defaults to WFS.
    pub cite_compliant: bool,
    /// Bounding boxes of the feature collection and of each feature in GML output
    /// (GeoServer `featureBounding`). Requires an additional pass over the result.
    pub feature_bounding: bool,
}

/// WFS caching. Cached responses do not reflect data changes until they expire.
#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct WfsCacheCfg {
    /// Time to live of cached responses in seconds (0: response cache disabled)
    pub response_ttl: u64,
    /// Maximum size of a cached response in bytes (larger responses are streamed uncached)
    pub max_response_bytes: usize,
    /// Total size of cached responses in bytes
    pub capacity_bytes: usize,
    /// Collections whose responses are cached (empty: all)
    pub collections: Vec<String>,
    /// Time to live of cached feature counts (numberMatched) in seconds (0: disabled)
    pub count_ttl: u64,
    /// Cache-Control max-age in seconds for capabilities, schemas and cached responses
    pub max_age: Option<u64>,
}

impl Default for WfsCacheCfg {
    fn default() -> Self {
        WfsCacheCfg {
            response_ttl: 0,
            max_response_bytes: 1024 * 1024,
            capacity_bytes: 64 * 1024 * 1024,
            collections: Vec::new(),
            count_ttl: 0,
            max_age: None,
        }
    }
}

impl Default for WfsCfg {
    fn default() -> Self {
        WfsCfg {
            enabled: true,
            title: "BBOX Web Feature Service".to_string(),
            abstract_: None,
            keywords: Vec::new(),
            fees: "NONE".to_string(),
            access_constraints: "NONE".to_string(),
            provider: WfsProviderCfg::default(),
            namespace_prefix: "bbox".to_string(),
            namespace_uri: "https://www.bbox.earth/wfs".to_string(),
            namespaces: Vec::new(),
            count_default: None,
            other_crs: vec![4326, 3857],
            remote_resolve: true,
            resolve_timeout: 300,
            cache: WfsCacheCfg::default(),
            cite_compliant: false,
            feature_bounding: false,
        }
    }
}

/// Service provider contact information
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct WfsProviderCfg {
    pub name: Option<String>,
    pub site: Option<String>,
    pub individual_name: Option<String>,
    pub position_name: Option<String>,
    pub phone: Option<String>,
    pub fax: Option<String>,
    pub email: Option<String>,
    pub delivery_point: Option<String>,
    pub city: Option<String>,
    pub administrative_area: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
    pub hours_of_service: Option<String>,
    pub contact_instructions: Option<String>,
    pub role: Option<String>,
}

/// Feature type namespace
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WfsNamespaceCfg {
    pub prefix: String,
    pub uri: String,
    /// GML application schema (XSD file) describing the feature types of this namespace
    pub schema: Option<String>,
    /// Collections in this namespace. Default: collections matching a feature type of the schema
    #[serde(default)]
    pub collections: Vec<String>,
}

/// Collections with auto-detection
#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct CollectionsCfg {
    pub directory: Vec<DsFiledirCfg>,
    pub postgis: Vec<DsPostgisCfg>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DsFiledirCfg {
    pub dir: String,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredCollectionCfg {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    // extent: Option<CoreExtent>
    #[serde(flatten)]
    pub source: CollectionSourceCfg,
}

/// Collections with configuration
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub enum CollectionSourceCfg {
    #[serde(rename = "postgis")]
    Postgis(PostgisCollectionCfg),
    #[serde(rename = "gpkg")]
    Gpkg(GpkgCollectionCfg),
    #[serde(rename = "clickhouse")]
    Clickhouse(ClickhouseCollectionCfg),
}

/// ClickHouse table or query
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ClickhouseCollectionCfg {
    /// Name of datasource.clickhouse config (Default: first with matching type)
    pub datasource: Option<String>,
    pub table_name: Option<String>,
    /// Custom SQL query
    pub sql: Option<String>,
    pub fid_field: Option<String>,
    /// Geometry column (native geo type, WKB or WKT string)
    pub geometry_field: Option<String>,
    /// Geometry encoding of String columns: `wkb` or `wkt` (default: auto-detect)
    pub geometry_format: Option<String>,
    /// Point geometry from coordinate columns
    pub lon_field: Option<String>,
    pub lat_field: Option<String>,
    /// EPSG code of geometries (default 4326)
    pub srid: Option<u16>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PostgisCollectionCfg {
    /// Name of datasource.postgis config (Default: first with matching type)
    pub datasource: Option<String>,
    // maybe we should allow direct DS URLs?
    // pub url: Option<String>,
    pub table_schema: Option<String>,
    pub table_name: Option<String>,
    /// Custom SQL query
    pub sql: Option<String>,
    pub fid_field: Option<String>,
    pub geometry_field: Option<String>,
    //pub field_list: Option<Vec<String>>,
    /// Field used for temporal filter expressions
    pub temporal_field: Option<String>,
    /// Field used for temporal end filter expressions
    pub temporal_end_field: Option<String>,
    /// Fields which can be used in filter expressions
    #[serde(default)]
    pub queryable_fields: Vec<String>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub struct GpkgCollectionCfg {
    /// Name of datasource.gpkg config (Default: first with matching type)
    pub datasource: Option<String>,
    pub table_name: Option<String>,
    /// Custom SQL query
    pub sql: Option<String>,
    pub fid_field: Option<String>,
    pub geometry_field: Option<String>,
    //pub field_list: Option<Vec<String>>,
}

impl ServiceConfig for FeatureServiceCfg {
    fn initialize(_cli: &ArgMatches) -> Result<Self, ConfigError> {
        let cfg: FeatureServiceCfg = from_config_root_or_exit();
        Ok(cfg)
    }
}

impl CollectionsCfg {
    #[allow(dead_code)]
    pub fn from_path(path: &str) -> Self {
        let mut cfg = CollectionsCfg::default();
        cfg.directory.push(DsFiledirCfg {
            dir: path.to_string(),
        });
        cfg
    }
}
