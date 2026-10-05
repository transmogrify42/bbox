//! ClickHouse feature store (native TCP protocol or HTTP interface).

use super::sql::{quote_ident, ColumnInfo, Dialect, IdMapping, SqlValue};
use super::*;
use crate::wfs::wkb::decode_wkb;
use futures::TryStreamExt;
use log::{info, warn};

/// Cell value of a result row
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
}

impl Cell {
    fn text(&self) -> Option<String> {
        match self {
            Cell::Null => None,
            Cell::Int(i) => Some(i.to_string()),
            Cell::UInt(u) => Some(u.to_string()),
            Cell::Float(f) => Some(format_double(*f)),
            Cell::Text(t) => Some(t.clone()),
        }
    }
}

/// Row with cells in select order
pub type Row = Vec<Cell>;
pub type RowStream = BoxStream<'static, Result<Row>>;

/// Connection to a ClickHouse server
#[derive(Clone)]
pub enum ChClient {
    Native(clickhouse_arrow::NativeClient),
    Http {
        client: reqwest::Client,
        url: String,
        user: String,
        password: String,
        database: String,
    },
}

fn ch_err(e: impl std::fmt::Display) -> StoreError {
    StoreError::Data(format!("ClickHouse: {e}"))
}

impl ChClient {
    /// Connect from URL `tcp://user:pw@host:9000/db` or `http(s)://user:pw@host:8123/db`
    pub async fn connect(url: &str) -> Result<Self> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| ch_err(format!("invalid URL `{url}`")))?;
        let (auth, hostpart) = match rest.rsplit_once('@') {
            Some((a, h)) => (Some(a), h),
            None => (None, rest),
        };
        let (user, password) = match auth {
            Some(a) => match a.split_once(':') {
                Some((u, p)) => (u.to_string(), p.to_string()),
                None => (a.to_string(), String::new()),
            },
            None => ("default".to_string(), String::new()),
        };
        // optional `?compression=zstd|lz4|none` (native protocol)
        let (hostpart, compression) = match hostpart.split_once('?') {
            Some((h, q)) => (
                h,
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("compression="))
                    .unwrap_or("zstd")
                    .to_lowercase(),
            ),
            None => (hostpart, "zstd".to_string()),
        };
        let (host, database) = match hostpart.split_once('/') {
            Some((h, d)) => (h.to_string(), d.trim_end_matches('/').to_string()),
            None => (hostpart.to_string(), "default".to_string()),
        };
        let database = if database.is_empty() {
            "default".to_string()
        } else {
            database
        };
        match scheme {
            "tcp" | "clickhouse" | "native" => {
                info!("Connecting to ClickHouse native protocol at {host}");
                // the server compresses with `network_compression_method`: keep both in sync
                let method = match compression.as_str() {
                    "lz4" => clickhouse_arrow::CompressionMethod::LZ4,
                    "none" => clickhouse_arrow::CompressionMethod::None,
                    _ => clickhouse_arrow::CompressionMethod::ZSTD,
                };
                let mut builder = clickhouse_arrow::ClientBuilder::new()
                    .with_endpoint(host)
                    .with_username(user)
                    .with_password(password)
                    .with_database(database)
                    .with_compression(method);
                if compression != "none" {
                    builder =
                        builder.with_setting("network_compression_method", compression.as_str());
                }
                let client = builder.build_native().await.map_err(ch_err)?;
                Ok(ChClient::Native(client))
            }
            "http" | "https" => {
                info!("Connecting to ClickHouse HTTP interface at {host}");
                Ok(ChClient::Http {
                    client: reqwest::Client::new(),
                    url: format!("{scheme}://{host}/"),
                    user,
                    password,
                    database,
                })
            }
            other => Err(ch_err(format!("unsupported scheme `{other}`"))),
        }
    }

    /// Execute a query, streaming rows
    pub fn query(&self, sql: String) -> RowStream {
        match self.clone() {
            ChClient::Native(client) => {
                let stream = async_stream::try_stream! {
                    let blocks = client
                        .query_raw(sql, None::<clickhouse_arrow::QueryParams>, clickhouse_arrow::Qid::new())
                        .await
                        .map_err(ch_err)?;
                    let mut blocks = Box::pin(blocks);
                    while let Some(block) = blocks.next().await {
                        let mut block = block.map_err(ch_err)?;
                        let rows: Vec<Row> = block
                            .take_iter_rows()
                            .map(|row| row.iter().map(|(_, _, v)| native_cell(v)).collect())
                            .collect();
                        for row in rows {
                            yield row;
                        }
                    }
                };
                stream.boxed()
            }
            ChClient::Http {
                client,
                url,
                user,
                password,
                database,
            } => {
                let stream = async_stream::try_stream! {
                    let resp = client
                        .post(&url)
                        .query(&[("database", database.as_str()), ("output_format_json_quote_64bit_integers", "0")])
                        .basic_auth(&user, Some(&password))
                        .body(format!("{sql} FORMAT JSONCompactEachRow"))
                        .send()
                        .await
                        .map_err(ch_err)?;
                    if !resp.status().is_success() {
                        let text = resp.text().await.unwrap_or_default();
                        Err(ch_err(text.trim()))?;
                        return;
                    }
                    let mut body = resp.bytes_stream();
                    let mut pending: Vec<u8> = Vec::new();
                    while let Some(chunk) = body.next().await {
                        let chunk = chunk.map_err(ch_err)?;
                        pending.extend_from_slice(&chunk);
                        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=pos).collect();
                            if line.len() > 1 {
                                yield json_row(&line)?;
                            }
                        }
                    }
                    if !pending.iter().all(|b| b.is_ascii_whitespace()) {
                        yield json_row(&pending)?;
                    }
                };
                stream.boxed()
            }
        }
    }

    /// Execute a statement without result
    pub async fn execute(&self, sql: &str) -> Result<()> {
        match self {
            ChClient::Native(client) => client.execute(sql.to_string(), None).await.map_err(ch_err),
            ChClient::Http {
                client,
                url,
                user,
                password,
                database,
            } => {
                let resp = client
                    .post(url)
                    .query(&[("database", database.as_str())])
                    .basic_auth(user, Some(password))
                    .body(sql.to_string())
                    .send()
                    .await
                    .map_err(ch_err)?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    Err(ch_err(resp.text().await.unwrap_or_default().trim()))
                }
            }
        }
    }

    pub async fn rows(&self, sql: &str) -> Result<Vec<Row>> {
        self.query(sql.to_string()).try_collect().await
    }
}

