use log::warn;
use std::sync::Arc;

use actix_web::{HttpRequest, HttpResponse, Responder, guard, http::Method, web};
use actix_ws::{Message, MessageStream, Session};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::config::EdgeDetect;
use crate::error::AppError;
use crate::gpio::{EdgeEvent, GpioBackend, GpioManager, GpioState, PinSettings};

pub struct AppState<B: GpioBackend> {
    pub manager: Arc<GpioManager<B>>,
}

impl<B: GpioBackend> Clone for AppState<B> {
    fn clone(&self) -> Self {
        Self {
            manager: Arc::clone(&self.manager),
        }
    }
}

#[derive(Deserialize)]
struct SettingsPayload {
    state: Option<GpioState>,
    edge: Option<EdgeDetect>,
    debounce_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
struct EventsQuery {
    limit: Option<usize>,
}

async fn handle_event_websocket(
    mut session: Session,
    mut client_stream: MessageStream,
    rx: broadcast::Receiver<EdgeEvent>,
    pin_filter: Option<u32>,
) {
    let mut events = BroadcastStream::new(rx);

    loop {
        tokio::select! {
            msg = client_stream.recv() => {
                let Some(msg) = msg else { break; };

                match msg {
                    Ok(Message::Ping(bytes)) => {
                        let _ = session.pong(&bytes).await;
                    }
                    Ok(Message::Close(reason)) => {
                        let _ = session.close(reason).await;
                        break;
                    }
                    Ok(Message::Text(_))
                    | Ok(Message::Binary(_))
                    | Ok(Message::Pong(_))
                    | Ok(Message::Continuation(_))
                    | Ok(Message::Nop) => {}
                    Err(_) => break,
                }
            }
            event = events.next() => {
                let Some(event) = event else { break; };

                match event {
                    Ok(event) => {
                        if pin_filter.as_ref().map(|p| *p == event.pin_id).unwrap_or(true) {
                            if let Ok(text) = serde_json::to_string(&event) {
                                if session.text(text).await.is_err() {
                                    warn!("WebSocket client disconnected");
                                    break;
                                }
                            }
                        }
                    }
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        if session.text(AppError::Gpio(format!("Event stream lagged by {n} messages")).to_string()).await.is_err() {
                            warn!("WebSocket client lagged and disconnected");
                            break;
                        }
                    }
                }
            }
        }
    }
}

impl<B: GpioBackend + 'static> AppState<B> {
    pub fn api_scope(&self, base_path: &str) -> actix_web::Scope {
        web::scope(base_path)
            .service(
                web::resource("/gpios")
                    .route(web::get().to(list_gpios::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpios/events")
                    .route(web::get().to(events_ws_all::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}")
                    .route(web::get().to(pin_descriptor::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}/info")
                    .route(web::get().to(pin_info::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}/settings")
                    .route(web::get().to(get_settings::<B>))
                    .route(web::post().to(set_settings::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET, Method::POST]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}/value")
                    .route(web::get().to(get_value::<B>))
                    .route(web::post().to(set_value::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET, Method::POST]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}/event")
                    .route(web::get().to(get_last_event::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
            .service(
                web::resource("/gpio/{pin_id}/events")
                    .route(web::get().to(get_events::<B>))
                    .route(
                        web::route()
                            .guard(guard_not_methods(&[Method::GET]))
                            .to(method_not_allowed),
                    ),
            )
    }
}

async fn list_gpios<B: GpioBackend + 'static>(
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pins = state.manager.list_pins().await;

    Ok(web::Json(pins))
}

async fn pin_descriptor<B: GpioBackend + 'static>(
    req: HttpRequest,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;
    let desc = state.manager.get_pin_descriptor(pin_id).await?;

    Ok(web::Json(desc))
}

async fn pin_info<B: GpioBackend + 'static>(
    req: HttpRequest,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;
    let info = state.manager.get_pin_info(pin_id).await?;

    Ok(web::Json(info))
}

async fn get_settings<B: GpioBackend + 'static>(
    req: HttpRequest,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;
    let settings = state.manager.get_pin_settings(pin_id).await?;

    Ok(web::Json(settings))
}

async fn set_settings<B: GpioBackend + 'static>(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;
    let current = state.manager.get_pin_settings(pin_id).await?;
    let merged = parse_settings_payload(&body, current)?;

    state.manager.set_pin_settings(pin_id, &merged).await?;

    Ok(web::Json(merged))
}

async fn get_value<B: GpioBackend + 'static>(
    req: HttpRequest,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;

    let value = state.manager.read_value(pin_id).await?;

    Ok(web::Json(value))
}

async fn set_value<B: GpioBackend + 'static>(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;
    let value = parse_value_payload(&body)?;

    state.manager.write_value(pin_id, value).await?;

    Ok(HttpResponse::Ok())
}

