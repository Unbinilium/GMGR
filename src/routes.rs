use crate::error::AppError;
use crate::gpio::{GpioManager, GpioState, PinDescriptor};
use actix_web::{HttpRequest, HttpResponse, Responder, guard, http::Method, web};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<GpioManager>,
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
            web::resource("/gpio/{pin_id}/state")
                .route(web::get().to(get_state))
                .route(web::post().to(set_state))
                .route(
                    web::route()
                        .guard(guard_not_methods(&[Method::GET, Method::POST]))
                        .to(method_not_allowed),
                ),
        )
}

async fn list_gpios(state: web::Data<AppState>) -> Result<impl Responder, AppError> {
    let pins = state.manager.list_pins().await;
    let response: HashMap<String, PinDescriptor> = pins
        .into_iter()
        .map(|p| (p.info.id.to_string(), p))
        .collect();
    Ok(web::Json(response))
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

async fn get_state(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let state_value = state.manager.get_state(pin_id).await?;
    Ok(HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(state_to_str(state_value)))
}

async fn set_state(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<impl Responder, AppError> {
    let pin_id = req
        .match_info()
        .get("pin_id")
        .ok_or_else(|| AppError::InvalidValue("missing pin id".to_string()))?;
    let desired_state = parse_state_payload(&body)?;
    state.manager.set_state(pin_id, desired_state).await?;
    Ok(HttpResponse::Ok())
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
        .content_type("text/plain; charset=utf-8")
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

fn parse_state_payload(body: &[u8]) -> Result<GpioState, AppError> {
    if body.is_empty() {
        return Err(AppError::InvalidState("empty state payload".to_string()));
    }

    if let Ok(text) = std::str::from_utf8(body) {
        if let Ok(parsed) = parse_state_str(text.trim()) {
            return Ok(parsed);
        }
    }

    Err(AppError::InvalidState(
        "state payload must be one of: disabled, push-pull, floating, pull-up, pull-down"
            .to_string(),
    ))
}

fn parse_state_str(input: &str) -> Result<GpioState, AppError> {
    match input {
        "disabled" => Ok(GpioState::Disabled),
        "push-pull" => Ok(GpioState::PushPull),
        "floating" => Ok(GpioState::Floating),
        "pull-up" => Ok(GpioState::PullUp),
        "pull-down" => Ok(GpioState::PullDown),
        other => Err(AppError::InvalidState(format!("invalid state: {other}"))),
    }
}

fn state_to_str(state: GpioState) -> &'static str {
    match state {
        GpioState::Error => "error",
        GpioState::Disabled => "disabled",
        GpioState::PushPull => "push-pull",
        GpioState::OpenDrain => "open-drain",
        GpioState::OpenSource => "open-source",
        GpioState::Floating => "floating",
        GpioState::PullUp => "pull-up",
        GpioState::PullDown => "pull-down",
    }
}

async fn method_not_allowed() -> HttpResponse {
    HttpResponse::MethodNotAllowed().finish()
}

fn guard_not_methods(methods: &[Method]) -> impl guard::Guard {
    let allowed: Vec<Method> = methods.to_vec();
    guard::fn_guard(move |ctx| !allowed.iter().any(|m| m == ctx.head().method))
}
