//! PostGIS feature store.

use super::sql::{quote_ident, ColumnInfo, Dialect, IdMapping, SqlValue};
use super::*;
use crate::wfs::wkb::decode_wkb_z;
use futures::TryStreamExt;
use geo::Rect;
use log::warn;
use sqlx::postgres::{PgRow, PgTypeInfo};
use sqlx::{Column, Executor, PgPool, Row, TypeInfo, ValueRef};

pub struct PgStore {
    pool: PgPool,
    layout: Arc<SqlLayout>,
    feature_type: Arc<FeatureTypeDef>,
}

struct PgColumn {
    name: String,
    /// `format_type` of the column type (e.g. `bigint`, `uuid`, `geometry`)
    db_type: String,
    not_null: bool,
}

async fn describe_columns(pool: &PgPool, sql: &str) -> Result<Vec<PgColumn>> {
    let desc = pool.describe(sql).await?;
    let mut cols = Vec::new();
    for c in desc.columns() {
        let ti: &PgTypeInfo = c.type_info();
        let db_type = match ti.oid() {
            Some(oid) => {
                sqlx::query_scalar::<_, String>("SELECT format_type($1, NULL)")
                    .bind(oid)
                    .fetch_one(pool)
                    .await?
            }
            None => ti.name().to_lowercase(),
        };
        cols.push(PgColumn {
            name: c.name().to_string(),
            db_type,
            not_null: false,
        });
    }
    Ok(cols)
}

async fn not_null_columns(pool: &PgPool, schema: &str, table: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT a.attname::text FROM pg_attribute a
         JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped AND a.attnotnull",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|r| Ok(r.try_get(0)?)).collect()
}

