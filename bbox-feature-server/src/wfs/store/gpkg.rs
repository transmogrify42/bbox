//! GeoPackage feature store.

use super::sql::{quote_ident, ColumnInfo, Dialect, IdMapping, SqlValue};
use super::*;
use crate::wfs::crs;
use crate::wfs::wkb::decode_gpkg_z;
use futures::TryStreamExt;
use geo::Rect;
use log::warn;
use sqlx::sqlite::{SqliteRow, SqliteValueRef};
use sqlx::{Column, Row, SqlitePool, TypeInfo, Value as _, ValueRef};

pub struct GpkgStore {
    pool: SqlitePool,
    layout: Arc<SqlLayout>,
    feature_type: Arc<FeatureTypeDef>,
}

struct TableColumn {
    name: String,
    decl_type: String,
    not_null: bool,
    pk: bool,
}

async fn table_columns(pool: &SqlitePool, table: &str) -> Result<Vec<TableColumn>> {
    let rows = sqlx::query("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(TableColumn {
                name: r.try_get("name")?,
                decl_type: r.try_get::<String, _>("type")?.to_uppercase(),
                not_null: r.try_get::<i64, _>("notnull")? != 0,
                pk: r.try_get::<i64, _>("pk")? > 0,
            })
        })
        .collect()
}

async fn query_columns(pool: &SqlitePool, sql: &str) -> Result<Vec<TableColumn>> {
    use sqlx::Executor;
    let desc = pool.describe(sql).await?;
    Ok(desc
        .columns()
        .iter()
        .map(|c| TableColumn {
            name: c.name().to_string(),
            decl_type: c.type_info().name().to_uppercase(),
            not_null: false,
            pk: false,
        })
        .collect())
}

/// Geometry columns registered in gpkg_geometry_columns: (column, type, srs_id)
async fn geometry_columns(pool: &SqlitePool, table: &str) -> Result<Vec<(String, String, i64)>> {
    let rows = sqlx::query(
        "SELECT column_name, geometry_type_name, srs_id FROM gpkg_geometry_columns WHERE table_name = ?",
    )
    .bind(table)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
        .collect()
}

async fn epsg_code(pool: &SqlitePool, srs_id: i64) -> u16 {
    let row = sqlx::query(
        "SELECT organization, organization_coordsys_id FROM gpkg_spatial_ref_sys WHERE srs_id = ?",
    )
    .bind(srs_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(r) => {
            let org: String = r.try_get(0).unwrap_or_default();
            let code: i64 = r.try_get(1).unwrap_or(srs_id);
            if org.eq_ignore_ascii_case("EPSG") && code > 0 {
                code as u16
            } else if srs_id > 0 {
                srs_id as u16
            } else {
                4326
            }
        }
        None => {
            if srs_id > 0 {
                srs_id as u16
            } else {
                4326
            }
        }
    }
}

async fn rtree_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn table_extent(pool: &SqlitePool, table: &str) -> Option<Rect<f64>> {
    let row =
        sqlx::query("SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = ?")
            .bind(table)
            .fetch_optional(pool)
            .await
            .ok()??;
    let v: (Option<f64>, Option<f64>, Option<f64>, Option<f64>) = (
        row.try_get(0).ok()?,
        row.try_get(1).ok()?,
        row.try_get(2).ok()?,
        row.try_get(3).ok()?,
    );
    match v {
        (Some(a), Some(b), Some(c), Some(d)) => Some(Rect::new((a, b), (c, d))),
        _ => None,
    }
}

/// Extent in WGS84 lon/lat
pub fn wgs84_extent(rect: Rect<f64>, srid: u16) -> Option<Rect<f64>> {
    if srid == 4326 {
        return Some(rect);
    }
    let mut g = geo::Geometry::Polygon(rect.to_polygon());
    crs::transform(&mut g, srid, 4326).ok()?;
    use geo::BoundingRect;
    g.bounding_rect()
}

fn value_type(decl: &str) -> ValueType {
    let d = decl.to_uppercase();
    if d.contains("INT") {
        ValueType::Integer
    } else if d.contains("REAL")
        || d.contains("DOUBLE")
        || d.contains("FLOAT")
        || d.contains("NUMERIC")
    {
        ValueType::Double
    } else if d.starts_with("BOOL") {
        ValueType::Boolean
    } else if d == "DATE" {
        ValueType::Date
    } else if d.starts_with("DATETIME") || d.starts_with("TIMESTAMP") {
        ValueType::DateTime
    } else {
        ValueType::String
    }
}

