use log::info;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use actix_web::{App, HttpServer, web};

use gmgr::{AppConfig, AppState, GpioManager};

#[cfg(feature = "hardware-gpio")]
use gmgr::LibgpiodBackend;
#[cfg(not(feature = "hardware-gpio"))]
use gmgr::MockGpioBackend;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let config_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("GMGR_CONFIG").ok())
        .unwrap_or_else(|| "config.json".to_string());
    let config = Arc::new(
        AppConfig::load_from_file(&config_path)
            .unwrap_or_else(|e| panic!("Failed to load config: {e}")),
    );

    let backend = {
        #[cfg(feature = "hardware-gpio")]
        {
            Arc::new(
                LibgpiodBackend::new()
                    .unwrap_or_else(|e| panic!("Failed to init libgpiod backend: {e}")),
            )
        }
        #[cfg(not(feature = "hardware-gpio"))]
        {
            Arc::new(MockGpioBackend::default())
        }
    };

    let manager = Arc::new(GpioManager::new(config.clone(), backend));
    let app_state = AppState { manager };

    let http_cfg = config.http.clone();
    let server = HttpServer::new(move || {
        let scope_path = http_cfg.path.clone();
        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .service(app_state.api_scope(&scope_path))
    });

    let bind_addrs: String;
    let http_cfg = config.http.clone();
    let server = match (&http_cfg.unix_socket, &http_cfg.host) {
        (Some(socket_path), Some(host)) => {
            if Path::new(socket_path).exists() {
                fs::remove_file(socket_path)?;
            }
            bind_addrs = format!("{} and {}", socket_path, host);

            server.bind_uds(socket_path)?.bind_auto_h2c(host)?
        }
        (Some(socket_path), None) => {
            if Path::new(socket_path).exists() {
                fs::remove_file(socket_path)?;
            }
            bind_addrs = socket_path.clone();

            server.bind_uds(socket_path)?
        }
        (None, Some(host)) => {
            bind_addrs = host.clone();

            server.bind_auto_h2c(host)?
        }
        _ => {
            panic!("Config error: either 'unix_socket' or both 'host' and 'port' must be specified")
        }
    };

    info!("Starting server on {}...", bind_addrs);

    server.run().await
}
