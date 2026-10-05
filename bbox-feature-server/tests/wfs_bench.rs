//! WFS benchmark: bbox-feature-server vs GeoServer on the same PostGIS data (tests/bench/run-bench.sh).
//!
//! Each scenario is a WFS 2.0 KVP request sent to both servers, first sequentially (latency),
//! then with 16 concurrent clients (throughput). Results are printed as a table.

mod common;
use common::fixtures::observation;
use std::time::{Duration, Instant};

fn enc(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

struct Scenario {
    name: &'static str,
    query: String,
}

fn scenarios() -> Vec<Scenario> {
    let fes = |body: &str| {
        enc(&format!(
            r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:app="http://example.com/app" xmlns:gml="http://www.opengis.net/gml/3.2">{body}</fes:Filter>"#
        ))
    };
    let gf = "service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations";
    let o = observation(424_242);
    let id = o.image_id.clone();
    // 1 degree box around a feature location (lat/lon order of urn:ogc:def:crs:EPSG::4326)
    let small = format!(
        "{},{},{},{}",
        o.lat - 0.5,
        o.lon - 0.5,
        o.lat + 0.5,
        o.lon + 0.5
    );
    vec![
        Scenario { name: "GetCapabilities", query: "service=WFS&version=2.0.0&request=GetCapabilities".into() },
        Scenario {
            name: "DescribeFeatureType",
            query: "service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=app:observations".into(),
        },
        Scenario {
            name: "image_id = (index)",
            query: format!(
                "{gf}&filter={}",
                fes(&format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{id}</fes:Literal></fes:PropertyIsEqualTo>"))
            ),
        },
        Scenario { name: "resourceId", query: format!("{gf}&resourceId=observations.500000") },
        Scenario {
            name: "BBOX 1deg, 100",
            query: format!("{gf}&count=100&bbox={small},urn:ogc:def:crs:EPSG::4326"),
        },
        Scenario {
            name: "BBOX 20deg, 5000 GML",
            query: format!("{gf}&count=5000&bbox=0,0,20,20,urn:ogc:def:crs:EPSG::4326"),
        },
        Scenario {
            name: "BBOX 20deg, 5000 JSON",
            query: format!("{gf}&count=5000&bbox=0,0,20,20,urn:ogc:def:crs:EPSG::4326&outputFormat=application/json"),
        },
        Scenario {
            name: "hits BBOX 20deg",
            query: format!("{gf}&resultType=hits&bbox=0,0,20,20,urn:ogc:def:crs:EPSG::4326"),
        },
        Scenario {
            name: "attr filter, 1000",
            query: format!(
                "{gf}&count=1000&filter={}",
                fes("<fes:And><fes:PropertyIsLessThan><fes:ValueReference>app:cloud</fes:ValueReference><fes:Literal>5</fes:Literal></fes:PropertyIsLessThan><fes:PropertyIsEqualTo><fes:ValueReference>app:label</fes:ValueReference><fes:Literal>class-1</fes:Literal></fes:PropertyIsEqualTo></fes:And>")
            ),
        },
        Scenario {
            name: "CQL_FILTER, 1000",
            query: format!("{gf}&count=1000&cql_filter={}", enc("cloud < 5 AND label = 'class-1'")),
        },
    ]
}

struct Stats {
    p50: Duration,
    p95: Duration,
    p99: Duration,
    rps: f64,
    bytes: usize,
    matched: String,
}

async fn fetch(client: &reqwest::Client, url: &str) -> (Duration, usize, String) {
    let t = Instant::now();
    let resp = client.get(url).send().await.expect("request");
    assert!(resp.status().is_success(), "{url}: {}", resp.status());
    let body = resp.bytes().await.expect("body");
    let elapsed = t.elapsed();
    let text = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    let matched = text
        .split("numberMatched")
        .nth(1)
        .map(|s| {
            s.trim_start_matches(['=', '"', ':'])
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect()
        })
        .unwrap_or_default();
    (elapsed, body.len(), matched)
}

async fn run(url: &str, requests: usize, concurrency: usize) -> Stats {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .build()
        .unwrap();
    // warm up
    let mut last = (Duration::ZERO, 0, String::new());
    for _ in 0..5.min(requests) {
        last = fetch(&client, url).await;
    }
    let start = Instant::now();
    let per_worker = requests / concurrency;
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = client.clone();
        let url = url.to_string();
        handles.push(tokio::spawn(async move {
            let mut times = Vec::with_capacity(per_worker);
            for _ in 0..per_worker {
                times.push(fetch(&client, &url).await.0);
            }
            times
        }));
    }
    let mut times: Vec<Duration> = Vec::new();
    for h in handles {
        times.extend(h.await.unwrap());
    }
    let total = start.elapsed();
    times.sort();
    let pct = |p: f64| times[((times.len() as f64 * p) as usize).min(times.len() - 1)];
    Stats {
        p50: pct(0.50),
        p95: pct(0.95),
        p99: pct(0.99),
        rps: times.len() as f64 / total.as_secs_f64(),
        bytes: last.1,
        matched: last.2,
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore]
async fn wfs_benchmark() {
    let (Ok(bbox), Ok(gs)) = (
        std::env::var("BBOX_WFS_BENCH_BBOX"),
        std::env::var("BBOX_WFS_BENCH_GEOSERVER"),
    ) else {
        return;
    };
    let requests: usize = std::env::var("BBOX_WFS_BENCH_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    println!(
        "| scenario | server | p50 ms (c=1) | p95 ms (c=1) | p99 ms (c=1) | req/s (c=1) | req/s (c=16) | p95 ms (c=16) | bytes | matched |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for s in scenarios() {
        // heavy scenarios with fewer requests
        let n = if s.name.contains("5000") {
            requests / 8
        } else {
            requests
        };
        let mut matched = Vec::new();
        let mut servers = vec![
            ("bbox PostGIS", format!("{bbox}/wfs")),
            ("GeoServer PostGIS", format!("{gs}/wfs")),
        ];
        if let Ok(ch) = std::env::var("BBOX_WFS_BENCH_BBOX_CH") {
            servers.push(("bbox ClickHouse", format!("{ch}/wfs")));
        }
        for (server, base) in servers {
            let url = format!("{base}?{}", s.query);
            let seq = run(&url, n.max(16), 1).await;
            let par = run(&url, n.max(16), 16).await;
            println!(
                "| {} | {server} | {} | {} | {} | {:.0} | {:.0} | {} | {} | {} |",
                s.name,
                ms(seq.p50),
                ms(seq.p95),
                ms(seq.p99),
                seq.rps,
                par.rps,
                ms(par.p95),
                seq.bytes,
                seq.matched
            );
            matched.push(seq.matched);
        }
        if matched.iter().any(|m| *m != matched[0]) {
            println!("  WARNING {}: numberMatched differs: {matched:?}", s.name);
        }
    }
}