fn is_geometry_decl(decl: &str) -> bool {
    matches!(
        decl,
        "GEOMETRY"
            | "POINT"
            | "LINESTRING"
            | "POLYGON"
            | "MULTIPOINT"
            | "MULTILINESTRING"
            | "MULTIPOLYGON"
            | "GEOMETRYCOLLECTION"
            | "CURVE"
            | "SURFACE"
            | "MULTICURVE"
            | "MULTISURFACE"
    )
}

impl GpkgStore {
    pub async fn create(
        pool: SqlitePool,
        table: Option<String>,
        sql: String,
        pk: Option<String>,
        name: QName,
        title: Option<String>,
        mapping: Option<SchemaMapping>,
    ) -> Result<Self> {
        let (columns, geoms) = match &table {
            Some(t) => (
                table_columns(&pool, t).await?,
                geometry_columns(&pool, t).await?,
            ),
            None => (query_columns(&pool, &sql).await?, Vec::new()),
        };
        let has_column = |n: &str| columns.iter().any(|c| c.name == n);
        let pk = pk
            .or_else(|| columns.iter().find(|c| c.pk).map(|c| c.name.clone()))
            .filter(|p| has_column(p));
        // native CRS of first registered geometry column
        let srid = match geoms.first() {
            Some((_, _, srs_id)) => epsg_code(&pool, *srs_id).await,
            None => 4326,
        };
        let geom_type = |col: &str| -> Option<GeomType> {
            geoms
                .iter()
                .find(|(c, _, _)| c == col)
                .map(|(_, t, _)| GeomType::from_sf_name(t))
                .or_else(|| {
                    columns
                        .iter()
                        .find(|c| c.name == col && is_geometry_decl(&c.decl_type))
                        .map(|c| GeomType::from_sf_name(&c.decl_type))
                })
        };
        let mut feature_type = match mapping {
            Some(m) => m.feature_type,
            None => {
                let mut props = standard_gml_properties();
                for c in &columns {
                    if Some(&c.name) == pk.as_ref()
                        || c.name == "gml_id"
                        || c.name.ends_with("_gmlid")
                        || c.name.ends_with("_gmlmeta")
                        || c.name.ends_with("_gmlxml")
                        || props.iter().any(|p| p.column == c.name)
                    {
                        continue;
                    }
                    let vt = match geom_type(&c.name) {
                        Some(g) => ValueType::Geometry(g),
                        None => value_type(&c.decl_type),
                    };
                    if map_gml_column(&mut props, &c.name, &vt) {
                        continue;
                    }
                    let mut p = PropertyDef::simple(
                        QName::new(&name.ns, &name.prefix, &c.name),
                        &c.name,
                        vt,
                    );
                    if c.not_null {
                        p.min_occurs = 1;
                        p.nillable = false;
                    }
                    props.push(p);
                }
                FeatureTypeDef {
                    name: name.clone(),
                    title: None,
                    abstract_: None,
                    keywords: vec![],
                    properties: props,
                    srid,
                    wgs84_bbox: None,
                    schema_xsd: None,
                    supertypes: vec![],
                }
            }
        };
        feature_type.name = QName::new(
            &feature_type.name.ns,
            &feature_type.name.prefix,
            &name.local,
        );
        feature_type.srid = srid;
        if feature_type.title.is_none() {
            feature_type.title = title;
        }
        if let Some(t) = &table {
            if let Some(ext) = table_extent(&pool, t).await {
                feature_type.wgs84_bbox = wgs84_extent(ext, srid);
            }
        }
        let mut col_infos = Vec::new();
        let mut gid_cols = Vec::new();
        let mut meta_cols = Vec::new();
        let mut xml_cols = Vec::new();
        for p in &feature_type.properties {
            if !has_column(&p.column) {
                if p.min_occurs > 0 {
                    warn!("{}: column `{}` missing", name.prefixed(), p.column);
                }
                col_infos.push(None);
                gid_cols.push(None);
                meta_cols.push(None);
                xml_cols.push(None);
                continue;
            }
            let rtree = match (&table, p.is_geometry()) {
                (Some(t), true) => {
                    let rt = format!("rtree_{t}_{}", p.column);
                    rtree_exists(&pool, &rt).await.then_some(rt)
                }
                _ => None,
            };
            let decl = columns
                .iter()
                .find(|c| c.name == p.column)
                .map(|c| c.decl_type.clone())
                .unwrap_or_default();
            col_infos.push(Some(ColumnInfo {
                db_type: decl,
                rtree,
                coords: None,
            }));
            let gid = format!("{}_gmlid", p.column);
            gid_cols.push(has_column(&gid).then_some(gid));
            let meta = format!("{}_gmlmeta", p.column);
            meta_cols.push(has_column(&meta).then_some(meta));
            let gx = format!("{}_gmlxml", p.column);
            xml_cols.push(has_column(&gx).then_some(gx));
        }
        let key_column = pk
            .clone()
            .or_else(|| table.as_ref().map(|_| "rowid".to_string()));
        let id = match (&key_column, has_column("gml_id")) {
            (Some(k), true) => IdMapping::GmlId {
                column: "gml_id".to_string(),
                pk: k.clone(),
            },
            (Some(k), false) => IdMapping::Pk {
                column: k.clone(),
                db_type: "INTEGER".to_string(),
            },
            (None, _) => IdMapping::None,
        };
        let from = match &table {
            Some(t) => quote_ident(t),
            None => format!("({sql}) AS t"),
        };
        let layout = SqlLayout {
            dialect: Dialect::Sqlite,
            feature_type: feature_type.clone(),
            from,
            columns: col_infos,
            id,
            key_column,
            geometry_id_columns: gid_cols,
            geometry_meta_columns: meta_cols,
            geometry_xml_columns: xml_cols,
        };
        Ok(GpkgStore {
            pool,
            layout: Arc::new(layout),
            feature_type: Arc::new(feature_type),
        })
    }
}

fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    binds: &[SqlValue],
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    for b in binds {
        q = match b {
            SqlValue::Text(t) => q.bind(t.clone()),
            SqlValue::Int(i) => q.bind(*i),
            SqlValue::Float(f) => q.bind(*f),
            SqlValue::Bool(b) => q.bind(*b),
            SqlValue::Bytes(v) => q.bind(v.clone()),
            SqlValue::TextArray(a) => q.bind(a.join(",")),
        };
    }
    q
}

fn raw_text(v: &SqliteValueRef) -> Option<String> {
    let owned = ValueRef::to_owned(v);
    match v.type_info().name() {
        "INTEGER" => owned.try_decode::<i64>().ok().map(|i| i.to_string()),
        "REAL" => owned.try_decode::<f64>().ok().map(format_double),
        "BLOB" => None,
        _ => owned.try_decode::<String>().ok(),
    }
}

fn decode_value(
    row: &SqliteRow,
    prop: &PropertyDef,
    col: &str,
    gml_id: Option<String>,
    meta: Option<String>,
) -> Result<Value> {
    let raw = row.try_get_raw(col)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    if prop.is_xml() {
        return Ok(raw_text(&raw).map(Value::Xml).unwrap_or(Value::Null));
    }
    let storage = raw.type_info().name().to_string();
    Ok(match &prop.value_type {
        ValueType::Geometry(_) => {
            let blob: Vec<u8> = row.try_get(col)?;
            match decode_gpkg_z(&blob).map_err(|e| StoreError::Data(e.to_string()))? {
                Some((geometry, z)) => Value::Geometry(GeomValue {
                    geometry,
                    gml_id,
                    meta,
                    z,
                }),
                None => Value::Null,
            }
        }
        ValueType::Integer if storage == "INTEGER" => Value::Integer(row.try_get(col)?),
        ValueType::Integer => match raw_text(&raw) {
            Some(t) => t
                .trim()
                .parse()
                .map(Value::Integer)
                .unwrap_or(Value::String(t)),
            None => Value::Null,
        },
        ValueType::Double if storage == "REAL" || storage == "INTEGER" => {
            Value::Double(row.try_get::<f64, _>(col)?)
        }
        ValueType::Decimal => match raw_text(&raw) {
            Some(t) => Value::Decimal(t),
            None => Value::Null,
        },
        ValueType::Boolean => match storage.as_str() {
            "INTEGER" => Value::Boolean(row.try_get::<i64, _>(col)? != 0),
            _ => match raw_text(&raw).as_deref() {
                Some("true") | Some("1") => Value::Boolean(true),
                Some(_) => Value::Boolean(false),
                None => Value::Null,
            },
        },
        ValueType::Date => raw_text(&raw).map(Value::Date).unwrap_or(Value::Null),
        ValueType::DateTime => raw_text(&raw).map(Value::DateTime).unwrap_or(Value::Null),
        ValueType::Time => raw_text(&raw).map(Value::Time).unwrap_or(Value::Null),
        ValueType::Double => match raw_text(&raw) {
            Some(t) => t
                .trim()
                .parse()
                .map(Value::Double)
                .unwrap_or(Value::String(t)),
            None => Value::Null,
        },
        _ => raw_text(&raw).map(Value::String).unwrap_or(Value::Null),
    })
}