fn native_cell(v: &clickhouse_arrow::Value) -> Cell {
    use clickhouse_arrow::Value as V;
    match v {
        V::Null => Cell::Null,
        V::Int8(i) => Cell::Int(*i as i64),
        V::Int16(i) => Cell::Int(*i as i64),
        V::Int32(i) => Cell::Int(*i as i64),
        V::Int64(i) => Cell::Int(*i),
        V::UInt8(i) => Cell::UInt(*i as u64),
        V::UInt16(i) => Cell::UInt(*i as u64),
        V::UInt32(i) => Cell::UInt(*i as u64),
        V::UInt64(i) => Cell::UInt(*i),
        V::Float32(f) => Cell::Float(*f as f64),
        V::Float64(f) => Cell::Float(*f),
        V::String(b) => Cell::Text(String::from_utf8_lossy(b).to_string()),
        V::Uuid(u) => Cell::Text(u.to_string()),
        other => Cell::Text(format!("{other:?}")),
    }
}

fn json_row(line: &[u8]) -> Result<Row> {
    let v: serde_json::Value = serde_json::from_slice(line).map_err(ch_err)?;
    let arr = v
        .as_array()
        .ok_or_else(|| ch_err("expected JSON array row"))?;
    Ok(arr
        .iter()
        .map(|c| match c {
            serde_json::Value::Null => Cell::Null,
            serde_json::Value::Bool(b) => Cell::UInt(*b as u64),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Cell::Int(i)
                } else if let Some(u) = n.as_u64() {
                    Cell::UInt(u)
                } else {
                    Cell::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            serde_json::Value::String(s) => Cell::Text(s.clone()),
            other => Cell::Text(other.to_string()),
        })
        .collect())
}

