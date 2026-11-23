use std::{
    convert::Infallible,
    fs::{self, DirBuilder, OpenOptions},
    io,
    net::Ipv4Addr,
    path::Path,
    sync::Arc,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use kuvo_protocol::{
    gate::compatibility,
    version::{
        ROUTE_VERSION, ROUTE_VERSION_HEADER, SERVE_RECORD_VERSION, UnsupportedFormatVersion,
    },
};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let run = home.join("run");
    let mut dirs = DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
    dirs.create(&run)?;

    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(run.join("rust-serve.lock"))?;
    lock.try_lock()?;

    let mut secret = [0u8; 24];
    getrandom::fill(&mut secret).map_err(io::Error::other)?;
    let password: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
    let authorization = format!("Basic {}", STANDARD.encode(format!("opencode:{password}")));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();

    let info = Bytes::from(serde_json::to_vec(&info(home)?)?);
    let mut record = tempfile::NamedTempFile::new_in(&run)?;
    serde_json::to_writer(
        &mut record,
        &json!({
            "formatVersion": SERVE_RECORD_VERSION,
            "pid": std::process::id(),
            "port": port,
            "password": password,
            "kept": false,
        }),
    )?;
    record.persist(run.join("serve.json"))?;
    println!("{}", json!({"url": format!("http://127.0.0.1:{port}")}));

    let authorization: Arc<str> = authorization.into();
    loop {
        let (stream, _) = listener.accept().await?;
        let authorization = authorization.clone();
        let info = info.clone();
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(15))
                .serve_connection(
                    TokioIo::new(stream),
                    service_fn(move |request| {
                        std::future::ready(Ok::<_, Infallible>(answer(
                            request,
                            &authorization,
                            &info,
                        )))
                    }),
                )
                .await;
        });
    }
}

fn info(home: &Path) -> io::Result<serde_json::Value> {
    let chats = home.join("chats");
    fs::create_dir_all(&chats)?;
    let name = hostname::get()?;
    let is_mac = match fs::read_to_string(home.join("box/kind")) {
        Ok(kind) => kind.trim() == "mac",
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    // The scaffold serves only info. It must not pass the desktop's compatibility gate yet.
    let features = ["info"];
    let mut answer = json!({
        "kind": if is_mac { "mac" } else { "machine" },
        "name": name.to_string_lossy(),
        "features": features,
        "compatibility": compatibility(&features),
        "agents": false,
        "started": null,
        "engineState": "idle",
        "chats": chats.to_string_lossy(),
    });
    if is_mac {
        answer["app"] = json!("");
        answer["engine"] = json!("");
        answer["integrations"] = json!({});
    } else {
        let resident = json!({"running": VERSION, "current": VERSION, "behind": false});
        answer["bridge"] = json!(VERSION);
        answer["engine"] = json!(null);
        answer["running"] = json!(false);
        answer["stopped"] = json!(false);
        answer["pieces"] = json!({
            "link": resident,
            "agents": {"running": null, "current": VERSION, "behind": false, "renewal": null, "waits": null},
            "engine": {"running": null, "current": null, "behind": false},
        });
    }
    Ok(answer)
}

fn answer(request: Request<Incoming>, authorization: &str, info: &Bytes) -> Response<Full<Bytes>> {
    let given = request.headers().get(header::AUTHORIZATION);
    if !given.is_some_and(|given| bool::from(given.as_bytes().ct_eq(authorization.as_bytes()))) {
        let mut response = Response::new(Full::default());
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        return response;
    }

    let mut versions = request.headers().get_all(ROUTE_VERSION_HEADER).iter();
    if let Some(version) = versions.next() {
        let header = version.to_str().ok().filter(|_| versions.next().is_none());
        if let Some(refusal) = UnsupportedFormatVersion::for_route(Some(header.unwrap_or(""))) {
            return response(
                StatusCode::BAD_REQUEST,
                Bytes::from(json!(refusal).to_string()),
            );
        }
    }

    match (request.method(), request.uri().path()) {
        (&Method::GET, "/kuvo/info") => response(StatusCode::OK, info.clone()),
        (_, "/kuvo/info") => response(StatusCode::METHOD_NOT_ALLOWED, Bytes::from_static(b"{}")),
        _ => response(StatusCode::NOT_FOUND, Bytes::from_static(b"{}")),
    }
}

fn response(status: StatusCode, body: Bytes) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(ROUTE_VERSION_HEADER, ROUTE_VERSION)
        .body(Full::new(body))
        .expect("static response headers")
}