fn decode_row(
    row: &SqliteRow,
    layout: &SqlLayout,
    props: &[usize],
    rownum: u64,
) -> Result<Feature> {
    let ft = &layout.feature_type;
    let key = if layout.key_column.is_some() {
        let raw = row.try_get_raw("__key")?;
        raw_text(&raw)
    } else {
        Some(rownum.to_string())
    };
    let gml_id = match &layout.id {
        IdMapping::GmlId { .. } => row.try_get::<Option<String>, _>("__gml_id")?,
        _ => None,
    };
    let mut values = vec![Value::Null; ft.properties.len()];
    for &i in props {
        let prop = &ft.properties[i];
        let gid = if layout.geometry_id_columns[i].is_some() {
            row.try_get::<Option<String>, _>(format!("__gid_{i}").as_str())?
        } else {
            None
        };
        if layout.geometry_xml_columns[i].is_some() {
            if let Some(raw) = row.try_get::<Option<String>, _>(format!("__gxml_{i}").as_str())? {
                values[i] = Value::Xml(raw);
                continue;
            }
        }
        let meta = if layout.geometry_meta_columns[i].is_some() {
            row.try_get::<Option<String>, _>(format!("__gmeta_{i}").as_str())?
        } else {
            None
        };
        values[i] = decode_value(row, prop, &prop.column, gid, meta)?;
    }
    Ok(Feature {
        id: layout.feature_id(key, gml_id),
        values,
    })
}

#[async_trait]
impl FeatureStore for GpkgStore {
    fn feature_type(&self) -> &FeatureTypeDef {
        &self.feature_type
    }

    fn plan(&self, q: &StoreQuery) -> QueryPlan {
        self.layout.plan(q)
    }

    async fn find_geometry(&self, gml_id: &str) -> Result<Option<GeomValue>> {
        for (i, sql) in self.layout.geometry_lookup_sql() {
            let prop = &self.layout.feature_type.properties[i];
            if let Some(row) = sqlx::query(&sql)
                .bind(gml_id)
                .fetch_optional(&self.pool)
                .await?
            {
                let gid: Option<String> = row.try_get("__gid")?;
                let meta: Option<String> = row.try_get("__gmeta")?;
                if let Value::Geometry(g) = decode_value(&row, prop, &prop.column, gid, meta)? {
                    return Ok(Some(g));
                }
            }
        }
        Ok(None)
    }

    fn query(&self, q: StoreQuery) -> FeatureStream {
        let plan = self.layout.plan(&q);
        let props = self.layout.loaded_properties(&q);
        let pool = self.pool.clone();
        let layout = self.layout.clone();
        let sql = plan.sql.clone();
        let binds = plan.binds.clone();
        let offset = if plan.paging_in_sql { q.offset } else { 0 };
        let stream = async_stream::try_stream! {
            let query = bind_all(sqlx::query(&sql), &binds);
            let mut rows = query.fetch(&pool);
            let mut n = offset;
            while let Some(row) = rows.try_next().await? {
                n += 1;
                yield decode_row(&row, &layout, &props, n)?;
            }
        };
        post_process(stream.boxed(), self.feature_type.clone(), &plan, &q)
    }

