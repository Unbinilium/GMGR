[GMGT]
    A simple GPIO management REST API server written in Rust.

[Build]
    cargo build --release --features hardware-gpio

[Run]
    cargo run --release # mock-gpio feature is default for local runs

[Test]
    cargo test
    curl -vX GET http://localhost:8080/api/v1/gpios | jq
    websocat ws://localhost:8080/api/v1/gpios/events | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1 | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1/info | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1/settings | jq
    curl -vX POST http://localhost:8080/api/v1/gpio/1/settings \
        -d '{"state":"floating","edge":"both","debounce_ms":50}' | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1/value | jq
    curl -vX POST http://localhost:8080/api/v1/gpio/1/value -d 1 | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1/event | jq
    curl -vX GET http://localhost:8080/api/v1/gpio/1/events?limit=5 | jq

[Configuration]
    Edit the config.json file to set up GPIO pins and server settings.

[REST-API]
    /gpios - GET: list all pins with their full description
    /gpios/events - GET: websocket stream events for all pins
    /gpio/{pin_id} - GET: get pin full description
        /info - GET: get pin info (as info from config file)
        /settings - GET/POST: get/set pin settings (state, edge, debounce)
        /value - GET/POST: get/set the value
        /event - GET: get last event for the pin
        /events - GET: get last N events for the pin

[Cross-Building]
    cargo install cross --git https://github.com/cross-rs/cross
    cross build --target <target-triple> --release # e.g., armv7-unknown-linux-gnueabihf