/// Escape a string literal
pub fn ch_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Replace `?` placeholders with ClickHouse literals
pub fn inline_binds(sql: &str, binds: &[SqlValue]) -> String {
    let mut out = String::with_capacity(sql.len() + binds.len() * 8);
    let mut it = binds.iter();
    let mut in_str = false;
    let mut prev = ' ';
    for c in sql.chars() {
        if c == '\'' && prev != '\\' {
            in_str = !in_str;
        }
        if c == '?' && !in_str {
            match it.next() {
                Some(SqlValue::Text(t)) => out.push_str(&ch_string(t)),
                Some(SqlValue::Int(i)) => out.push_str(&i.to_string()),
                Some(SqlValue::Float(f)) => out.push_str(&format!("{f:?}")),
                Some(SqlValue::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
                Some(SqlValue::Bytes(b)) => {
                    out.push_str("unhex('");
                    for x in b {
                        out.push_str(&format!("{x:02X}"));
                    }
                    out.push_str("')");
                }
                Some(SqlValue::TextArray(a)) => {
                    out.push('[');
                    out.push_str(&a.iter().map(|s| ch_string(s)).collect::<Vec<_>>().join(","));
                    out.push(']');
                }
                None => out.push('?'),
            }
        } else {
            out.push(c);
        }
        prev = c;
    }
    out
}

/// Geometry encoding of a column
#[derive(Clone, Debug, PartialEq)]
enum GeomEncoding {
    /// Native geo type, selected as hex WKB
    Native,
    Wkb,
    Wkt,
}

pub struct ChStore {
    client: ChClient,
    layout: Arc<SqlLayout>,
    feature_type: Arc<FeatureTypeDef>,
    decoder: Arc<Decoder>,
}

/// Row decoding
struct Decoder {
    layout: Arc<SqlLayout>,
    /// Select expressions of loaded properties, by property index
    select: Vec<Option<String>>,
    geom_encoding: Vec<Option<GeomEncoding>>,
}

/// ClickHouse type to value type
fn value_type(t: &str) -> (ValueType, bool) {
    let mut ty = t.trim();
    let mut nullable = false;
    if let Some(inner) = ty
        .strip_prefix("Nullable(")
        .and_then(|s| s.strip_suffix(')'))
    {
        ty = inner;
        nullable = true;
    }
    if let Some(inner) = ty
        .strip_prefix("LowCardinality(")
        .and_then(|s| s.strip_suffix(')'))
    {
        ty = inner
            .strip_prefix("Nullable(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner);
    }
    let vt = if ty.starts_with("Int") || ty.starts_with("UInt") {
        ValueType::Integer
    } else if ty.starts_with("Float") {
        ValueType::Double
    } else if ty.starts_with("Decimal") {
        ValueType::Decimal
    } else if ty == "Bool" {
        ValueType::Boolean
    } else if ty.starts_with("Date32") || ty == "Date" {
        ValueType::Date
    } else if ty.starts_with("DateTime") {
        ValueType::DateTime
    } else {
        match ty {
            "Point" => ValueType::Geometry(GeomType::Point),
            "Ring" | "LineString" => ValueType::Geometry(GeomType::LineString),
            "MultiLineString" => ValueType::Geometry(GeomType::MultiLineString),
            "Polygon" => ValueType::Geometry(GeomType::Polygon),
            "MultiPolygon" => ValueType::Geometry(GeomType::MultiPolygon),
            _ => ValueType::String,
        }
    };
    (vt, nullable)
}

impl ChStore {
    /// The same store with the feature type in another namespace (no queries)
    pub fn renamed(&self, name: QName, title: Option<String>) -> Self {
        let mut ft = (*self.feature_type).clone();
        for p in ft.properties.iter_mut() {
            if !p.name.is_gml() {
                p.name = QName::new(&name.ns, &name.prefix, &p.name.local);
            }
        }
        ft.name = name;
        if title.is_some() {
            ft.title = title;
        }
        // filters are translated against the layout's feature type
        let layout = Arc::new(SqlLayout {
            feature_type: ft.clone(),
            ..(*self.layout).clone()
        });
        ChStore {
            client: self.client.clone(),
            decoder: Arc::new(Decoder {
                layout: layout.clone(),
                select: self.decoder.select.clone(),
                geom_encoding: self.decoder.geom_encoding.clone(),
            }),
            layout,
            feature_type: Arc::new(ft),
        }
    }

    pub async fn create(
        client: ChClient,
        cfg: &crate::config::ClickhouseCollectionCfg,
        name: QName,
        title: Option<String>,
        mapping: Option<SchemaMapping>,
    ) -> Result<Self> {
        let from = match (&cfg.table_name, &cfg.sql) {
            (_, Some(sql)) => format!("({sql}) AS t"),
            (Some(t), None) => quote_ident(t),
            _ => return Err(ch_err("configuration `table_name` or `sql` missing")),
        };
        let describe = match (&cfg.table_name, &cfg.sql) {
            (_, Some(sql)) => format!("DESCRIBE TABLE ({sql})"),
            (Some(t), None) => format!("DESCRIBE TABLE {}", quote_ident(t)),
            _ => unreachable!(),
        };
        let rows = client.rows(&describe).await?;
        let columns: Vec<(String, String)> = rows
            .iter()
            .filter_map(|r| Some((r.first()?.text()?, r.get(1)?.text()?)))
            .collect();
        let has_column = |n: &str| columns.iter().any(|(c, _)| c == n);
        let srid = cfg.srid.unwrap_or(4326);
        let pk = cfg.fid_field.clone().filter(|p| has_column(p));
        // geometry from configuration or first geo column
        let lonlat = match (&cfg.lon_field, &cfg.lat_field) {
            (Some(x), Some(y)) if has_column(x) && has_column(y) => Some((x.clone(), y.clone())),
            _ => None,
        };
        let geom_col = cfg
            .geometry_field
            .clone()
            .filter(|g| has_column(g))
            .or_else(|| {
                if lonlat.is_some() {
                    None
                } else {
                    columns
                        .iter()
                        .find(|(_, t)| value_type(t).0.is_geometry())
                        .map(|(c, _)| c.clone())
                }
            });
        let mut feature_type = match mapping {
            Some(m) => m.feature_type,
            None => {
                let mut props = standard_gml_properties();
                if lonlat.is_some() {
                    props.push(PropertyDef::simple(
                        QName::new(&name.ns, &name.prefix, "geom"),
                        "geom",
                        ValueType::Geometry(GeomType::Point),
                    ));
                }
                for (c, t) in &columns {
                    if Some(c) == pk.as_ref() || props.iter().any(|p| p.column == *c) {
                        continue;
                    }
                    let (mut vt, nullable) = value_type(t);
                    if Some(c) == geom_col.as_ref() && !vt.is_geometry() {
                        vt = ValueType::Geometry(GeomType::Geometry);
                    }
                    if map_gml_column(&mut props, c, &vt) {
                        continue;
                    }
                    let mut p = PropertyDef::simple(QName::new(&name.ns, &name.prefix, c), c, vt);
                    if !nullable {
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
        // column infos, select expressions and geometry encodings
        let mut col_infos = Vec::new();
        let mut select = Vec::new();
        let mut geom_encoding = Vec::new();
        for p in &feature_type.properties {
            let col_type = columns
                .iter()
                .find(|(c, _)| *c == p.column)
                .map(|(_, t)| t.clone());
            let qc = quote_ident(&p.column);
            if p.is_geometry() && lonlat.is_some() && p.column == "geom" && col_type.is_none() {
                let (x, y) = lonlat.clone().expect("lonlat");
                let (qx, qy) = (quote_ident(&x), quote_ident(&y));
                col_infos.push(Some(ColumnInfo {
                    db_type: "Point".into(),
                    rtree: None,
                    coords: Some((qx.clone(), qy.clone())),
                }));
                select.push(Some(format!("hex(wkb(CAST(({qx}, {qy}) AS Point)))")));
                geom_encoding.push(Some(GeomEncoding::Native));
                continue;
            }
            let Some(t) = col_type else {
                col_infos.push(None);
                select.push(None);
                geom_encoding.push(None);
                continue;
            };
            let (vt, _) = value_type(&t);
            let coords = (vt == ValueType::Geometry(GeomType::Point)).then(|| {
                (
                    format!("tupleElement({qc},1)"),
                    format!("tupleElement({qc},2)"),
                )
            });
            col_infos.push(Some(ColumnInfo {
                db_type: t.clone(),
                rtree: None,
                coords,
            }));
            if p.is_geometry() {
                let enc = if vt.is_geometry() {
                    GeomEncoding::Native
                } else {
                    match cfg.geometry_format.as_deref() {
                        Some("wkt") => GeomEncoding::Wkt,
                        Some("wkb") => GeomEncoding::Wkb,
                        _ => {
                            // detect from data
                            let sample = client
                                .rows(&format!(
                                    "SELECT {qc} FROM {from} WHERE {qc} IS NOT NULL LIMIT 1"
                                ))
                                .await
                                .unwrap_or_default();
                            let s = sample
                                .first()
                                .and_then(|r| r.first())
                                .and_then(|c| c.text())
                                .unwrap_or_default();
                            if s.trim_start()
                                .chars()
                                .next()
                                .map(|c| {
                                    c.is_ascii_alphabetic()
                                        && !s.starts_with("01")
                                        && !s.starts_with("00")
                                })
                                .unwrap_or(false)
                                && s.contains('(')
                            {
                                GeomEncoding::Wkt
                            } else {
                                GeomEncoding::Wkb
                            }
                        }
                    }
                };
                select.push(Some(match enc {
                    GeomEncoding::Native => format!("hex(wkb({qc}))"),
                    GeomEncoding::Wkb => format!("hex({qc})"),
                    GeomEncoding::Wkt => qc.clone(),
                }));
                geom_encoding.push(Some(enc));
            } else {
                let simple = matches!(
                    vt,
                    ValueType::Integer | ValueType::Double | ValueType::Boolean
                ) || (vt == ValueType::String && (t.contains("String")));
                select.push(Some(if simple {
                    qc.clone()
                } else {
                    format!("toString({qc})")
                }));
                geom_encoding.push(None);
            }
        }
        // extent of point data
        if let Some((x, y)) = &lonlat {
            let (qx, qy) = (quote_ident(x), quote_ident(y));
            if let Ok(rows) = client
                .rows(&format!(
                    "SELECT min({qx}), min({qy}), max({qx}), max({qy}) FROM {from}"
                ))
                .await
            {
                if let Some(r) = rows.first() {
                    let f = |i: usize| {
                        r.get(i)
                            .and_then(|c| c.text())
                            .and_then(|t| t.parse::<f64>().ok())
                    };
                    if let (Some(a), Some(b), Some(c), Some(d)) = (f(0), f(1), f(2), f(3)) {
                        feature_type.wgs84_bbox =
                            gpkg::wgs84_extent(geo::Rect::new((a, b), (c, d)), srid);
                    }
                }
            }
        } else if let Some(g) = &geom_col {
            if columns.iter().any(|(c, t)| c == g && t.contains("Point")) {
                let q = quote_ident(g);
                if let Ok(rows) = client.rows(&format!("SELECT min(tupleElement({q},1)), min(tupleElement({q},2)), max(tupleElement({q},1)), max(tupleElement({q},2)) FROM {from}")).await {
                    if let Some(r) = rows.first() {
                        let f = |i: usize| r.get(i).and_then(|c| c.text()).and_then(|t| t.parse::<f64>().ok());
                        if let (Some(a), Some(b), Some(c), Some(d)) = (f(0), f(1), f(2), f(3)) {
                            feature_type.wgs84_bbox = gpkg::wgs84_extent(geo::Rect::new((a, b), (c, d)), srid);
                        }
                    }
                }
            }
        }
        if pk.is_none() {
            warn!(
                "{}: no fid_field configured - feature ids are not stable",
                name.prefixed()
            );
        }
        let id = match &pk {
            Some(k) => IdMapping::Pk {
                column: k.clone(),
                db_type: columns
                    .iter()
                    .find(|(c, _)| c == k)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default(),
            },
            None => IdMapping::None,
        };
        let n = feature_type.properties.len();
        let layout = SqlLayout {
            dialect: Dialect::ClickHouse,
            feature_type: feature_type.clone(),
            from,
            columns: col_infos,
            id,
            key_column: pk,
            geometry_id_columns: vec![None; n],
            geometry_meta_columns: vec![None; n],
            geometry_xml_columns: vec![None; n],
        };
        let layout = Arc::new(layout);
        Ok(ChStore {
            client,
            decoder: Arc::new(Decoder {
                layout: layout.clone(),
                select,
                geom_encoding,
            }),
            layout,
            feature_type: Arc::new(feature_type),
        })
    }

    /// SQL with ClickHouse specific select expressions
    fn sql(&self, q: &StoreQuery) -> (String, QueryPlan, Vec<usize>) {
        let plan = self.layout.plan(q);
        let props = self.layout.loaded_properties(q);
        let mut cols = Vec::new();
        if let Some(k) = &self.layout.key_column {
            cols.push(format!("toString({}) AS __key", quote_ident(k)));
        }
        for &i in &props {
            if let Some(e) = &self.decoder.select[i] {
                cols.push(format!("{e} AS {}", quote_ident(&format!("__p{i}"))));
            }
        }
        if cols.is_empty() {
            cols.push("1".to_string());
        }
        // replace the generic select list
        let generic = plan
            .sql
            .split_once(" FROM ")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let sql = format!("SELECT {} FROM {}", cols.join(", "), generic);
        (inline_binds(&sql, &plan.binds), plan, props)
    }
}

impl Decoder {
    fn decode(&self, row: &Row, props: &[usize], rownum: u64) -> Result<Feature> {
        let ft = &self.layout.feature_type;
        let mut cells = row.iter();
        let key = if self.layout.key_column.is_some() {
            cells.next().and_then(|c| c.text())
        } else {
            Some(rownum.to_string())
        };
        let mut values = vec![Value::Null; ft.properties.len()];
        for &i in props {
            if self.select[i].is_none() {
                continue;
            }
            let cell = cells.next().cloned().unwrap_or(Cell::Null);
            let prop = &ft.properties[i];
            values[i] = match (&cell, &prop.value_type) {
                (Cell::Null, _) => Value::Null,
                (_, ValueType::Geometry(_)) => {
                    let text = cell.text().unwrap_or_default();
                    let geometry = match self.geom_encoding[i] {
                        Some(GeomEncoding::Wkt) => {
                            use geozero::ToGeo;
                            geozero::wkt::Wkt(&text).to_geo().map_err(ch_err)?
                        }
                        _ => {
                            let bytes =
                                hex_decode(&text).ok_or_else(|| ch_err("invalid hex WKB"))?;
                            if bytes.is_empty() {
                                values[i] = Value::Null;
                                continue;
                            }
                            decode_wkb(&bytes).map_err(ch_err)?
                        }
                    };
                    Value::Geometry(GeomValue::new(geometry))
                }
                (Cell::Int(v), ValueType::Integer) => Value::Integer(*v),
                (Cell::UInt(v), ValueType::Integer) => Value::Integer(*v as i64),
                (Cell::Float(v), ValueType::Double) => Value::Double(*v),
                (Cell::Int(v), ValueType::Double) => Value::Double(*v as f64),
                (Cell::UInt(v), ValueType::Boolean) => Value::Boolean(*v != 0),
                (Cell::Int(v), ValueType::Boolean) => Value::Boolean(*v != 0),
                (c, ValueType::Integer) => c
                    .text()
                    .and_then(|t| t.parse().ok())
                    .map(Value::Integer)
                    .unwrap_or(Value::Null),
                (c, ValueType::Double) => c
                    .text()
                    .and_then(|t| t.parse().ok())
                    .map(Value::Double)
                    .unwrap_or(Value::Null),
                (c, ValueType::Decimal) => Value::Decimal(c.text().unwrap_or_default()),
                (c, ValueType::Boolean) => {
                    Value::Boolean(matches!(c.text().as_deref(), Some("true") | Some("1")))
                }
                (c, ValueType::Date) => Value::Date(c.text().unwrap_or_default()),
                (c, ValueType::DateTime) => {
                    // `2020-01-01 00:00:00[.fff]` (server time zone UTC) -> xsd:dateTime
                    let t = c.text().unwrap_or_default();
                    Value::DateTime(if t.contains('T') {
                        t
                    } else {
                        format!("{}Z", t.replacen(' ', "T", 1))
                    })
                }
                (c, _) => Value::String(c.text().unwrap_or_default()),
            };
        }
        Ok(Feature {
            id: self.layout.feature_id(key, None),
            values,
        })
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[async_trait]
impl FeatureStore for ChStore {
    fn feature_type(&self) -> &FeatureTypeDef {
        &self.feature_type
    }

    fn plan(&self, q: &StoreQuery) -> QueryPlan {
        let (sql, mut plan, _) = self.sql(q);
        plan.sql = sql;
        plan.binds = Vec::new();
        plan
    }

    fn query(&self, q: StoreQuery) -> FeatureStream {
        let (sql, plan, props) = self.sql(&q);
        let offset = if plan.paging_in_sql { q.offset } else { 0 };
        let rows = self.client.query(sql);
        let store = self.decoder.clone();
        let stream = async_stream::try_stream! {
            let mut rows = rows;
            let mut n = offset;
            while let Some(row) = rows.next().await {
                n += 1;
                yield store.decode(&row?, &props, n)?;
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
        let sql = inline_binds(&plan.sql.replacen("COUNT(*)", "count()", 1), &plan.binds);
        let rows = self.client.rows(&sql).await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.text())
            .and_then(|t| t.parse().ok())
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        assert_eq!(ch_string("a'b\\c"), r"'a\'b\\c'");
        assert_eq!(
            inline_binds(
                "x = ? AND y IN ? AND z = '?' AND w > ?",
                &[
                    SqlValue::Text("o'k".into()),
                    SqlValue::TextArray(vec!["a".into(), "b".into()]),
                    SqlValue::Float(1.5)
                ]
            ),
            r"x = 'o\'k' AND y IN ['a','b'] AND z = '?' AND w > 1.5"
        );
    }

    #[test]
    fn types() {
        assert_eq!(value_type("Nullable(Int64)"), (ValueType::Integer, true));
        assert_eq!(
            value_type("LowCardinality(String)"),
            (ValueType::String, false)
        );
        assert_eq!(
            value_type("DateTime64(3, 'UTC')"),
            (ValueType::DateTime, false)
        );
        assert_eq!(
            value_type("Point"),
            (ValueType::Geometry(GeomType::Point), false)
        );
        assert_eq!(
            value_type("MultiPolygon"),
            (ValueType::Geometry(GeomType::MultiPolygon), false)
        );
    }
}
