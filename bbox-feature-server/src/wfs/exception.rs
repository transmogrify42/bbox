//! WFS exception reports (OGC ServiceExceptionReport 1.2.0, OWS 1.0, OWS 1.1).

use crate::wfs::filter::FilterError;
use crate::wfs::version::Version;
use crate::wfs::xml::escape;
use actix_web::http::StatusCode;

#[derive(Debug, Clone, PartialEq)]
pub struct WfsError {
    pub code: &'static str,
    pub locator: Option<String>,
    pub text: String,
}

pub type WfsResult<T> = Result<T, WfsError>;

impl WfsError {
    pub fn new(code: &'static str, locator: Option<&str>, text: impl Into<String>) -> Self {
        WfsError {
            code,
            locator: locator.map(str::to_string),
            text: text.into(),
        }
    }
    pub fn missing(locator: &str) -> Self {
        Self::new(
            "MissingParameterValue",
            Some(locator),
            format!("Missing parameter `{locator}`"),
        )
    }
    pub fn invalid(locator: &str, text: impl Into<String>) -> Self {
        Self::new("InvalidParameterValue", Some(locator), text)
    }
    pub fn not_supported(op: &str) -> Self {
        Self::new(
            "OperationNotSupported",
            Some(op),
            format!("Operation `{op}` is not supported"),
        )
    }
    pub fn option_not_supported(locator: &str, text: impl Into<String>) -> Self {
        Self::new("OptionNotSupported", Some(locator), text)
    }
    pub fn parsing(locator: &str, text: impl Into<String>) -> Self {
        Self::new("OperationParsingFailed", Some(locator), text)
    }
    pub fn processing(locator: &str, text: impl Into<String>) -> Self {
        Self::new("OperationProcessingFailed", Some(locator), text)
    }
    pub fn no_applicable(text: impl Into<String>) -> Self {
        Self::new("NoApplicableCode", None, text)
    }
    pub fn not_found(locator: &str, text: impl Into<String>) -> Self {
        Self::new("NotFound", Some(locator), text)
    }

    /// Map filter errors to exception codes
    pub fn from_filter(e: FilterError, locator: &str) -> Self {
        match e {
            FilterError::Parse(t) => Self::new("InvalidParameterValue", Some(locator), t),
            FilterError::UnknownProperty(p) => Self::new(
                "InvalidParameterValue",
                Some(locator),
                format!("Unknown property `{p}`"),
            ),
            FilterError::Processing(t) => Self::new("OperationProcessingFailed", Some(locator), t),
            FilterError::Crs(c) => Self::new(
                "InvalidParameterValue",
                Some("srsName"),
                format!("Unsupported CRS `{c}`"),
            ),
        }
    }

    /// HTTP status for the exception in the given version
    pub fn status(&self, version: Version) -> StatusCode {
        if !version.is_v2() {
            // TEAM Engine and clients of WFS 1.x expect exception reports with status 200
            return StatusCode::OK;
        }
        // WFS 2.0 Table D.2 / OWS 1.1
        match self.code {
            "OperationNotSupported" | "OptionNotSupported" => StatusCode::NOT_IMPLEMENTED,
            "OperationProcessingFailed" | "ResponseCacheExpired" | "LockHasExpired" => {
                StatusCode::FORBIDDEN
            }
            "NotFound" => StatusCode::NOT_FOUND,
            "NoApplicableCode" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    pub fn content_type(&self, version: Version) -> &'static str {
        match version {
            Version::V100 => "application/vnd.ogc.se_xml",
            _ => "application/xml",
        }
    }

    /// Exception report document
    pub fn render(&self, version: Version) -> String {
        let text = if self.text.is_empty() {
            self.code.to_string()
        } else {
            self.text.clone()
        };
        let locator = self
            .locator
            .as_ref()
            .map(|l| format!(r#" locator="{}""#, escape(l)))
            .unwrap_or_default();
        match version {
            Version::V100 => format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><ServiceExceptionReport version="1.2.0" xmlns="http://www.opengis.net/ogc" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/ogc http://schemas.opengis.net/wfs/1.0.0/OGC-exception.xsd"><ServiceException code="{}"{locator}>{}</ServiceException></ServiceExceptionReport>"#,
                self.code,
                escape(&text)
            ),
            Version::V110 => format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><ows:ExceptionReport version="1.0.0" xmlns:ows="http://www.opengis.net/ows" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/ows http://schemas.opengis.net/ows/1.0.0/owsExceptionReport.xsd"><ows:Exception exceptionCode="{}"{locator}><ows:ExceptionText>{}</ows:ExceptionText></ows:Exception></ows:ExceptionReport>"#,
                self.code,
                escape(&text)
            ),
            _ => format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><ows:ExceptionReport version="2.0.0" xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/ows/1.1 http://schemas.opengis.net/ows/1.1.0/owsExceptionReport.xsd"><ows:Exception exceptionCode="{}"{locator}><ows:ExceptionText>{}</ows:ExceptionText></ows:Exception></ows:ExceptionReport>"#,
                self.code,
                escape(&text)
            ),
        }
    }
}

impl std::fmt::Display for WfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.text)
    }
}
