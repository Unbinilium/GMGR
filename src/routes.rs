use crate::config::EdgeDetect;
use crate::error::AppError;
use crate::gpio::{EdgeEvent, GpioManager, GpioState, PinSettings};
use actix::prelude::*;
use actix_web::{HttpRequest, HttpResponse, Responder, guard, http::Method, web};
use actix_web_actors::ws;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<GpioManager>,
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

struct EventWs {
    rx: broadcast::Receiver<EdgeEvent>,
    pin_filter: Option<String>,
}

impl Actor for EventWs {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let stream = BroadcastStream::new(self.rx.resubscribe());
        ctx.add_stream(stream);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for EventWs {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Text(_)) | Ok(ws::Message::Binary(_)) => {
                // ignore incoming data from clients
            }
            Ok(ws::Message::Close(reason)) => ctx.close(reason),
            Ok(ws::Message::Pong(_)) => {}
            Ok(ws::Message::Continuation(_)) => {}
            Err(_) => ctx.stop(),
            _ => {}
        }
    }
}

impl StreamHandler<Result<EdgeEvent, BroadcastStreamRecvError>> for EventWs {
    fn handle(
        &mut self,
        item: Result<EdgeEvent, BroadcastStreamRecvError>,
        ctx: &mut Self::Context,
    ) {
        match item {
            Ok(event) => {
                if self
                    .pin_filter
                    .as_ref()
                    .map(|p| p == &event.pin_id)
                    .unwrap_or(true)
                {
                    if let Ok(text) = serde_json::to_string(&event) {
                        ctx.text(text);
                    }
                }
            }
            Err(BroadcastStreamRecvError::Lagged(_)) => {
                // drop lagged messages silently
            }
        }
    }
}

pub fn api_scope(base_path: &str) -> actix_web::Scope {
    web::scope(base_path)
        .service(
            web::resource("/gpios")
                .route(web::get().to(list_gpios))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpios/events")
                .route(web::get().to(events_ws_all))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}")
                .route(web::get().to(pin_descriptor))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}/info")
                .route(web::get().to(pin_info))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}/settings")
                .route(web::get().to(get_settings))
                .route(web::post().to(set_settings))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET, Method::POST]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}/value")
                .route(web::get().to(get_value))
                .route(web::post().to(set_value))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET, Method::POST]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}/event")
                .route(web::get().to(get_last_event))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
        .service(
            web::resource("/gpio/{pin_id}/events")
                .route(web::get().to(get_events))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET]))
                        .to(method_not_allowed),
                ),
        )
}

async fn list_gpios(state: web::Data<AppState>) -> Result<impl Responder, AppError> {
    let pins = state.manager.list_pins().await;
    Ok(web::Json(pins))
}

async fn pin_descriptor(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let desc = state.manager.get_pin_descriptor(pin_id).await?;
    Ok(web::Json(desc))
}

async fn pin_info(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let info = state.manager.get_pin_info(pin_id).await?;
    Ok(web::Json(info))
}

async fn get_settings(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let settings = state.manager.get_pin_settings(pin_id).await?;
    Ok(web::Json(settings))
}

async fn set_settings(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;

    let current = state.manager.get_pin_settings(pin_id).await?;
    let merged = parse_settings_payload(&body, current)?;
    state.manager.set_pin_settings(pin_id, &merged).await?;
    Ok(web::Json(merged))
}

async fn get_value(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let value = state.manager.read_value(pin_id).await?;
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(value.to_string()))
}

async fn set_value(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let value = parse_value_payload(&body)?;
    state.manager.write_value(pin_id, value).await?;
    Ok(HttpResponse::Ok())
}

async fn get_last_event(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let last = state.manager.get_last_event(pin_id).await?;
    if let Some(event) = last {
        Ok(HttpResponse::Ok().json(event))
    } else {
        Ok(HttpResponse::Ok().finish())
    }
}

async fn get_events(
    req: HttpRequest,
    query: web::Query<EventsQuery>,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;

    let mut events = state.manager.get_events(pin_id).await?;
    if let Some(limit) = query.limit {
        if events.len() > limit {
            let start = events.len() - limit;
            events = events.split_off(start);
        }
    }

    Ok(web::Json(events))
}

async fn events_ws_all(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let rx = state.manager.subscribe_events();
    let session = EventWs {
        rx,
        pin_filter: None,
    };

    ws::start(session, &req, stream).map_err(|e| AppError::Gpio(format!("websocket error: {e}")))
}

fn parse_value_payload(body: &[u8]) -> Result<u8, AppError> {
    if body.is_empty() {
        return Err(AppError::InvalidValue("empty value payload".to_string()));
    }

    if let Ok(text) = std::str::from_utf8(body) {
        let trimmed = text.trim();
        if trimmed == "0" {
            return Ok(0);
        }
        if trimmed == "1" {
            return Ok(1);
        }
    }

    Err(AppError::InvalidValue(
        "value payload must be 0 or 1".to_string(),
    ))
}

fn parse_settings_payload(body: &[u8], current: PinSettings) -> Result<PinSettings, AppError> {
    if body.is_empty() {
        return Err(AppError::InvalidValue("empty settings payload".to_string()));
    }

    let payload: SettingsPayload = serde_json::from_slice(body)
        .map_err(|e| AppError::InvalidValue(format!("invalid settings payload: {e}")))?;

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