    async fn count(&self, filter: Option<Filter>) -> Result<u64> {
        let plan = self.layout.count_plan(filter.as_ref());
        if plan.residual.is_some() {
            let q = StoreQuery {
                filter,
                ..Default::default()
            };
            let n = self
                .query(q)
                .try_fold(0u64, |n, _| async move { Ok(n + 1) })
                .await?;
            return Ok(n);
        }
        let row = bind_all(sqlx::query(&plan.sql), &plan.binds)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<i64, _>(0)? as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfs::filter::{parse_filter_str, prepare, FilterVersion, ParseContext};
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn store(table: &str) -> GpkgStore {
        let opts = SqliteConnectOptions::new()
            .filename("../assets/ne_extracts.gpkg")
            .read_only(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        GpkgStore::create(
            pool,
            Some(table.to_string()),
            format!("SELECT * FROM {table}"),
            None,
            QName::new("http://ne", "ne", "places"),
            Some("Places".to_string()),
            None,
        )
        .await
        .unwrap()
    }

    fn filter(store: &GpkgStore, body: &str) -> Filter {
        let xml = format!(
            r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:ne="http://ne">{body}</fes:Filter>"#
        );
        let f = parse_filter_str(&xml, &ParseContext::new(FilterVersion::V200)).unwrap();
        prepare(&f, &[store.feature_type()]).unwrap()
    }

    #[tokio::test]
    async fn auto_mapped_type() {
        let s = store("ne_10m_populated_places").await;
        let ft = s.feature_type();
        assert_eq!(ft.srid, 4326);
        assert_eq!(ft.name.prefixed(), "ne:places");
        let (_, geom) = ft.default_geometry().unwrap();
        assert_eq!(geom.value_type, ValueType::Geometry(GeomType::Point));
        assert!(
            ft.property_by_local("fid").is_none(),
            "pk is the id, not a property"
        );
        assert!(ft.wgs84_bbox.is_some());
        assert_eq!(s.count(None).await.unwrap(), 7342);
    }

    #[tokio::test]
    async fn stream_with_pushdown_and_paging() {
        let s = store("ne_10m_populated_places").await;
        let f = filter(
            &s,
            r#"<fes:PropertyIsEqualTo><fes:ValueReference>ne:NAME</fes:ValueReference><fes:Literal>Bern</fes:Literal></fes:PropertyIsEqualTo>"#,
        );
        let q = StoreQuery {
            filter: Some(f.clone()),
            ..Default::default()
        };
        let plan = s.plan(&q);
        assert!(plan.sql.contains(r#"WHERE "NAME" = ?"#), "{}", plan.sql);
        assert_eq!(plan.residual, None);
        let features: Vec<Feature> = s.query(q).try_collect().await.unwrap();
        assert_eq!(features.len(), 1);
        assert!(features[0].id.starts_with("places."));
        assert_eq!(s.count(Some(f)).await.unwrap(), 1);
        // paging with stable order
        let q = StoreQuery {
            offset: 10,
            limit: Some(5),
            ..Default::default()
        };
        let plan = s.plan(&q);
        assert!(plan.paging_in_sql);
        assert!(
            plan.sql
                .ends_with(r#"ORDER BY "fid" ASC LIMIT 5 OFFSET 10"#),
            "{}",
            plan.sql
        );
        let page: Vec<Feature> = s.query(q).try_collect().await.unwrap();
        assert_eq!(page.len(), 5);
        assert_eq!(page[0].id, "places.11");
    }

    #[tokio::test]
    async fn bbox_uses_rtree_and_exact_residual() {
        let s = store("ne_10m_populated_places").await;
        let f = filter(
            &s,
            r#"<fes:BBOX><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>45.8 5.9</gml:lowerCorner><gml:upperCorner>47.8 10.5</gml:upperCorner></gml:Envelope></fes:BBOX>"#,
        );
        let q = StoreQuery {
            filter: Some(f.clone()),
            limit: Some(1000),
            ..Default::default()
        };
        let plan = s.plan(&q);
        assert!(
            plan.sql.contains("rtree_ne_10m_populated_places_geom"),
            "{}",
            plan.sql
        );
        assert!(plan.residual.is_some());
        assert!(!plan.paging_in_sql);
        let features: Vec<Feature> = s.query(q).try_collect().await.unwrap();
        let n = features.len();
        assert!(n > 10 && n < 200, "{n}");
        assert_eq!(s.count(Some(f)).await.unwrap(), n as u64);
    }

    #[tokio::test]
    async fn residual_paging_is_applied_after_filter() {
        let s = store("ne_10m_populated_places").await;
        let f = filter(
            &s,
            r#"<fes:PropertyIsEqualTo><fes:Function name="strToUpperCase"><fes:ValueReference>ne:NAME</fes:ValueReference></fes:Function><fes:Literal>BERN</fes:Literal></fes:PropertyIsEqualTo>"#,
        );
        let q = StoreQuery {
            filter: Some(f),
            limit: Some(10),
            ..Default::default()
        };
        assert!(!s.plan(&q).paging_in_sql);
        let features: Vec<Feature> = s.query(q).try_collect().await.unwrap();
        assert_eq!(features.len(), 1);
    }
}
