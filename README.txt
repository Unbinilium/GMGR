[GMGT]
    A simple GPIO management REST API server written in Rust.

[Build]
    cargo build --release # mock-gpio

[Run]
    cargo run --release --features hardware-gpio

[Test]
    cargo test
    curl -X GET http://localhost:8080/api/v1/gpios
    curl -X GET http://localhost:8080/api/v1/gpio/1/info
    curl -X GET http://localhost:8080/api/v1/gpio/1/state
    curl -X POST http://localhost:8080/api/v1/gpio/1/state -d push-pull
    curl -X GET http://localhost:8080/api/v1/gpio/1/value
    curl -X POST http://localhost:8080/api/v1/gpio/1/value -d 1

[Configuration]
    Edit the config.json file to set up GPIO pins and server settings.

[REST-API]
    /gpios - GET: list all GPIO pins and their states in JSON format.
    /gpio/{pin_id}
        /info - GET: get detailed information about a specific GPIO pin in JSON format.
        /state - GET/POST: get/set the current state of the GPIO pin, e.g., push-pull, floating, etc.
        /value - GET/POST: get/set the value of the GPIO pin, e.g., 0 or 1.

[Cross-Building]
    cargo install cross --git https://github.com/cross-rs/cross
    cross build --target <target-triple> --release # e.g., armv7-unknown-linux-gnueabihf
