//! Dispatch of WFS operations other than GetCapabilities.

use crate::wfs::endpoint::OpResult;
use crate::wfs::exception::WfsError;
use crate::wfs::request::RawRequest;
use crate::wfs::service::WfsService;
use std::sync::Arc;

pub async fn dispatch(svc: &Arc<WfsService>, op: &str, raw: &RawRequest) -> OpResult {
    match op.to_ascii_lowercase().as_str() {
        "describefeaturetype" => crate::wfs::describe::describe_feature_type(svc, raw),
        "getfeature" => crate::wfs::getfeature::get_feature(svc, raw).await,
        "getpropertyvalue" => crate::wfs::getfeature::get_property_value(svc, raw).await,
        "liststoredqueries" => crate::wfs::storedquery::list_stored_queries(svc, raw),
        "describestoredqueries" => crate::wfs::storedquery::describe_stored_queries(svc, raw),
        "createstoredquery" => crate::wfs::storedquery::create_stored_query(svc, raw),
        "dropstoredquery" => crate::wfs::storedquery::drop_stored_query(svc, raw),
        "getgmlobject" => crate::wfs::xlink::get_gml_object(svc, raw).await,
        _ => Err((raw.error_version(), WfsError::not_supported(op))),
    }
}
