[GMGT]
    A simple GPIO management REST API server written in Rust.

[Build]
    cargo build --release # mock-gpio

[Run]
    cargo run --release --features hardware-gpio

[Test]
    cargo test
    curl -X GET http://localhost:8080/api/v1/gpios
    curl -X GET http://localhost:8080/api/v1/gpio/1
    curl -X GET http://localhost:8080/api/v1/gpio/1/info
    curl -X GET http://localhost:8080/api/v1/gpio/1/state
    curl -X POST http://localhost:8080/api/v1/gpio/1/state -d push-pull
    curl -X GET http://localhost:8080/api/v1/gpio/1/value
    curl -X POST http://localhost:8080/api/v1/gpio/1/value -d 1

[Configuration]
    Edit the config.json file to set up GPIO pins and server settings.

[REST-API]
    /gpios - GET: list all pins with their full description
    /gpio/{pin_id} - GET: get pin full description
        /info - GET: get pin information
        /state - GET/POST: get/set the current state
        /value - GET/POST: get/set the value

[Cross-Building]
    cargo install cross --git https://github.com/cross-rs/cross
    cross build --target <target-triple> --release # e.g., armv7-unknown-linux-gnueabihf