async fn primary_key(pool: &PgPool, schema: &str, table: &str) -> Option<String> {
    let rows = sqlx::query(
        "SELECT a.attname::text FROM pg_index i
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
         JOIN pg_class c ON c.oid = i.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND c.relname = $2 AND i.indisprimary",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await
    .ok()?;
    if rows.len() == 1 {
        rows[0].try_get(0).ok()
    } else {
        None
    }
}

/// (srid, geometry type) of a geometry column
async fn geometry_info(
    pool: &PgPool,
    schema: Option<&str>,
    table: Option<&str>,
    column: &str,
    from: &str,
) -> (u16, GeomType) {
    if let (Some(s), Some(t)) = (schema, table) {
        let row = sqlx::query(
            "SELECT srid, type FROM geometry_columns
             WHERE f_table_schema = $1 AND f_table_name = $2 AND f_geometry_column = $3",
        )
        .bind(s)
        .bind(t)
        .bind(column)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some(r) = row {
            let srid: i32 = r.try_get(0).unwrap_or(0);
            let gtype: String = r.try_get(1).unwrap_or_default();
            if srid > 0 {
                return (srid as u16, GeomType::from_sf_name(&gtype));
            }
        }
    }
    let col = quote_ident(column);
    let sql = format!(
        "SELECT ST_SRID({col}), GeometryType({col}) FROM {from} WHERE {col} IS NOT NULL LIMIT 1"
    );
    match sqlx::query(&sql).fetch_optional(pool).await {
        Ok(Some(r)) => {
            let srid: i32 = r.try_get(0).unwrap_or(4326);
            let gtype: String = r.try_get(1).unwrap_or_default();
            (
                if srid > 0 { srid as u16 } else { 4326 },
                GeomType::Geometry,
            )
                .0
                .then_type(GeomType::from_sf_name(&gtype))
        }
        _ => (4326, GeomType::Geometry),
    }
}

trait ThenType {
    fn then_type(self, t: GeomType) -> (u16, GeomType);
}

impl ThenType for u16 {
    fn then_type(self, t: GeomType) -> (u16, GeomType) {
        (self, t)
    }
}

async fn wgs84_extent(
    pool: &PgPool,
    schema: Option<&str>,
    table: Option<&str>,
    column: &str,
    from: &str,
    srid: u16,
) -> Option<Rect<f64>> {
    let col = quote_ident(column);
    // fast estimate from statistics, exact extent as fallback
    let estimated = match (schema, table) {
        (Some(s), Some(t)) => sqlx::query(&format!(
            "SELECT ST_XMin(e), ST_YMin(e), ST_XMax(e), ST_YMax(e) FROM (SELECT ST_Transform(ST_SetSRID(ST_EstimatedExtent($1, $2, $3)::geometry, {srid}), 4326) AS e) x"
        ))
        .bind(s)
        .bind(t)
        .bind(column)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten(),
        _ => None,
    };
    let row = match estimated {
        Some(r) if r.try_get::<Option<f64>, _>(0).ok().flatten().is_some() => r,
        _ => sqlx::query(&format!(
            "SELECT ST_XMin(e), ST_YMin(e), ST_XMax(e), ST_YMax(e) FROM (SELECT ST_Transform(ST_SetSRID(ST_Extent({col})::geometry, {srid}), 4326) AS e FROM {from}) x"
        ))
        .fetch_optional(pool)
        .await
        .ok()??,
    };
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

fn value_type(db_type: &str) -> ValueType {
    let t = db_type.to_lowercase();
    if t == "geometry" || t.starts_with("geometry(") {
        ValueType::Geometry(GeomType::Geometry)
    } else if matches!(t.as_str(), "smallint" | "integer" | "bigint") {
        ValueType::Integer
    } else if matches!(t.as_str(), "real" | "double precision") {
        ValueType::Double
    } else if t.starts_with("numeric") {
        ValueType::Decimal
    } else if t == "boolean" {
        ValueType::Boolean
    } else if t == "date" {
        ValueType::Date
    } else if t.starts_with("timestamp") {
        ValueType::DateTime
    } else if t.starts_with("time") {
        ValueType::Time
    } else {
        ValueType::String
    }
}

impl PgStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        pool: PgPool,
        schema: Option<String>,
        table: Option<String>,
        sql: String,
        pk: Option<String>,
        name: QName,
        title: Option<String>,
        mapping: Option<SchemaMapping>,
    ) -> Result<Self> {
        let schema = schema.or_else(|| table.as_ref().map(|_| "public".to_string()));
        let from = match (&schema, &table) {
            (Some(s), Some(t)) => format!("{}.{}", quote_ident(s), quote_ident(t)),
            _ => format!("({sql}) AS t"),
        };
        let mut columns = describe_columns(&pool, &format!("SELECT * FROM {from}")).await?;
        if let (Some(s), Some(t)) = (&schema, &table) {
            let nn = not_null_columns(&pool, s, t).await?;
            for c in &mut columns {
                c.not_null = nn.contains(&c.name);
            }
        }
        let pk = match (pk, &schema, &table) {
            (Some(p), _, _) => Some(p),
            (None, Some(s), Some(t)) => primary_key(&pool, s, t).await,
            _ => None,
        };
        let has_column = |n: &str| columns.iter().any(|c| c.name == n);
        let pk = pk.filter(|p| has_column(p));
        let mut geom_info = Vec::new();
        for c in columns
            .iter()
            .filter(|c| value_type(&c.db_type).is_geometry())
        {
            let info =
                geometry_info(&pool, schema.as_deref(), table.as_deref(), &c.name, &from).await;
            geom_info.push((c.name.clone(), info));
        }
        let srid = geom_info.first().map(|(_, (s, _))| *s).unwrap_or(4326);
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
                    let vt = match geom_info.iter().find(|(n, _)| *n == c.name) {
                        Some((_, (_, gt))) => ValueType::Geometry(*gt),
                        None => value_type(&c.db_type),
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
        if let Some((gcol, _)) = geom_info.first() {
            feature_type.wgs84_bbox = wgs84_extent(
                &pool,
                schema.as_deref(),
                table.as_deref(),
                gcol,
                &from,
                srid,
            )
            .await;
        }
        let mut col_infos = Vec::new();
        let mut gid_cols = Vec::new();
        let mut meta_cols = Vec::new();
        let mut xml_cols = Vec::new();
        for p in &feature_type.properties {
            match columns.iter().find(|c| c.name == p.column) {
                Some(c) => {
                    col_infos.push(Some(ColumnInfo {
                        db_type: c.db_type.clone(),
                        rtree: None,
                        coords: None,
                    }));
                    let gid = format!("{}_gmlid", p.column);
                    gid_cols.push(has_column(&gid).then_some(gid));
                    let meta = format!("{}_gmlmeta", p.column);
                    meta_cols.push(has_column(&meta).then_some(meta));
                    let gx = format!("{}_gmlxml", p.column);
                    xml_cols.push(has_column(&gx).then_some(gx));
                    let gx = format!("{}_gmlxml", p.column);
                    xml_cols.push(has_column(&gx).then_some(gx));
                }
                None => {
                    if p.min_occurs > 0 {
                        warn!("{}: column `{}` missing", name.prefixed(), p.column);
                    }
                    col_infos.push(None);
                    gid_cols.push(None);
                    meta_cols.push(None);
                    xml_cols.push(None);
                }
            }
        }
        let id = match (&pk, has_column("gml_id")) {
            (Some(k), true) => IdMapping::GmlId {
                column: "gml_id".to_string(),
                pk: k.clone(),
            },
            (Some(k), false) => IdMapping::Pk {
                column: k.clone(),
                db_type: columns
                    .iter()
                    .find(|c| c.name == *k)
                    .map(|c| c.db_type.clone())
                    .unwrap_or_else(|| "bigint".to_string()),
            },
            (None, _) => IdMapping::None,
        };
        let layout = SqlLayout {
            dialect: Dialect::Postgres,
            feature_type: feature_type.clone(),
            from,
            columns: col_infos,
            id,
            key_column: pk,
            geometry_id_columns: gid_cols,
            geometry_meta_columns: meta_cols,
            geometry_xml_columns: xml_cols,
        };
        Ok(PgStore {
            pool,
            layout: Arc::new(layout),
            feature_type: Arc::new(feature_type),
        })
    }
}

fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    binds: &[SqlValue],
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    for b in binds {
        q = match b {
            SqlValue::Text(t) => q.bind(t.clone()),
            SqlValue::Int(i) => q.bind(*i),
            SqlValue::Float(f) => q.bind(*f),
            SqlValue::Bool(b) => q.bind(*b),
            SqlValue::Bytes(v) => q.bind(v.clone()),
            SqlValue::TextArray(a) => q.bind(a.clone()),
        };
    }
    q
}

