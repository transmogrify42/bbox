//! Export fixtures and server configurations for running the OGC ETS suites (ets-wfs10, ets-wfs11,
//! ets-wfs20, DGIWG WFS 2.0) against a bbox-feature-server instance. See tests/ets/run-ets.sh.

mod common;
use common::*;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/wfs-ets");
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

const HOST: &str = "http://host.docker.internal";

#[tokio::test]
#[ignore]
async fn export_ets_fixtures() {
    let dir = out_dir();
    for d in ["cite10", "cite11", "synth"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    fixtures::cite10_gpkg(&dir.join("cite10")).await;
    fixtures::cite11_gpkg(&dir.join("cite11")).await;
    let synth = dir.join("synth/synth.gpkg");
    fixtures::synthetic_gpkg(&synth, 2000).await;
    // ets-wfs20 1.43 re-parses Instant.toString() of the temporal extent with a pattern requiring
    // milliseconds, which fails for whole-second values: give the ETS data a millisecond part.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", synth.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE observations SET acquired = replace(acquired, 'Z', '.123Z')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let webserver = |port: u16| {
        format!("[webserver]\nserver_addr = \"0.0.0.0:{port}\"\npublic_server_url = \"{HOST}:{port}\"\n")
    };
    // WFS 1.0: CITE cdf/cgf/ccf data
    let mut cfg = webserver(18010);
    cfg.push_str(&format!(
        "\n[[collections.directory]]\ndir = \"{}\"\n\n[wfs]\ncite_compliant = true\ntitle = \"BBOX WFS 1.0 CITE\"\n",
        dir.join("cite10").display()
    ));
    for s in fixtures::cite10_schemas() {
        cfg.push_str(&format!(
            "\n[[wfs.namespace]]\nprefix = \"{}\"\nuri = \"{}\"\nschema = \"{}\"\n",
            s.prefix,
            s.uri,
            s.xsd.display()
        ));
    }
    std::fs::write(dir.join("ets-wfs10.toml"), cfg).unwrap();
    // WFS 1.1: CITE sf data
    let mut cfg = webserver(18011);
    cfg.push_str(&format!(
        "\n[[collections.directory]]\ndir = \"{}\"\n\n[wfs]\ncite_compliant = true\ntitle = \"BBOX WFS 1.1 CITE\"\n",
        dir.join("cite11").display()
    ));
    for s in fixtures::cite11_schemas() {
        cfg.push_str(&format!(
            "\n[[wfs.namespace]]\nprefix = \"{}\"\nuri = \"{}\"\nschema = \"{}\"\n",
            s.prefix,
            s.uri,
            s.xsd.display()
        ));
    }
    std::fs::write(dir.join("ets-wfs11.toml"), cfg).unwrap();
    // WFS 2.0 and DGIWG: synthetic data, one application namespace
    for (name, port, extra) in [
        ("ets-wfs20", 18020, String::new()),
        (
            "ets-dgiwg",
            18021,
            "abstract = \"This server implements the DGIWG BASIC WFS profile of WFS 2.0\"\nkeywords = [\"Road\", \"Building\"]\naccess_constraints = \"unclassified\"\n".to_string(),
        ),
    ] {
        let mut cfg = webserver(port);
        cfg.push_str(&format!(
            "\n[[collections.directory]]\ndir = \"{}\"\n\n[wfs]\ncite_compliant = true\ntitle = \"BBOX WFS 2.0\"\nnamespace_prefix = \"app\"\nnamespace_uri = \"http://example.com/app\"\ncount_default = 1000\nother_crs = [3857, 32632]\n{extra}",
            dir.join("synth").display()
        ));
        // application schema with a nillable property (exercises PropertyIsNil)
        cfg.push_str(&format!(
            "\n[[wfs.namespace]]\nprefix = \"app\"\nuri = \"http://example.com/app\"\nschema = \"{}\"\n",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/synth/observations.xsd").canonicalize().unwrap().display()
        ));
        std::fs::write(dir.join(format!("{name}.toml")), cfg).unwrap();
    }
    eprintln!("ETS fixtures written to {}", dir.display());
}
