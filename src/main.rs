#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;


use actix::prelude::*;
use actix_web::{HttpServer, App, web};
use env_logger;


mod client_ws;
mod protocol;
mod server_actor;


#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let room_db = server_actor::ServerActor::default().start();

    let bind_addr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8081".to_string());

    println!("Starting server on {}", bind_addr);
    HttpServer::new(move || {
        App::new()
            .data(room_db.clone())
            .route("/", web::get().to(client_ws::matchmaking_start))
    })
        .bind(bind_addr)?
        .run()
        .await
}
