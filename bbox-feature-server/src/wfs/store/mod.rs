//! Feature stores: streaming, read-only access to feature types with SQL pushdown.

pub mod clickhouse;
pub mod gpkg;
pub mod postgis;
pub mod sql;

use crate::wfs::filter::{self, EvalContext, FeatureSource, Filter};
use crate::wfs::model::*;
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use sql::{quote_ident, render_placeholders, Dialect, SqlExpr, SqlValue, Translation};
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("filter error: {0}")]
    Filter(#[from] filter::FilterError),
    #[error("invalid data: {0}")]
    Data(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Sort target
#[derive(Clone, Debug, PartialEq)]
pub enum SortTarget {
    Property(usize),
    Id,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortKey {
    pub target: SortTarget,
    pub descending: bool,
}

/// Query on a single feature type
#[derive(Clone, Debug, Default)]
pub struct StoreQuery {
    /// Prepared filter (literals in native CRS)
    pub filter: Option<Filter>,
    pub sort: Vec<SortKey>,
    pub offset: u64,
    pub limit: Option<u64>,
    /// Properties to load (None: all). Other values are returned as Null.
    pub properties: Option<Vec<usize>>,
}

pub type FeatureStream = BoxStream<'static, Result<Feature>>;

/// Description of the planned query (for tests and debugging)
#[derive(Clone, Debug, PartialEq)]
pub struct QueryPlan {
    pub sql: String,
    pub binds: Vec<SqlValue>,
    pub residual: Option<Filter>,
    /// Paging applied in SQL
    pub paging_in_sql: bool,
}

/// Read-only feature store
#[async_trait]
pub trait FeatureStore: Send + Sync {
    fn feature_type(&self) -> &FeatureTypeDef;
    /// Stream features matching the query
    fn query(&self, q: StoreQuery) -> FeatureStream;
    /// Number of features matching the filter
    async fn count(&self, filter: Option<Filter>) -> Result<u64>;
    /// Query plan for a query
    fn plan(&self, q: &StoreQuery) -> QueryPlan;
    /// Geometry with the given gml:id (stored in companion gml:id columns)
    async fn find_geometry(&self, _gml_id: &str) -> Result<Option<GeomValue>> {
        Ok(None)
    }
}

/// Mapping of a feature type to a SQL table or query
#[derive(Clone, Debug)]
pub struct SqlLayout {
    pub dialect: Dialect,
    pub feature_type: FeatureTypeDef,
    /// FROM clause (quoted table or `(query) AS t`)
    pub from: String,
    /// Column info per property (None: not stored)
    pub columns: Vec<Option<sql::ColumnInfo>>,
    pub id: sql::IdMapping,
    /// Primary key / rowid column for stable ordering and rtree lookups
    pub key_column: Option<String>,
    /// Companion gml:id columns of geometry properties
    pub geometry_id_columns: Vec<Option<String>>,
    /// Companion GML metadata columns of geometry properties
    pub geometry_meta_columns: Vec<Option<String>>,
    /// Companion columns with verbatim GML of geometry properties (e.g. containing xlinks)
    pub geometry_xml_columns: Vec<Option<String>>,
}

impl SqlLayout {
    fn ctx(&self) -> sql::SqlContext<'_> {
        sql::SqlContext {
            dialect: self.dialect,
            feature_type: &self.feature_type,
            columns: &self.columns,
            id: &self.id,
            rowid_column: self.key_column.as_deref(),
        }
    }

    pub fn translate(&self, filter: Option<&Filter>) -> Translation {
        match filter {
            Some(f) => sql::translate(f, &self.ctx()),
            None => Translation {
                sql: None,
                residual: None,
            },
        }
    }

    /// Expression selecting a property column
    fn select_expr(&self, idx: usize) -> Option<String> {
        let prop = &self.feature_type.properties[idx];
        self.columns[idx].as_ref()?;
        let col = quote_ident(&prop.column);
        Some(match (self.dialect, &prop.value_type) {
            (Dialect::Postgres, ValueType::Geometry(_)) if !prop.is_xml() => {
                format!("ST_AsBinary({col}) AS {col}")
            }
            (Dialect::Postgres, ValueType::Decimal)
            | (Dialect::Postgres, ValueType::Date)
            | (Dialect::Postgres, ValueType::Time)
            | (Dialect::Postgres, ValueType::String)
            | (Dialect::Postgres, ValueType::Uri)
            | (Dialect::Postgres, ValueType::Complex) => format!("{col}::text AS {col}"),
            (Dialect::Postgres, ValueType::DateTime) => format!("to_json({col})#>>'{{}}' AS {col}"),
            _ => col,
        })
    }

    /// Columns loaded for a query
    pub fn loaded_properties(&self, q: &StoreQuery) -> Vec<usize> {
        let n = self.feature_type.properties.len();
        let mut props: Vec<usize> = match &q.properties {
            Some(p) => p.clone(),
            None => (0..n).collect(),
        };
        // residual filter and sorting may need other properties
        if q.properties.is_some() {
            let needed: Vec<usize> = (0..n).collect();
            let translation = self.translate(q.filter.as_ref());
            if let Some(residual) = &translation.residual {
                for p in residual.properties() {
                    if let Some((i, _, _)) = filter::resolve_property(&self.feature_type, p) {
                        if !props.contains(&i) {
                            props.push(i);
                        }
                    }
                }
            }
            for s in &q.sort {
                if let SortTarget::Property(i) = s.target {
                    if !props.contains(&i) && needed.contains(&i) {
                        props.push(i);
                    }
                }
            }
        }
        props.sort();
        props.dedup();
        props.retain(|i| self.columns[*i].is_some());
        props
    }

    fn select_list(&self, props: &[usize]) -> String {
        let mut cols = Vec::new();
        if let Some(key) = &self.key_column {
            match self.dialect {
                Dialect::Postgres => cols.push(format!("{}::text AS __key", quote_ident(key))),
                _ => cols.push(format!("{} AS __key", quote_ident(key))),
            }
        }
        if let sql::IdMapping::GmlId { column, .. } = &self.id {
            cols.push(format!("{} AS __gml_id", quote_ident(column)));
        }
        for i in props {
            if let Some(e) = self.select_expr(*i) {
                cols.push(e);
            }
            if let Some(Some(gid)) = self.geometry_id_columns.get(*i) {
                cols.push(format!(
                    "{} AS {}",
                    quote_ident(gid),
                    quote_ident(&format!("__gid_{i}"))
                ));
            }
            if let Some(Some(meta)) = self.geometry_meta_columns.get(*i) {
                cols.push(format!(
                    "{} AS {}",
                    quote_ident(meta),
                    quote_ident(&format!("__gmeta_{i}"))
                ));
            }
            if let Some(Some(gx)) = self.geometry_xml_columns.get(*i) {
                cols.push(format!(
                    "{} AS {}",
                    quote_ident(gx),
                    quote_ident(&format!("__gxml_{i}"))
                ));
            }
        }
        if cols.is_empty() {
            cols.push("1".to_string());
        }
        cols.join(", ")
    }

    fn order_by(&self, sort: &[SortKey], paging: bool) -> Option<String> {
        let mut keys = Vec::new();
        for s in sort {
            let col = match &s.target {
                SortTarget::Property(i) => {
                    self.columns[*i].as_ref()?;
                    let prop = &self.feature_type.properties[*i];
                    if prop.is_xml() || prop.is_geometry() {
                        return None;
                    }
                    quote_ident(&prop.column)
                }
                SortTarget::Id => quote_ident(self.key_column.as_ref()?),
            };
            keys.push(format!(
                "{col} {}",
                if s.descending { "DESC" } else { "ASC" }
            ));
        }
        // stable order for paging
        if paging {
            if let Some(key) = &self.key_column {
                let k = format!("{} ASC", quote_ident(key));
                if !keys.iter().any(|x| x.starts_with(&quote_ident(key))) {
                    keys.push(k);
                }
            }
        }
        if keys.is_empty() {
            Some(String::new())
        } else {
            Some(format!(" ORDER BY {}", keys.join(", ")))
        }
    }

    /// Build SQL for a query
    pub fn plan(&self, q: &StoreQuery) -> QueryPlan {
        let translation = self.translate(q.filter.as_ref());
        let props = self.loaded_properties(q);
        let paging = q.offset > 0 || q.limit.is_some();
        let order = self.order_by(&q.sort, paging);
        let sort_in_sql = order.is_some();
        let paging_in_sql = translation.residual.is_none() && sort_in_sql;
        let mut sql = format!("SELECT {} FROM {}", self.select_list(&props), self.from);
        let mut binds = Vec::new();
        if let Some(SqlExpr {
            sql: where_sql,
            binds: b,
        }) = &translation.sql
        {
            sql.push_str(" WHERE ");
            sql.push_str(where_sql);
            binds.extend(b.iter().cloned());
        }
        if let Some(order) = &order {
            sql.push_str(order);
        }
        if paging_in_sql {
            if let Some(limit) = q.limit {
                sql.push_str(&format!(" LIMIT {limit}"));
            }
            if q.offset > 0 {
                if q.limit.is_none() && self.dialect == Dialect::Sqlite {
                    sql.push_str(" LIMIT -1");
                }
                sql.push_str(&format!(" OFFSET {}", q.offset));
            }
        }
        QueryPlan {
            sql: render_placeholders(&sql, self.dialect, 1),
            binds,
            residual: translation.residual,
            paging_in_sql,
        }
    }

    pub fn count_plan(&self, filter: Option<&Filter>) -> QueryPlan {
        let translation = self.translate(filter);
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.from);
        let mut binds = Vec::new();
        if let Some(e) = &translation.sql {
            sql.push_str(" WHERE ");
            sql.push_str(&e.sql);
            binds.extend(e.binds.iter().cloned());
        }
        QueryPlan {
            sql: render_placeholders(&sql, self.dialect, 1),
            binds,
            residual: translation.residual,
            paging_in_sql: false,
        }
    }

    /// SQL selecting a geometry by its gml:id: (property index, sql)
    pub fn geometry_lookup_sql(&self) -> Vec<(usize, String)> {
        let mut result = Vec::new();
        for (i, gid) in self.geometry_id_columns.iter().enumerate() {
            let Some(gid) = gid else { continue };
            let Some(geom_expr) = self.select_expr(i) else {
                continue;
            };
            let meta = match self.geometry_meta_columns.get(i).cloned().flatten() {
                Some(m) => quote_ident(&m),
                None => "NULL".to_string(),
            };
            let sql = format!(
                "SELECT {geom_expr}, {} AS __gid, {meta} AS __gmeta FROM {} WHERE {} = ? LIMIT 1",
                quote_ident(gid),
                self.from,
                quote_ident(gid)
            );
            result.push((i, render_placeholders(&sql, self.dialect, 1)));
        }
        result
    }

    /// Feature id from key / gml:id column values
    pub fn feature_id(&self, key: Option<String>, gml_id: Option<String>) -> String {
        if let Some(id) = gml_id {
            return id;
        }
        match key {
            Some(k) => format!("{}.{k}", self.feature_type.name.local),
            None => format!("{}.0", self.feature_type.name.local),
        }
    }
}

/// Apply residual filter, sorting and paging not handled in SQL to a feature stream
pub fn post_process(
    stream: FeatureStream,
    feature_type: Arc<FeatureTypeDef>,
    plan: &QueryPlan,
    q: &StoreQuery,
) -> FeatureStream {
    let residual = plan.residual.clone();
    let srid = feature_type.srid;
    let ft = feature_type.clone();
    let mut stream = stream;
    if let Some(residual) = residual {
        stream = stream
            .filter_map(move |r| {
                let residual = residual.clone();
                let ft = ft.clone();
                async move {
                    match r {
                        Ok(f) => {
                            let src = FeatureSource {
                                feature_type: &ft,
                                feature: &f,
                            };
                            match filter::evaluate(&residual, &src, &EvalContext { srid }) {
                                Ok(true) => Some(Ok(f)),
                                Ok(false) => None,
                                Err(e) => Some(Err(e.into())),
                            }
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
            })
            .boxed();
    }
    if !plan.paging_in_sql {
        let offset = q.offset as usize;
        stream = stream.skip(offset).boxed();
        if let Some(limit) = q.limit {
            stream = stream.take(limit as usize).boxed();
        }
    }
    stream
}

/// Source of a collection for the WFS layer
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum SourceDesc {
    Gpkg {
        pool: sqlx::SqlitePool,
        table: Option<String>,
        sql: String,
        pk: Option<String>,
    },
    Postgis {
        pool: sqlx::PgPool,
        schema: Option<String>,
        table: Option<String>,
        sql: String,
        pk: Option<String>,
    },
    Clickhouse {
        client: clickhouse::ChClient,
        cfg: crate::config::ClickhouseCollectionCfg,
        /// Store already set up for the collection (reused without schema mapping)
        store: Option<Arc<clickhouse::ChStore>>,
    },
}

/// Feature type mapping from configuration (application schema)
#[derive(Clone, Debug)]
pub struct SchemaMapping {
    pub feature_type: FeatureTypeDef,
}

/// Create a store for a collection
pub async fn create_store(
    desc: SourceDesc,
    name: QName,
    title: Option<String>,
    mapping: Option<SchemaMapping>,
) -> Result<Arc<dyn FeatureStore>> {
    match desc {
        SourceDesc::Gpkg {
            pool,
            table,
            sql,
            pk,
        } => Ok(Arc::new(
            gpkg::GpkgStore::create(pool, table, sql, pk, name, title, mapping).await?,
        )),
        SourceDesc::Postgis {
            pool,
            schema,
            table,
            sql,
            pk,
        } => Ok(Arc::new(
            postgis::PgStore::create(pool, schema, table, sql, pk, name, title, mapping).await?,
        )),
        SourceDesc::Clickhouse {
            store: Some(store), ..
        } if mapping.is_none() => Ok(Arc::new(store.renamed(name, title))),
        SourceDesc::Clickhouse { client, cfg, .. } => Ok(Arc::new(
            clickhouse::ChStore::create(client, &cfg, name, title, mapping).await?,
        )),
    }
}
