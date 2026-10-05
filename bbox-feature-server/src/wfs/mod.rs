//! OGC Web Feature Service (WFS 1.0.0, 1.1.0, 2.0.0, 2.0.2)

pub mod cache;
pub mod capabilities;
pub mod cql;
pub mod crs;
pub mod describe;
pub mod endpoint;
pub mod exception;
pub mod filter;
pub mod formats;
pub mod getfeature;
pub mod gml;
pub mod join;
pub mod model;
pub mod operations;
pub mod output;
pub mod query;
pub mod request;
pub mod service;
pub mod store;
pub mod storedquery;
pub mod version;
pub mod wkb;
pub mod xlink;
pub mod xml;
pub mod xsd;

pub use service::WfsService;
