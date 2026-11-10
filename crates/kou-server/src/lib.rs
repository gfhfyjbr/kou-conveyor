use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use kou_store::{SessionLog, validate_session_id};
use kou_view::{HeaderPage, SessionDescription, SessionFile, native::SessionView};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    path::{Component, Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

const CACHE_BYTES: usize = 32 * 1024 * 1024;
const BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Server(Arc<Inner>);

struct Inner {
    directory: PathBuf,
    assets: Option<PathBuf>,
    cache: Mutex<Cache>,
    load: Mutex<()>,
    jobs: Arc<Semaphore>,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Cached>,
    bytes: usize,
}
struct Cached {
    id: String,
    view: Arc<SessionView>,
    bytes: usize,
}

impl Server {
    pub fn new(directory: PathBuf, assets: Option<PathBuf>) -> io::Result<Self> {
        let directory = directory.canonicalize()?;
        if !directory.is_dir() {
            return Err(io::Error::other("session directory is not a directory"));
        }
        let assets = assets.map(|path| path.canonicalize()).transpose()?;
        Ok(Self(Arc::new(Inner {
            directory,
            assets,
            cache: Mutex::new(Cache::default()),
            load: Mutex::new(()),
            jobs: Arc::new(Semaphore::new(4)),
        })))
    }

    fn cached(&self, id: &str) -> Result<Option<Arc<SessionView>>, ApiError> {
        let mut cache = self.0.cache.lock().map_err(|_| ApiError::internal())?;
        let Some(index) = cache.entries.iter().position(|entry| entry.id == id) else {
            return Ok(None);
        };
        let entry = cache.entries.remove(index).ok_or_else(ApiError::internal)?;
        if entry.view.log.ensure_current().is_err() {
            cache.bytes -= entry.bytes;
            return Ok(None);
        }
        let view = Arc::clone(&entry.view);
        cache.entries.push_back(entry);
        Ok(Some(view))
    }

    fn view(&self, id: &str) -> Result<Arc<SessionView>, ApiError> {
        validate_session_id(id)
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid session ID"))?;
        let path = self.0.directory.join(format!("{id}.session.jsonl"));
        let metadata = std::fs::symlink_metadata(&path).map_err(ApiError::io)?;
        if !metadata.file_type().is_file() {
            return Err(ApiError(StatusCode::NOT_FOUND, "session not found"));
        }
        if let Some(view) = self.cached(id)? {
            return Ok(view);
        }
        let _loading = self.0.load.lock().map_err(|_| ApiError::internal())?;
        if let Some(view) = self.cached(id)? {
            return Ok(view);
        }
        let log = SessionLog::open(path).map_err(ApiError::store)?;
        if log.session().id != id {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "session ID mismatch",
            ));
        }
        let view = Arc::new(SessionView::new(log).map_err(ApiError::store)?);
        let bytes = view.estimated_resident_bytes();
        if bytes <= CACHE_BYTES {
            let mut cache = self.0.cache.lock().map_err(|_| ApiError::internal())?;
            while cache.bytes.saturating_add(bytes) > CACHE_BYTES {
                let Some(old) = cache.entries.pop_front() else {
                    break;
                };
                cache.bytes -= old.bytes;
            }
            cache.bytes += bytes;
            cache.entries.push_back(Cached {
                id: id.into(),
                view: Arc::clone(&view),
                bytes,
            });
        }
        Ok(view)
    }
}

pub fn router(server: Server) -> Router {
    Router::new()
        .route("/api/health", get(|| async { Json(serde_json::json!({"implementation":"rust", "read_only":true, "execution_available":false})) }))
        .route("/api/v2/sessions", get(sessions))
        .route("/api/v2/sessions/{id}/view", get(description))
        .route("/api/v2/sessions/{id}/headers", get(headers))
        .route("/api/v2/sessions/{id}/items/{sequence}", get(item))
        .route("/api/v2/sessions/{id}/export", get(export))
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .layer(middleware::from_fn(local_request))
        .with_state(server)
}

pub async fn serve(listener: tokio::net::TcpListener, server: Server) -> io::Result<()> {
    axum::serve(listener, router(server))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

async fn local_request(request: Request<Body>, next: Next) -> Response {
    if !allowed_request(request.headers()) {
        return ApiError(StatusCode::FORBIDDEN, "local same-origin requests only").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    response
}

fn allowed_request(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let name = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if !name.eq_ignore_ascii_case("localhost")
        && !name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        return false;
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|h| h == "cross-site")
    {
        return false;
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let Ok(origin) = origin.to_str() else {
            return false;
        };
        if origin != format!("http://{host}") {
            return false;
        }
    }
    true
}

async fn blocking<T, F>(server: Server, function: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(Server) -> Result<T, ApiError> + Send + 'static,
{
    let permit = server
        .0
        .jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "server is busy"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        function(server)
    })
    .await
    .map_err(|_| ApiError::internal())?
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ListQuery {
    after: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct SessionPage {
    sessions: Vec<SessionFile>,
    next_after: String,
    more: bool,
}

async fn sessions(
    State(server): State<Server>,
    Query(query): Query<ListQuery>,
) -> Result<Json<SessionPage>, ApiError> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "limit must be 1..200"));
    }
    blocking(server, move |server| {
        let mut selected = BTreeMap::new();
        for entry in std::fs::read_dir(&server.0.directory).map_err(ApiError::io)? {
            let entry = entry.map_err(ApiError::io)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(id) = name.strip_suffix(".session.jsonl") else {
                continue;
            };
            if id <= query.after.as_str()
                || validate_session_id(id).is_err()
                || !entry.file_type().map_err(ApiError::io)?.is_file()
            {
                continue;
            }
            selected.insert(id.to_owned(), entry.metadata().map_err(ApiError::io)?.len());
            if selected.len() > limit + 1 {
                selected.pop_last();
            }
        }
        let more = selected.len() > limit;
        if more {
            selected.pop_last();
        }
        let next_after = selected
            .last_key_value()
            .map_or(query.after.clone(), |(id, _)| id.clone());
        Ok(Json(SessionPage {
            sessions: selected
                .into_iter()
                .map(|(id, bytes)| SessionFile { id, bytes })
                .collect(),
            next_after,
            more,
        }))
    })
    .await
}