async fn get_last_event<B: GpioBackend + 'static>(
    req: HttpRequest,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;

    let last = state.manager.get_last_event(pin_id).await?;

    match last {
        Some(event) => Ok(HttpResponse::Ok().json(event)),
        None => Ok(HttpResponse::Ok().finish()),
    }
}

async fn get_events<B: GpioBackend + 'static>(
    req: HttpRequest,
    query: web::Query<EventsQuery>,
    state: web::Data<AppState<B>>,
) -> Result<impl Responder, AppError> {
    let pin_id = parse_pin_id(&req)?;

    let events = state.manager.get_events(pin_id, query.limit).await?;

    Ok(web::Json(events))
}

async fn events_ws_all<B: GpioBackend + 'static>(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState<B>>,
) -> Result<HttpResponse, AppError> {
    let rx = state.manager.subscribe_events();
    let (response, session, client_stream) = actix_ws::handle(&req, stream)
        .map_err(|e| AppError::Gpio(format!("Websocket error: {e}")))?;

    actix_web::rt::spawn(async move {
        handle_event_websocket(session, client_stream, rx, None).await;
    });

    Ok(response)
}

fn parse_value_payload(body: &[u8]) -> Result<u8, AppError> {
    if body.is_empty() {
        return Err(AppError::InvalidValue("Empty value payload".into()));
    }

    match std::str::from_utf8(body) {
        Ok(text) => text
            .trim()
            .parse::<u8>()
            .map_err(|_| AppError::InvalidValue("Value must be an integer".into())),
        _ => Err(AppError::InvalidValue(
            "Value payload must be valid UTF-8".into(),
        )),
    }
}

fn parse_pin_id(req: &HttpRequest) -> Result<u32, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("Missing pin id".into()))?;
    let pin_id = pin_id
        .parse::<u32>()
        .map_err(|_| AppError::InvalidValue("Invalid pin id".into()))?;

    Ok(pin_id)
}

fn parse_settings_payload(body: &[u8], current: PinSettings) -> Result<PinSettings, AppError> {
    if body.is_empty() {
        return Err(AppError::InvalidValue("Empty settings payload".into()));
    }

    let payload: SettingsPayload = serde_json::from_slice(body)
        .map_err(|e| AppError::InvalidValue(format!("Invalid settings payload: {e}")))?;

    let mut merged = current;
    if let Some(state) = payload.state {
        merged.state = state;
    }
    if let Some(edge) = payload.edge {
        merged.edge = edge;
    }
    if let Some(debounce) = payload.debounce_ms {
        merged.debounce_ms = debounce;
    }
    Ok(merged)
}

async fn method_not_allowed() -> HttpResponse {
    HttpResponse::MethodNotAllowed().finish()
}

fn guard_not_methods(methods: &[Method]) -> impl guard::Guard {
    let allowed: Vec<Method> = methods.to_vec();
    guard::fn_guard(move |ctx| !allowed.iter().any(|m| m == ctx.head().method))
}