fn decode_value(
    row: &PgRow,
    prop: &PropertyDef,
    gml_id: Option<String>,
    meta: Option<String>,
) -> Result<Value> {
    let col = prop.column.as_str();
    let raw = row.try_get_raw(col)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let pg_type = raw.type_info().name().to_string();
    if prop.is_xml() {
        return Ok(Value::Xml(row.try_get::<String, _>(col)?));
    }
    Ok(match &prop.value_type {
        ValueType::Geometry(_) => {
            let wkb: Vec<u8> = row.try_get(col)?;
            let (geometry, z) = decode_wkb_z(&wkb).map_err(|e| StoreError::Data(e.to_string()))?;
            Value::Geometry(GeomValue {
                geometry,
                gml_id,
                meta,
                z,
            })
        }
        ValueType::Integer => match pg_type.as_str() {
            "INT2" => Value::Integer(row.try_get::<i16, _>(col)? as i64),
            "INT4" => Value::Integer(row.try_get::<i32, _>(col)? as i64),
            "INT8" => Value::Integer(row.try_get::<i64, _>(col)?),
            _ => Value::String(row.try_get::<String, _>(col)?),
        },
        ValueType::Double => match pg_type.as_str() {
            "FLOAT4" => Value::Double(row.try_get::<f32, _>(col)? as f64),
            "FLOAT8" => Value::Double(row.try_get::<f64, _>(col)?),
            _ => Value::String(row.try_get::<String, _>(col)?),
        },
        ValueType::Boolean => match pg_type.as_str() {
            "BOOL" => Value::Boolean(row.try_get::<bool, _>(col)?),
            _ => Value::String(row.try_get::<String, _>(col)?),
        },
        ValueType::Decimal => Value::Decimal(row.try_get::<String, _>(col)?),
        ValueType::Date => Value::Date(row.try_get::<String, _>(col)?),
        ValueType::DateTime => Value::DateTime(row.try_get::<String, _>(col)?),
        ValueType::Time => Value::Time(row.try_get::<String, _>(col)?),
        _ => Value::String(row.try_get::<String, _>(col)?),
    })
}

fn decode_row(row: &PgRow, layout: &SqlLayout, props: &[usize], rownum: u64) -> Result<Feature> {
    let ft = &layout.feature_type;
    let key = if layout.key_column.is_some() {
        row.try_get::<Option<String>, _>("__key")?
    } else {
        Some(rownum.to_string())
    };
    let gml_id = match &layout.id {
        IdMapping::GmlId { .. } => row.try_get::<Option<String>, _>("__gml_id")?,
        _ => None,
    };
    let mut values = vec![Value::Null; ft.properties.len()];
    for &i in props {
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
        values[i] = decode_value(row, &ft.properties[i], gid, meta)?;
    }
    Ok(Feature {
        id: layout.feature_id(key, gml_id),
        values,
    })
}

#[async_trait]
impl FeatureStore for PgStore {
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
                if let Value::Geometry(g) = decode_value(&row, prop, gid, meta)? {
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
            return self
                .query(q)
                .try_fold(0u64, |n, _| async move { Ok(n + 1) })
                .await;
        }
        let row = bind_all(sqlx::query(&plan.sql), &plan.binds)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<i64, _>(0)? as u64)
    }
}
