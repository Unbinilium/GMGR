use std::sync::Arc;

use actix_web::{App, HttpServer, web};

use gmgr::config::AppConfig;
use gmgr::gpio::{GpioBackend, GpioManager};
use gmgr::routes::{AppState, api_scope};

#[cfg(feature = "hardware-gpio")]
use gmgr::gpio::LibgpiodBackend;
#[cfg(not(feature = "hardware-gpio"))]
use gmgr::gpio::MockGpioBackend;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("GMGR_CONFIG").ok())
        .unwrap_or_else(|| "config.json".to_string());
    let config = Arc::new(
        AppConfig::load_from_file(&config_path)
            .unwrap_or_else(|e| panic!("failed to load config: {e}")),
    );

    let backend: Arc<dyn GpioBackend> = {
        #[cfg(feature = "hardware-gpio")]
        {
            Arc::new(
                LibgpiodBackend::new()
                    .unwrap_or_else(|e| panic!("failed to init libgpiod backend: {e}")),
            )
        }
        #[cfg(not(feature = "hardware-gpio"))]
        {
            Arc::new(MockGpioBackend::default())
        }
    };

    let manager = Arc::new(GpioManager::new(config.clone(), backend));
    let app_state = AppState {
        manager: manager.clone(),
    };

    let http_cfg = config.http.clone();
    let bind_addr = format!("{}:{}", http_cfg.host, http_cfg.port);

    println!(
        "Starting GPIO manager on http://{}{}",
        bind_addr, http_cfg.path
    );

    HttpServer::new(move || {
        let state = app_state.clone();
        let scope_path = http_cfg.path.clone();
        App::new()
            .app_data(web::Data::new(state))
            .service(api_scope(&scope_path))
    })
    .bind(bind_addr)?
    .run()
    .await
}
