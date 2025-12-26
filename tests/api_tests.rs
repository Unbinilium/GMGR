use std::collections::HashMap;
use std::sync::Arc;

use actix_web::{App, test, web};
use gmgr::backend::MockGpioBackend;
use gmgr::config::AppConfig;
use gmgr::gpio::{GpioBackend, GpioManager};
use gmgr::routes::{AppState, api_scope};
use serde_json::Value;

fn sample_config() -> AppConfig {
    serde_json::from_str(
        r#"
        {
            "http": {
                "host": "localhost",
                "port": 8080,
                "path": "/api/v1",
                "timeout": 30
            },
            "gpios": {
                "1": {
                    "name": "LED 1",
                    "chip": "/dev/gpiochip0",
                    "line": 2,
                    "capabilities": [
                        "push-pull"
                    ]
                },
                "2": {
                    "name": "BUTTON 1",
                    "chip": "/dev/gpiochip0",
                    "line": 3,
                    "capabilities": [
                        "floating",
                        "pull-up",
                        "pull-down"
                    ]
                },
                "42": {
                    "name": "General IO 1",
                    "chip": "/dev/gpiochip1",
                    "line": 5,
                    "capabilities": [
                        "push-pull",
                        "open-drain",
                        "open-source",
                        "floating",
                        "pull-up",
                        "pull-down"
                    ]
                }
            },
            "event_history_capacity": 32
        }
        "#,
    )
    .expect("valid sample config")
}

#[actix_rt::test]
async fn list_gpios_returns_all() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;
    let req = test::TestRequest::get().uri("/api/v1/gpios").to_request();
    let response: HashMap<String, Value> = test::call_and_read_body_json(&app, req).await;
    assert_eq!(response.len(), 3);
    assert!(response.contains_key("1"));

    let led = response.get("1").unwrap();
    assert_eq!(led["settings"]["state"], "disabled");
    let cfg = &led["info"];
    assert_eq!(cfg["name"], "LED 1");
    assert_eq!(cfg["chip"], "/dev/gpiochip0");
    assert_eq!(cfg["line"], 2);
}

#[actix_rt::test]
async fn pin_not_found_returns_404() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;
    let req = test::TestRequest::get()
        .uri("/api/v1/gpio/999/info")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_rt::test]
async fn wrong_method_returns_405() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/api/v1/gpio/1/info")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 405);
}

#[actix_rt::test]
async fn set_state_and_value_happy_path() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/api/v1/gpio/1/settings")
        .set_payload(r#"{"state":"push-pull"}"#)
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::post()
        .uri("/api/v1/gpio/1/value")
        .set_payload("1")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/api/v1/gpio/1/value")
        .to_request();
    let body = test::call_and_read_body(&app, req).await;
    assert_eq!(body, "1");
}

#[actix_rt::test]
async fn reject_value_when_not_output() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/api/v1/gpio/2/value")
        .set_payload("1")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_rt::test]
async fn get_pin_info_happy_path() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/api/v1/gpio/1/info")
        .to_request();
    let resp: Value = test::call_and_read_body_json(&app, req).await;

    assert_eq!(resp["name"], "LED 1");
    assert_eq!(resp["chip"], "/dev/gpiochip0");
    assert_eq!(resp["line"], 2);
}

#[actix_rt::test]
async fn get_pin_info_alias_happy_path() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    let req = test::TestRequest::get().uri("/api/v1/gpio/1").to_request();
    let resp: Value = test::call_and_read_body_json(&app, req).await;

    assert_eq!(resp["settings"]["state"], "disabled");
    let cfg = &resp["info"];
    assert_eq!(cfg["name"], "LED 1");
    assert_eq!(cfg["chip"], "/dev/gpiochip0");
    assert_eq!(cfg["line"], 2);
}

#[actix_rt::test]
async fn get_state_happy_path() {
    let cfg = Arc::new(sample_config());
    let backend: Arc<dyn GpioBackend> = Arc::new(MockGpioBackend::default());
    let manager = Arc::new(GpioManager::new(cfg.clone(), backend));
    let state = AppState { manager };
    let scope_path = cfg.http.path.clone();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path)),
    )
    .await;

    // Default state is disabled
    let req = test::TestRequest::get()
        .uri("/api/v1/gpio/1/settings")
        .to_request();
    let settings: Value = test::call_and_read_body_json(&app, req).await;
    assert_eq!(settings["state"], "disabled");

    let req = test::TestRequest::post()
        .uri("/api/v1/gpio/1/settings")
        .set_payload(r#"{"state":"push-pull"}"#)
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/api/v1/gpio/1/settings")
        .to_request();
    let settings: Value = test::call_and_read_body_json(&app, req).await;
    assert_eq!(settings["state"], "push-pull");
}
