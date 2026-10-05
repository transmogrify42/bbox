//! Wire level HTTP behaviour (raw TCP requests against a real HttpServer), as seen by TEAM Engine.

mod common;
use actix_web::App;
use bbox_core::lenient_http::http_server;
use bbox_core::service::ServiceEndpoints;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Start an HTTP server on an ephemeral port, returning its address
async fn serve(srv: TestServer) -> std::net::SocketAddr {
    let service = srv.service;
    // reserve an ephemeral port
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let server = http_server(
        move || {
            let service = service.clone();
            App::new().configure(move |cfg| service.register_endpoints(cfg))
        },
        &addr.to_string(),
        1,
        1,
    )
    .unwrap();
    actix_web::rt::spawn(server);
    addr
}

/// Send raw request bytes and return the raw response head
async fn raw(addr: std::net::SocketAddr, request: &[u8]) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    text.split("\r\n\r\n").next().unwrap_or("").to_string()
}

#[actix_web::test]
async fn camel_case_headers() {
    // TEAM Engine matches header names case sensitively (header[@name='Content-Type'])
    let addr = serve(synth_server().await).await;
    let head = raw(
        addr,
        b"GET /wfs?service=WFS&version=1.1.0&request=GetCapabilities HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("\r\nContent-Type: text/xml"), "{head}");
}

#[actix_web::test]
async fn raw_utf8_request_target() {
    // TEAM Engine (ets-wfs11) sends type names like sf:EntitéGénérique without percent-encoding
    let srv = cite11_server().await;
    let addr = serve(srv).await;
    let mut req = Vec::new();
    for _ in 0..2 {
        // keep-alive: two requests on one connection
        req.extend_from_slice("GET /wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:EntitéGénérique&maxFeatures=1 HTTP/1.1\r\nHost: x\r\n\r\n".as_bytes());
    }
    req.extend_from_slice(b"GET /wfs?service=WFS&request=GetCapabilities HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(&req).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert_eq!(text.matches("HTTP/1.1 200 OK").count(), 3, "{text}");
    assert_eq!(text.matches("<sf:EntitéGénérique ").count(), 2, "{text}");
    // POST body containing non-ASCII text is passed unchanged
    let body = "<wfs:GetFeature service=\"WFS\" version=\"1.1.0\" maxFeatures=\"1\" xmlns:wfs=\"http://www.opengis.net/wfs\" xmlns:sf=\"http://cite.opengeospatial.org/gmlsf\"><wfs:Query typeName=\"sf:EntitéGénérique\"/></wfs:GetFeature>";
    let req = format!("POST /wfs HTTP/1.1\r\nHost: x\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert_eq!(text.matches("<sf:EntitéGénérique ").count(), 1, "{text}");
}
