mod echo_server;
mod server;

use std::sync;

pub use echo_server::EchoServer;
pub use server::Server;

static INIT_TESTS: sync::Once = sync::Once::new();

/// The function `init_logging` initializes logging with the `info` filter in Rust.
pub fn init_logging() {
    INIT_TESTS.call_once(|| {
        pretty_env_logger::formatted_builder()
            .is_test(true)
            .parse_filters("info")
            .init();
    });
}