async fn description(
    State(server): State<Server>,
    Path(id): Path<String>,
) -> Result<Json<SessionDescription>, ApiError> {
    blocking(server, move |server| {
        Ok(Json(server.view(&id)?.description()))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    generation: String,
    after: Option<u64>,
    limit: Option<usize>,
}

fn generation(view: &SessionView, expected: &str) -> Result<(), ApiError> {
    if view.log.generation() != expected {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "session changed; reload the view",
        ));
    }
    view.log.ensure_current().map_err(ApiError::store)
}

async fn headers(
    State(server): State<Server>,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<HeaderPage>, ApiError> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "limit must be 1..200"));
    }
    blocking(server, move |server| {
        let view = server.view(&id)?;
        generation(&view, &query.generation)?;
        Ok(Json(
            view.headers(query.after.unwrap_or(0), limit)
                .map_err(ApiError::store)?,
        ))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationQuery {
    generation: String,
}

async fn item(
    State(server): State<Server>,
    Path((id, sequence)): Path<(String, u64)>,
    Query(query): Query<GenerationQuery>,
) -> Result<Response, ApiError> {
    blocking(server, move |server| {
        let view = server.view(&id)?;
        generation(&view, &query.generation)?;
        if sequence == 0 || sequence > view.log.item_count() as u64 {
            return Err(ApiError(StatusCode::NOT_FOUND, "item not found"));
        }
        if view
            .log
            .item_source_bytes(sequence)
            .map_err(ApiError::store)?
            > BODY_BYTES
        {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "item is too large for inline view; use full session export",
            ));
        }
        let item = view.log.item(sequence).map_err(ApiError::store)?;
        let mut out = BoundedWriter(Vec::new());
        serde_json::to_writer(&mut out, &item).map_err(|_| {
            ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "item exceeds inline byte budget; use full session export",
            )
        })?;
        Ok(([(header::CONTENT_TYPE, "application/json")], out.0).into_response())
    })
    .await
}

struct BoundedWriter(Vec<u8>);
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > BODY_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("inline byte budget exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn export(
    State(server): State<Server>,
    Path(id): Path<String>,
    Query(query): Query<GenerationQuery>,
) -> Result<Response, ApiError> {
    let server_for_view = server.clone();
    let view = blocking(server_for_view, move |server| {
        let view = server.view(&id)?;
        generation(&view, &query.generation)?;
        Ok(view)
    })
    .await?;
    let permit = server
        .0
        .jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "server is busy"))?;
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(2);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        struct StreamWriter(tokio::sync::mpsc::Sender<Result<Bytes, io::Error>>);
        impl Write for StreamWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0
                    .blocking_send(Ok(Bytes::copy_from_slice(bytes)))
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected")
                    })?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        if let Err(error) = view.log.copy_committed(StreamWriter(sender.clone())) {
            let _ = sender.blocking_send(Err(io::Error::other(error)));
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=session.jsonl",
            ),
        ],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver)),
    )
        .into_response())
}

async fn index(State(server): State<Server>) -> Result<Response, ApiError> {
    file_asset(server, "index.html".into()).await
}
async fn asset(
    State(server): State<Server>,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    file_asset(server, path).await
}

async fn file_asset(server: Server, path: String) -> Result<Response, ApiError> {
    if FsPath::new(&path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid asset path"));
    }
    blocking(server, move |server| {
        let root = server.0.assets.as_ref().ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "WASM assets not built; run python3 compat/build_web.py",
        ))?;
        let file = root.join(&path).canonicalize().map_err(ApiError::io)?;
        if !file.starts_with(root) {
            return Err(ApiError(StatusCode::NOT_FOUND, "asset not found"));
        }
        let mime = match file.extension().and_then(|s| s.to_str()) {
            Some("html") if path == "index.html" => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("wasm") => "application/wasm",
            Some("css") => "text/css; charset=utf-8",
            _ => return Err(ApiError(StatusCode::NOT_FOUND, "asset not found")),
        };
        Ok((
            [(header::CONTENT_TYPE, mime)],
            std::fs::read(file).map_err(ApiError::io)?,
        )
            .into_response())
    })
    .await
}

#[derive(Debug)]
struct ApiError(StatusCode, &'static str);
impl ApiError {
    fn internal() -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }
    fn io(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::NotFound {
            Self(StatusCode::NOT_FOUND, "not found")
        } else {
            Self::internal()
        }
    }
    fn store(error: kou_store::Error) -> Self {
        match error {
            kou_store::Error::Changed => {
                Self(StatusCode::CONFLICT, "session changed; reload the view")
            }
            kou_store::Error::Io(error) => Self::io(error),
            _ => Self(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid or unsupported session",
            ),
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error":self.1}))).into_response()
    }
}
